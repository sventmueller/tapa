//! The overlays' backend for Codex over its app-server: the management calls of `term_codex`, read into the
//! neutral model; Codex writes its own `config.toml` (`config/value/write`), never the app. What the app-server does
//! not offer stays greyed with the reason.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use splat_i18n::{t, tf};
use term_base::explain::Explained;
use term_codex::{Account as CodexAccount, Feedback, MergeStrategy};
use term_thread::Thread;

use super::{Backend, Done, acp};
use crate::dialogs::model::{
    Account, Exported, FeedbackKind, InstructionFile, McpCaps, McpDraft, McpServer, McpState, Memory, Scope, Settings,
    Skill, StatusSection, Transport,
};
use crate::tab::AgentTab;

pub(super) struct Codex {
    session: term_codex::Session,
    thread: Arc<Thread>,
    cwd: PathBuf,
}

impl Codex {
    pub(super) fn new(tab: &AgentTab, session: term_codex::Session) -> Codex {
        Codex {
            session,
            thread: tab.state.borrow().thread().clone(),
            cwd: tab.cwd.clone(),
        }
    }
}

fn failed(error: &term_codex::SessionError) -> Explained {
    Explained(crate::codex::explained(error))
}

/// `reply`'s answer through `done`, mapped by `map`.
fn answer<T: Send + 'static, U: Send + 'static>(
    reply: term_codex::Reply<T>,
    done: Done<U>,
    map: impl FnOnce(T) -> U + Send + 'static,
) {
    reply.then(move |result| done(result.map(map).map_err(|e| failed(&e))));
}

impl Backend for Codex {
    fn live(&self) -> bool {
        self.session.is_ready() && !self.session.is_ended()
    }

    fn status(&self, done: Done<Vec<StatusSection>>) {
        let caps = self.session.capabilities();
        let cwd = self.cwd.clone();
        // Signed in with a key or a provider there are no plan limits, which leaves the rest of the status as it is.
        self.session
            .rate_limits()
            .then(move |limits| done(Ok(status(&caps, &cwd, &limits.unwrap_or_default()))));
    }

    fn export(&self, done: Done<Exported>) {
        done(Ok(Exported {
            text: acp::export(&self.thread),
            file_name: super::export_name(&self.thread),
        }));
    }

    fn mcp_caps(&self) -> McpCaps {
        let toml = Some(t("Codex schaltet und entfernt Server nur in seiner config.toml."));
        McpCaps {
            live: None,
            toggle: toml,
            reconnect: None,
            sign_in: None,
            add: None,
            remove: toml,
            restart: Some(t("Änderungen gelten sofort.")),
            transports: vec![Transport::Stdio, Transport::Http],
            scopes: vec![Scope::User],
        }
    }

    fn mcp_servers(&self, done: Done<Vec<McpServer>>) {
        answer(self.session.mcp_servers(), done, |servers| {
            servers
                .into_iter()
                .map(|server| McpServer {
                    state: match (&server.error, server.auth.as_str()) {
                        (Some(_), _) => McpState::Failed,
                        (None, "notLoggedIn") => McpState::NeedsSignIn,
                        _ => McpState::Connected,
                    },
                    detail: server.error.clone(),
                    transport: None,
                    scope: Some(Scope::User),
                    tools: Some(server.tools.len()),
                    remote: matches!(server.auth.as_str(), "notLoggedIn" | "oAuth" | "bearerToken"),
                    name: server.name,
                })
                .collect()
        });
    }

    fn mcp_reconnect(&self, _name: &str, done: Done<()>) {
        answer(self.session.reload_mcp(), done, |()| ());
    }

    fn mcp_sign_in(&self, name: &str, done: Done<Option<String>>) {
        answer(self.session.mcp_login(name), done, |login| {
            Some(login.authorization_url)
        });
    }

    fn mcp_add(&self, draft: McpDraft, done: Done<()>) {
        let object = |pairs: &[(String, String)]| -> Map<String, Value> {
            pairs.iter().map(|(k, v)| (k.clone(), Value::from(v.clone()))).collect()
        };
        let server = match draft.transport {
            Transport::Stdio => {
                let (command, args) = draft
                    .command
                    .split_first()
                    .map_or((String::new(), Vec::new()), |(c, a)| (c.clone(), a.to_vec()));
                json!({ "command": command, "args": args, "env": object(&draft.env) })
            }
            Transport::Http | Transport::Sse => json!({ "url": draft.url, "http_headers": object(&draft.headers) }),
        };
        let session = self.session.clone();
        let key = format!("mcp_servers.{}", draft.name);
        self.session
            .write_config(&key, server, MergeStrategy::Upsert)
            .then(move |written| match written {
                Ok(_) => answer(session.reload_mcp(), done, |()| ()),
                Err(error) => done(Err(failed(&error))),
            });
    }

