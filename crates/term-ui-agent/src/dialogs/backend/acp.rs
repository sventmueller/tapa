//! The overlays' backend for an ACP agent: what the protocol offers, through the running
//! session, and what the app keeps itself, such as the MCP servers it hands over when a session starts and the
//! conversation it shows. What the agent does not announce stays greyed with the reason.

use std::path::PathBuf;
use std::sync::Arc;

use splat_i18n::{t, tf};
use term_acp::capabilities::{Category, OptionValue};
use term_acp::mcp::{McpServerSpec, McpTransport};
use term_base::explain::Explained;
use term_thread::{EntryKind, Lane, Thread};
use term_ui::workspace_host::Trust;

use super::{Backend, Done, acp_mcp};
use crate::dialogs::model::{
    Account, Aside, Exported, InstructionFile, McpCaps, McpDraft, McpServer, McpState, Memory, Scope, Setting,
    Settings, StatusSection, Transport,
};
use crate::tab::AgentTab;

/// The file of instructions ACP agents read in a project.
const INSTRUCTIONS: &str = "AGENTS.md";

pub(super) struct Acp {
    session: Option<term_acp::Session>,
    caps: term_acp::Capabilities,
    thread: Arc<Thread>,
    cwd: PathBuf,
    name: String,
    /// Whether the workspace is trusted, so the project's servers go over (E16).
    trusted: bool,
}

impl Acp {
    pub(super) fn new(tab: &AgentTab, name: &str) -> Acp {
        let state = tab.state.borrow();
        Acp {
            session: state.source().acp(),
            caps: state.acp_caps().cloned().unwrap_or_default(),
            thread: state.thread().clone(),
            cwd: tab.cwd.clone(),
            name: name.to_string(),
            trusted: tab.controller.is_trusted(),
        }
    }

    fn session(&self) -> Result<&term_acp::Session, Explained> {
        self.session
            .as_ref()
            .ok_or_else(|| Explained(t("Die Sitzung läuft nicht.").to_string()))
    }
}

fn failed(error: &term_acp::SessionError) -> Explained {
    Explained::of(error)
}

impl Backend for Acp {
    fn live(&self) -> bool {
        self.session.as_ref().is_some_and(term_acp::Session::is_ready)
    }

    fn status(&self, done: Done<Vec<StatusSection>>) {
        done(Ok(status(&self.caps, &self.name, &self.cwd)));
    }

    fn export(&self, done: Done<Exported>) {
        done(Ok(Exported {
            text: export(&self.thread),
            file_name: super::export_name(&self.thread),
        }));
    }

    fn mcp_caps(&self) -> McpCaps {
        let next = Some(t(
            "Gilt ab dem nächsten Sitzungsstart; ACP meldet keinen Zustand der Server.",
        ));
        let mut transports = vec![Transport::Stdio];
        if self.caps.mcp_http {
            transports.push(Transport::Http);
        }
        if self.caps.mcp_sse {
            transports.push(Transport::Sse);
        }
        McpCaps {
            live: next,
            toggle: None,
            reconnect: Some(t("Über ACP lässt sich ein Server nicht neu verbinden.")),
            sign_in: Some(t("Über ACP gibt es keine Anmeldung bei einem Server.")),
            add: super::cannot(),
            remove: Some(t("Dieser Server steht in .mcp.json des Projekts.")),
            restart: (!self.caps.load_session).then(|| t("Dieser Agent kann eine Sitzung nicht fortsetzen.")),
            transports,
            scopes: vec![Scope::User],
        }
    }

    fn mcp_servers(&self, done: Done<Vec<McpServer>>) {
        let (cwd, trusted) = (self.cwd.clone(), self.trusted);
        super::on_work(
            move || {
                let env = term_ui::workspace_host::environment(std::time::Duration::from_secs(10));
                Ok(acp_mcp::servers(&cwd, &env, trusted).iter().map(listed).collect())
            },
            done,
        );
    }

    fn mcp_toggle(&self, name: &str, enabled: bool, done: Done<()>) {
        let (cwd, name) = (self.cwd.clone(), name.to_string());
        super::on_work(move || acp_mcp::toggle(&cwd, &name, enabled), done);
    }

    fn mcp_add(&self, _: McpDraft, done: Done<()>) {
        done(Err(Explained(t("Dieser Agent kann das hier nicht.").to_string())));
    }

    fn mcp_remove(&self, _: &str, _: Scope, done: Done<()>) {
        done(Err(Explained(
            t("Dieser Server steht in .mcp.json des Projekts.").to_string(),
        )));
    }

    fn memory(&self, done: Done<Memory>) {
        let path = self.cwd.join(INSTRUCTIONS);
        done(Ok(Memory {
            files: vec![InstructionFile {
                label: t("Anweisungen des Projekts").to_string(),
                place: INSTRUCTIONS.to_string(),
                exists: path.exists(),
                path,
            }],
            folders: Vec::new(),
            automatic: None,
        }));
    }

    fn settings(&self, done: Done<Settings>) {
        done(Ok(Settings {
            settings: settings(&self.caps),
            styles: None,
            files: Vec::new(),
        }));
    }

    fn set_setting(&self, key: &str, value: &str, done: Done<()>) {
        let session = match self.session() {
            Ok(session) => session,
            Err(why) => return done(Err(why)),
        };
        let flag = self
            .caps
            .options
            .iter()
            .any(|o| o.id == key && matches!(o.value, OptionValue::Bool(_)));
        let result = if key == MODE {
            session.set_mode(value)
        } else if flag {
            session.set_flag(key, value == "true")
        } else {
            session.set_option(key, value)
        };
        done(result.map_err(|e| failed(&e)));
    }

    fn aside(&self, question: &str, _before: &[Aside], done: Done<Aside>) {
        let session = match self.session() {
            Ok(session) if self.caps.fork => session,
            Ok(_) => return done(Err(super::unavailable())),
            Err(why) => return done(Err(why)),
        };
        let asked = question.to_string();
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(done)));
        let answering = slot.clone();
        let sent = session.ask_aside(question, move |answer| {
            if let Some(done) = term_base::LockIgnoringPoison::lock_ignoring_poison(&*answering).take() {
                done(
                    answer
                        .map(|answer| Aside {
                            question: asked,
                            answer,
                            note: None,
                        })
                        .map_err(|e| failed(&e)),
                );
            }
        });
        if let Err(error) = sent
            && let Some(done) = term_base::LockIgnoringPoison::lock_ignoring_poison(&*slot).take()
        {
            done(Err(failed(&error)));
        }
    }

    fn account(&self, done: Done<Account>) {
        done(Ok(Account {
            signed_in: self.live(),
            method: if self.caps.auth_methods.is_empty() {
                t("Der Agent meldet keine Anmeldung.").to_string()
            } else {
                self.caps
                    .auth_methods
                    .iter()
                    .map(|m| m.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            },
            name: None,
        }));
    }

    fn sign_out(&self, done: Done<()>) {
        let session = match self.session() {
            Ok(session) if self.caps.logout => session,
            Ok(_) => return done(Err(super::unavailable())),
            Err(why) => return done(Err(why)),
        };
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(done)));
        let answering = slot.clone();
        let sent = session.logout(move |result| {
            if let Some(done) = term_base::LockIgnoringPoison::lock_ignoring_poison(&*answering).take() {
                done(result.map_err(|e| failed(&e)));
            }
        });
        if let Err(error) = sent
            && let Some(done) = term_base::LockIgnoringPoison::lock_ignoring_poison(&*slot).take()
        {
            done(Err(failed(&error)));
        }
    }
}

/// `server` as `/mcp` lists it: its state known only once a session starts with it.
fn listed(server: &McpServerSpec) -> McpServer {
    McpServer {
        name: server.name.clone(),
        state: if server.enabled {
            McpState::Unknown
        } else {
            McpState::Disabled
        },
        detail: Some(t("Gilt ab dem nächsten Sitzungsstart").to_string()),
        transport: Some(
            match server.transport {
                McpTransport::Stdio => "stdio",
                McpTransport::Http => "http",
                McpTransport::Sse => "sse",
            }
            .to_string(),
        ),
        scope: Some(acp_mcp::scope(server)),
        tools: None,
        remote: server.transport != McpTransport::Stdio,
    }
}