    fn memory(&self, done: Done<Memory>) {
        let path = self.cwd.join("AGENTS.md");
        done(Ok(Memory {
            files: vec![InstructionFile {
                label: t("Anweisungen des Projekts").to_string(),
                place: "AGENTS.md".to_string(),
                exists: path.exists(),
                path,
            }],
            folders: Vec::new(),
            automatic: None,
        }));
    }

    fn settings(&self, done: Done<Settings>) {
        let caps = self.session.capabilities();
        let files = caps
            .codex_home
            .iter()
            .map(|home| (t("Alle meine Projekte").to_string(), home.join("config.toml")))
            .collect();
        done(Ok(Settings {
            settings: acp::settings(&crate::codex::capabilities(&caps)),
            styles: None,
            files,
        }));
    }

    fn set_setting(&self, key: &str, value: &str, done: Done<()>) {
        let result = if key == "mode" {
            self.session.set_mode(value)
        } else {
            self.session.set_option(key, value)
        };
        done(result.map_err(|e| failed(&e)));
    }

    fn skills(&self, done: Done<Vec<Skill>>) {
        answer(self.session.skills(false), done, |folders| {
            folders
                .into_iter()
                .flat_map(|folder| folder.skills)
                .map(|skill| Skill {
                    shown: skill.name.clone(),
                    name: skill.name,
                    description: skill.description,
                    source: skill.scope,
                    tokens: None,
                    enabled: skill.enabled,
                    handles: json!({ "path": skill.path }),
                })
                .collect()
        });
    }

    fn set_skill(&self, skill: &Skill, on: bool, done: Done<()>) {
        let path = PathBuf::from(skill.handles["path"].as_str().unwrap_or_default());
        answer(self.session.set_skill(&path, on), done, |_| ());
    }

    fn account(&self, done: Done<Account>) {
        answer(self.session.account(), done, |state| match state.account {
            Some(CodexAccount::ChatGpt { email, plan }) => Account {
                signed_in: true,
                method: tf("ChatGPT ({})", &[&plan]),
                name: email,
            },
            Some(CodexAccount::ApiKey) => Account {
                signed_in: true,
                method: t("API-Schlüssel").to_string(),
                name: None,
            },
            Some(CodexAccount::Provider(provider)) => Account {
                signed_in: true,
                method: provider,
                name: None,
            },
            None => Account {
                signed_in: false,
                method: t("Nicht angemeldet").to_string(),
                name: None,
            },
        });
    }

    fn sign_out(&self, done: Done<()>) {
        answer(self.session.logout(), done, |()| ());
    }

    fn feedback_sends(&self) -> Option<&'static str> {
        Some(t(
            "Codex sendet an OpenAI: deine Beschreibung und die Protokolle dieser Sitzung, mit den Befehlen und Pfaden darin.",
        ))
    }

    fn send_feedback(&self, kind: FeedbackKind, title: &str, description: &str, done: Done<String>) {
        let feedback = Feedback {
            classification: match kind {
                FeedbackKind::Bug => "bug",
                FeedbackKind::Idea => "other",
            }
            .to_string(),
            reason: Some(if title.is_empty() {
                description.to_string()
            } else {
                format!("{title}\n\n{description}")
            }),
            include_logs: true,
        };
        answer(self.session.upload_feedback(&feedback), done, |sent| {
            tf("Gesendet. Nummer des Berichts: {}", &[&sent.thread_id])
        });
    }
}