/// The key under which the session's mode shows among the settings.
const MODE: &str = "mode";

pub(super) fn settings(caps: &term_acp::Capabilities) -> Vec<Setting> {
    let source = t("Sitzung").to_string();
    let mut settings = Vec::new();
    if !caps.modes.is_empty() {
        let current = caps.mode.clone().unwrap_or_default();
        settings.push(Setting {
            key: MODE.to_string(),
            label: t("Modus").to_string(),
            value: caps
                .modes
                .iter()
                .find(|m| m.id == current)
                .map(|m| m.name.clone())
                .unwrap_or_default(),
            source: source.clone(),
            choices: caps.modes.iter().map(|m| (m.id.clone(), m.name.clone())).collect(),
            current,
        });
    }
    let mut options: Vec<_> = caps.options.iter().collect();
    options.sort_by_key(|o| match &o.category {
        Category::Mode => 0,
        Category::Model => 1,
        Category::ThoughtLevel => 2,
        Category::ModelConfig => 3,
        Category::Other(_) | Category::None => 4,
    });
    for option in options {
        let (choices, current) = match &option.value {
            OptionValue::Select { current, choices } => (
                choices.iter().map(|c| (c.value.clone(), c.name.clone())).collect(),
                current.clone(),
            ),
            OptionValue::Bool(on) => (
                vec![
                    ("true".to_string(), t("An").to_string()),
                    ("false".to_string(), t("Aus").to_string()),
                ],
                on.to_string(),
            ),
        };
        settings.push(Setting {
            key: option.id.clone(),
            label: option.name.clone(),
            value: option.current_name(),
            source: source.clone(),
            choices,
            current,
        });
    }
    settings
}

fn status(caps: &term_acp::Capabilities, name: &str, cwd: &std::path::Path) -> Vec<StatusSection> {
    let mut agent = vec![(Some(t("Agent").to_string()), name.to_string())];
    if let Some((id, title, version)) = &caps.agent {
        agent.push((
            Some(t("Programm").to_string()),
            title.clone().unwrap_or_else(|| id.clone()),
        ));
        agent.push((Some(t("Version").to_string()), version.clone()));
    }
    if let Some(version) = caps.protocol_version {
        agent.push((Some(t("Protokoll").to_string()), tf("ACP {}", &[&version.to_string()])));
    }
    let mut session = vec![(Some(t("Ordner").to_string()), cwd.display().to_string())];
    if let Some(mode) = caps
        .mode
        .as_ref()
        .and_then(|id| caps.modes.iter().find(|m| m.id == *id))
    {
        session.push((Some(t("Modus").to_string()), mode.name.clone()));
    }
    if let Some(model) = caps.model() {
        session.push((Some(t("Modell").to_string()), model.current_name()));
    }
    let offers: Vec<&str> = [
        (caps.load_session, t("Sitzungen laden")),
        (caps.fork, t("Fragen nebenbei")),
        (caps.logout, t("Abmelden")),
        (caps.mcp_http, t("MCP über HTTP")),
        (caps.mcp_sse, t("MCP über SSE")),
    ]
    .into_iter()
    .filter_map(|(on, what)| on.then_some(what))
    .collect();
    vec![
        StatusSection {
            title: t("Agent").to_string(),
            rows: agent,
        },
        StatusSection {
            title: t("Sitzung").to_string(),
            rows: session,
        },
        StatusSection {
            title: t("Kann").to_string(),
            rows: vec![(
                None,
                if offers.is_empty() {
                    "—".to_string()
                } else {
                    offers.join(", ")
                },
            )],
        },
    ]
}

/// The conversation as plain text: the user's messages quoted, the agent's as written, tool calls by title.
pub(super) fn export(thread: &Thread) -> String {
    let mut out = String::new();
    for entry in thread.entries(&Lane::Main) {
        let part = match &entry.kind {
            EntryKind::UserMessage(message) => message.text.lines().map(|l| format!("> {l}\n")).collect(),
            EntryKind::AgentMessage(message) => format!("{}\n", message.markdown),
            EntryKind::ToolCall(call) => format!("• {}\n", call.title),
            _ => continue,
        };
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&part);
    }
    out
}