fn status(
    caps: &term_codex::Capabilities,
    cwd: &std::path::Path,
    limits: &[term_thread::RateLimit],
) -> Vec<StatusSection> {
    let mut codex = vec![(Some(t("Agent").to_string()), "Codex".to_string())];
    if let Some(version) = caps.version {
        codex.push((Some(t("Version").to_string()), version.to_string()));
    }
    codex.push((Some(t("Protokoll").to_string()), "app-server".to_string()));
    let account = match &caps.account {
        Some(CodexAccount::ChatGpt { email, plan }) => {
            format!(
                "ChatGPT {plan}{}",
                email.as_ref().map(|e| format!(" · {e}")).unwrap_or_default()
            )
        }
        Some(CodexAccount::ApiKey) => t("API-Schlüssel").to_string(),
        Some(CodexAccount::Provider(provider)) => provider.clone(),
        None => t("Nicht angemeldet").to_string(),
    };
    codex.push((Some(t("Konto").to_string()), account));
    let mut session = vec![(Some(t("Ordner").to_string()), cwd.display().to_string())];
    if let Some(model) = &caps.model {
        session.push((Some(t("Modell").to_string()), model.clone()));
    }
    if let Some(effort) = &caps.effort {
        session.push((Some(t("Aufwand").to_string()), effort.clone()));
    }
    let mut sections = vec![
        StatusSection {
            title: "Codex".to_string(),
            rows: codex,
        },
        StatusSection {
            title: t("Sitzung").to_string(),
            rows: session,
        },
    ];
    if !limits.is_empty() {
        sections.push(StatusSection {
            title: t("Grenzen").to_string(),
            rows: limits
                .iter()
                .map(|limit| {
                    let used = limit
                        .utilization
                        .map_or_else(|| "—".to_string(), |u| format!("{:.0} %", u * 100.0));
                    (Some(limit.window.clone()), used)
                })
                .collect(),
        });
    }
    sections
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use term_codex::{Account as CodexAccount, Capabilities, Version};
    use term_thread::{LimitStatus, RateLimit};

    use super::*;

    fn row(label: &str, value: &str) -> (Option<String>, String) {
        (Some(label.to_string()), value.to_string())
    }

    /// What the status says of the account `account`.
    fn account(account: Option<CodexAccount>) -> Option<String> {
        let caps = Capabilities {
            account,
            ..Capabilities::default()
        };
        let sections = status(&caps, Path::new("/p"), &[]);
        sections[0]
            .rows
            .iter()
            .find(|(label, _)| label.as_deref() == Some(t("Konto")))
            .map(|(_, value)| value.clone())
    }

    #[test]
    fn the_status_names_the_account_however_codex_is_signed_in() {
        let chatgpt = |email: Option<&str>| CodexAccount::ChatGpt {
            email: email.map(str::to_string),
            plan: "pro".into(),
        };
        assert_eq!(
            account(Some(chatgpt(Some("dev@example.test")))).as_deref(),
            Some("ChatGPT pro · dev@example.test")
        );
        assert_eq!(account(Some(chatgpt(None))).as_deref(), Some("ChatGPT pro"));
        assert_eq!(account(Some(CodexAccount::ApiKey)).as_deref(), Some(t("API-Schlüssel")));
        assert_eq!(
            account(Some(CodexAccount::Provider("gcp".into()))).as_deref(),
            Some("gcp")
        );
        assert_eq!(account(None).as_deref(), Some(t("Nicht angemeldet")));
    }

    #[test]
    fn the_status_shows_the_thread_and_the_plan_limits_only_when_there_are_some() {
        let caps = Capabilities {
            version: Some(Version {
                major: 0,
                minor: 167,
                patch: 0,
            }),
            model: Some("gpt-5.5".into()),
            effort: Some("high".into()),
            ..Capabilities::default()
        };
        let without = status(&caps, Path::new("/p"), &[]);
        assert_eq!(without.len(), 2);
        assert_eq!(without[0].rows[1], row(t("Version"), "0.167.0"));
        assert_eq!(
            without[1].rows,
            [
                row(t("Ordner"), "/p"),
                row(t("Modell"), "gpt-5.5"),
                row(t("Aufwand"), "high")
            ]
        );
        let limit = |window: &str, utilization| RateLimit {
            window: window.into(),
            status: LimitStatus::Allowed,
            utilization,
            resets_at: None,
        };
        let with = status(
            &caps,
            Path::new("/p"),
            &[limit("five_hour", Some(0.42)), limit("weekly", None)],
        );
        assert_eq!(with[2].title, t("Grenzen"));
        assert_eq!(with[2].rows, [row("five_hour", "42 %"), row("weekly", "—")]);
    }
}
