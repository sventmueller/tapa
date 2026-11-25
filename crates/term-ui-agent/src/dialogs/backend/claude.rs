//! The overlays' backend for Claude Code: control requests over the running session and the CLI's own writers run
//! once each on the work lane.

use std::path::PathBuf;
use std::time::Duration;

use dispatch2::MainThreadBound;
use objc2::MainThreadMarker;
use serde_json::{Map, Value, json};
use term_base::explain::Explained;
use term_claude::Session;
use term_claude::cli::{Cli, CliError, HookAt, HookEdit, PermissionEdit};

use super::{Backend, Done, claude_map};
use crate::dialogs::model::{
    Account, Aside, Behavior, Blocked, Exported, FeedbackKind, Hook, HookDraft, Hooks, MarketChange, McpCaps, McpDraft,
    McpServer, McpState, Memory, PermissionCaps, PermissionRule, Permissions, PluginChange, PluginStep, Plugins, Scope,
    Settings, Skill, StatusSection, Transport,
};
use crate::tab::AgentTab;

/// How long to wait for the login shell's environment a writer runs with.
const ENVIRONMENT: Duration = Duration::from_secs(10);

pub(super) struct Claude {
    session: Option<Session>,
    cwd: PathBuf,
    /// The output style in effect and the ones to choose from, as the CLI announced them.
    styles: Option<(String, Vec<String>)>,
}

impl Claude {
    pub(super) fn new(tab: &AgentTab) -> Claude {
        let styles = tab.state.borrow().claude_caps().and_then(|caps| {
            let current = caps.output_style.clone()?;
            Some((current, caps.output_styles.clone()))
        });
        Claude {
            session: tab.session(),
            cwd: tab.cwd.clone(),
            styles,
        }
    }

    fn session(&self) -> Result<&Session, Explained> {
        self.session
            .as_ref()
            .ok_or_else(|| Explained(splat_i18n::t("Die Sitzung läuft nicht.").to_string()))
    }

    /// Runs `work` with the CLI on the work lane and answers through `done`, also when the lane is full.
    fn with_cli<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Cli) -> Result<T, CliError> + Send + 'static,
        done: Done<T>,
    ) {
        let cwd = self.cwd.clone();
        super::on_work(
            move || {
                let env = term_ui::workspace_host::environment(ENVIRONMENT);
                let Some(program) = crate::source::find_claude(&env) else {
                    return Err(Explained(splat_i18n::t("Claude Code nicht gefunden").to_string()));
                };
                work(&Cli::new(program, env, cwd)).map_err(|e| Explained::of(&e))
            },
            done,
        );
    }
}

/// Removes the allow rule `rule` Claude stored in `scope` of `project` through Claude's own writer, or straight from
/// the settings file where Claude Code is missing or too old for that writer; `done` runs on the main
/// thread.
pub fn remove_claude_rule(
    mtm: MainThreadMarker,
    project: PathBuf,
    scope: term_claude::rules::Scope,
    rule: String,
    done: impl FnOnce(Result<(), Explained>) + 'static,
) {
    let bound = MainThreadBound::new(done, mtm);
    let back: Done<()> = Box::new(move |result| {
        crate::common::util::main_soon(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                (bound.into_inner(mtm))(result);
            }
        });
    });
    let source = claude_map::settings_source(match scope {
        term_claude::rules::Scope::Local => Scope::Local,
        term_claude::rules::Scope::Project => Scope::Project,
    });
    super::on_work(
        move || {
            let env = term_ui::workspace_host::environment(ENVIRONMENT);
            let edit = PermissionEdit::Remove {
                rule: rule.clone(),
                behavior: "allow".to_string(),
                source: source.to_string(),
            };
            let written = crate::source::find_claude(&env)
                .map(|program| Cli::new(program, env, project.clone()).edit_permission_rules(&edit));
            match written {
                Some(Ok(_)) => Ok(()),
                None | Some(Err(CliError::Unknown)) => term_claude::rules::remove_allow_rule(&project, scope, &rule)
                    .map(|_| ())
                    .map_err(|e| Explained::of(&e)),
                Some(Err(error)) => Err(Explained::of(&error)),
            }
        },
        back,
    );
}

/// `answer` of a control request in the user's words.
fn explained<T>(answer: Result<T, term_claude::ControlError>) -> Result<T, Explained> {
    answer.map_err(|e| Explained::of(&e))
}

impl Backend for Claude {
    fn live(&self) -> bool {
        self.session.is_some()
    }

    fn status(&self, done: Done<Vec<StatusSection>>) {
        match self.session() {
            Ok(session) => session.status_report().then(move |report| {
                done(explained(report).map(|report| {
                    report
                        .sections
                        .into_iter()
                        .map(|section| StatusSection {
                            title: section.title,
                            rows: section.rows.into_iter().map(|row| (row.label, row.value)).collect(),
                        })
                        .collect()
                }))
            }),
            Err(why) => done(Err(why)),
        }
    }

    fn export(&self, done: Done<Exported>) {
        match self.session() {
            Ok(session) => session.export_conversation().then(move |exported| {
                done(explained(exported).map(|exported| Exported {
                    text: exported.text,
                    file_name: exported.default_filename,
                }))
            }),
            Err(why) => done(Err(why)),
        }
    }

    fn aside(&self, question: &str, before: &[Aside], done: Done<Aside>) {
        let history = before
            .iter()
            .map(|aside| term_claude::control::SideExchange {
                question: aside.question.clone(),
                answer: aside.answer.clone(),
            })
            .collect();
        let asked = question.to_string();
        match self.session() {
            Ok(session) => session
                .side_question(question, history)
                .then(move |answer| done(explained(answer).map(|answer| claude_map::aside(asked, answer)))),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_caps(&self) -> McpCaps {
        McpCaps {
            live: None,
            toggle: None,
            reconnect: None,
            sign_in: None,
            add: None,
            remove: None,
            restart: Some(splat_i18n::t("Änderungen gelten sofort.")),
            transports: Transport::ALL.to_vec(),
            scopes: Scope::ALL.to_vec(),
        }
    }

    fn mcp_servers(&self, done: Done<Vec<McpServer>>) {
        match self.session() {
            Ok(session) => session.mcp_status().then(move |status| {
                done(explained(status).map(|status| status.mcp_servers.into_iter().map(server).collect()))
            }),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_toggle(&self, name: &str, enabled: bool, done: Done<()>) {
        match self.session() {
            Ok(session) => session.mcp_toggle(name, enabled).then(move |r| done(explained(r))),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_reconnect(&self, name: &str, done: Done<()>) {
        match self.session() {
            Ok(session) => session.mcp_reconnect(name).then(move |r| done(explained(r))),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_sign_in(&self, name: &str, done: Done<Option<String>>) {
        match self.session() {
            Ok(session) => session.mcp_sign_in(name).then(move |answer| {
                done(explained(answer).and_then(|answer| match answer.error {
                    Some(why) => Err(Explained(why)),
                    None => Ok(answer.auth_url),
                }))
            }),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_sign_in_answer(&self, name: &str, url: &str, done: Done<()>) {
        match self.session() {
            Ok(session) => session.mcp_callback_url(name, url).then(move |r| done(explained(r))),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_sign_out(&self, name: &str, done: Done<()>) {
        match self.session() {
            Ok(session) => session.mcp_sign_out(name).then(move |r| done(explained(r))),
            Err(why) => done(Err(why)),
        }
    }

    fn mcp_add(&self, draft: McpDraft, done: Done<()>) {
        let definition = definition(&draft);
        self.with_cli(
            move |cli| cli.mcp_add(&draft.name, &definition, draft.scope.id()).map(|_| ()),
            done,
        );
    }

    fn mcp_remove(&self, name: &str, scope: Scope, done: Done<()>) {
        let name = name.to_string();
        self.with_cli(move |cli| cli.mcp_remove(&name, scope.id()).map(|_| ()), done);
    }

    fn permission_caps(&self) -> PermissionCaps {
        PermissionCaps {
            add: None,
            remove: None,
            scopes: Scope::ALL.to_vec(),
        }
    }

    fn permissions(&self, done: Done<Permissions>) {
        match self.session() {
            Ok(session) => session
                .permission_rules()
                .then(move |rules| done(explained(rules).map(claude_map::permissions))),
            Err(why) => done(Err(why)),
        }
    }

    fn rule_add(&self, rule: &str, behavior: Behavior, scope: Scope, done: Done<()>) {
        let edit = PermissionEdit::Add {
            rules: vec![rule.to_string()],
            behavior: behavior.id().to_string(),
            destination: claude_map::settings_source(scope).to_string(),
        };
        self.with_cli(move |cli| cli.edit_permission_rules(&edit).map(|_| ()), done);
    }

    fn rule_remove(&self, rule: &PermissionRule, done: Done<()>) {
        let edit = PermissionEdit::Remove {
            rule: rule.rule.clone(),
            behavior: rule.behavior.id().to_string(),
            source: rule.source.clone(),
        };
        self.with_cli(move |cli| cli.edit_permission_rules(&edit).map(|_| ()), done);
    }

    fn hooks_blocked(&self) -> Blocked {
        None
    }

    fn hooks(&self, done: Done<Hooks>) {
        match self.session() {
            Ok(session) => session
                .hooks_listing()
                .then(move |listing| done(explained(listing).map(claude_map::hooks))),
            Err(why) => done(Err(why)),
        }
    }

    fn hook_add(&self, draft: HookDraft, done: Done<()>) {
        let edit = HookEdit::Add {
            source: claude_map::settings_source(draft.scope).to_string(),
            hook: claude_map::hook_definition(&draft),
            event: draft.event,
            matcher: draft.matcher,
        };
        self.with_cli(move |cli| cli.edit_hook(&edit).map(|_| ()), done);
    }

    fn hook_replace(&self, old: &Hook, draft: HookDraft, done: Done<()>) {
        let edit = HookEdit::Replace {
            source: old.source.clone(),
            hook: claude_map::hook_definition(&draft),
            event: draft.event,
            matcher: draft.matcher,
            target: target(old),
        };
        self.with_cli(move |cli| cli.edit_hook(&edit).map(|_| ()), done);
    }

    fn hook_remove(&self, hook: &Hook, done: Done<()>) {
        let edit = HookEdit::Remove {
            source: hook.source.clone(),
            target: target(hook),
        };
        self.with_cli(move |cli| cli.edit_hook(&edit).map(|_| ()), done);
    }

    fn memory(&self, done: Done<Memory>) {
        match self.session() {
            Ok(session) => session
                .memory_dialog()
                .then(move |dialog| done(explained(dialog).map(claude_map::memory))),
            Err(why) => done(Err(why)),
        }
    }

    fn set_auto_memory(&self, on: bool, done: Done<()>) {
        self.with_cli(move |cli| cli.set_auto_memory(on).map(|_| ()), done);
    }

    fn settings(&self, done: Done<Settings>) {
        let styles = self.styles.clone();
        let project = self.cwd.clone();
        match self.session() {
            Ok(session) => session.settings().then(move |report| {
                let config = term_claude::history::config_dir(&term_ui::workspace_host::environment(ENVIRONMENT));
                done(explained(report).map(|report| claude_map::settings(&report, styles, &config, &project)))
            }),
            Err(why) => done(Err(why)),
        }
    }

    fn set_style(&self, style: &str, done: Done<()>) {
        match self.session() {
            Ok(session) => {
                let reload = session.clone();
                session.set_output_style(style).then(move |set| match explained(set) {
                    Ok(()) => reload
                        .reload_output_styles()
                        .then(move |r| done(explained(r).map(|_| ()))),
                    Err(why) => done(Err(why)),
                });
            }
            Err(why) => done(Err(why)),
        }
    }

    fn skills(&self, done: Done<Vec<Skill>>) {
        match self.session() {
            Ok(session) => session
                .skills_dialog()
                .then(move |dialog| done(explained(dialog).map(claude_map::skills))),
            Err(why) => done(Err(why)),
        }
    }

    fn plugins(&self, done: Done<Plugins>) {
        self.with_cli(super::claude_plugins::list, done);
    }

    fn plugin_install(&self, id: &str, scope: Scope, update: bool, accept: Option<String>, done: Done<PluginStep>) {
        let (id, session) = (id.to_string(), self.session.clone());
        self.with_cli(
            move |cli| super::claude_plugins::install(cli, session.as_ref(), &id, scope, update, accept.as_deref()),
            done,
        );
    }

    fn plugin_change(&self, id: &str, scope: Scope, change: PluginChange, done: Done<()>) {
        let (id, session) = (id.to_string(), self.session.clone());
        self.with_cli(
            move |cli| super::claude_plugins::change(cli, session.as_ref(), &id, scope, change),
            done,
        );
    }

    fn market_change(&self, name: &str, change: MarketChange, done: Done<()>) {
        let (name, session) = (name.to_string(), self.session.clone());
        self.with_cli(
            move |cli| super::claude_plugins::market(cli, session.as_ref(), &name, change),
            done,
        );
    }

    fn account(&self, done: Done<Account>) {
        self.with_cli(|cli| cli.auth_status().map(claude_map::account), done);
    }

    fn sign_out(&self, done: Done<()>) {
        self.with_cli(|cli| cli.auth_logout().map(|_| ()), done);
    }

    fn feedback_sends(&self) -> Option<&'static str> {
        Some(splat_i18n::t(
            "Claude Code sendet an Anthropic: deine Beschreibung, die ganze Unterhaltung dieser Sitzung mit den Pfaden darin und sein bereinigtes Fehlerprotokoll. Ohne Anthropic-Konto legt es den Bericht stattdessen als ZIP auf diesem Mac ab.",
        ))
    }

    fn send_feedback(&self, kind: FeedbackKind, title: &str, description: &str, done: Done<String>) {
        match self.session() {
            Ok(session) => session
                .submit_feedback(kind.id(), title, description)
                .then(move |result| done(explained(result).and_then(claude_map::feedback))),
            Err(why) => done(Err(why)),
        }
    }

    fn set_skill(&self, skill: &Skill, on: bool, done: Done<()>) {
        let (name, handles) = (skill.name.clone(), skill.handles.clone());
        let session = self.session.clone();
        self.with_cli(
            move |cli| {
                cli.set_skill_state(&name, if on { "on" } else { "off" }, &handles)?;
                if let Some(session) = session {
                    // The listing follows the file once the CLI reads its skills again.
                    let _ = session.reload_skills().wait();
                }
                Ok(())
            },
            done,
        );
    }
}

/// The hook `hook` as an edit names it.
fn target(hook: &Hook) -> HookAt {
    HookAt {
        event: hook.event.clone(),
        matcher: hook.matcher.clone(),
        hook: hook.definition.clone().unwrap_or(Value::Null),
    }
}

/// A server as `mcp_status` reports it, in the neutral model.
fn server(state: term_claude::control::McpServerState) -> McpServer {
    let status = match state.status.as_str() {
        "connected" => McpState::Connected,
        "pending" => McpState::Pending,
        "failed" => McpState::Failed,
        "needs-auth" => McpState::NeedsSignIn,
        "disabled" => McpState::Disabled,
        _ => McpState::Unknown,
    };
    let remote = matches!(state.transport.as_deref(), Some("http" | "sse" | "ws"));
    McpServer {
        tools: (status == McpState::Connected).then_some(state.tools.len()),
        state: status,
        detail: state.error,
        transport: state.transport,
        scope: state.scope.as_deref().and_then(Scope::from_id),
        name: state.name,
        remote,
    }
}

/// The definition `claude mcp add-json` takes for `draft`.
fn definition(draft: &McpDraft) -> Value {
    let pairs = |pairs: &[(String, String)]| -> Map<String, Value> {
        pairs.iter().map(|(k, v)| (k.clone(), Value::from(v.clone()))).collect()
    };
    match draft.transport {
        Transport::Stdio => {
            let (command, args) = draft
                .command
                .split_first()
                .map_or((String::new(), &[][..]), |(c, a)| (c.clone(), a));
            let mut definition = json!({ "type": "stdio", "command": command, "args": args });
            if !draft.env.is_empty() {
                definition["env"] = Value::Object(pairs(&draft.env));
            }
            definition
        }
        Transport::Http | Transport::Sse => {
            let mut definition = json!({ "type": draft.transport.id(), "url": draft.url });
            if !draft.headers.is_empty() {
                definition["headers"] = Value::Object(pairs(&draft.headers));
            }
            definition
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_draft_becomes_the_definition_the_cli_takes() {
        let stdio = McpDraft {
            name: "docs".into(),
            transport: Transport::Stdio,
            command: vec!["npx".into(), "-y".into(), "docs-mcp".into()],
            url: String::new(),
            env: vec![("KEY".into(), "v".into())],
            headers: Vec::new(),
            scope: Scope::Project,
        };
        assert_eq!(
            definition(&stdio),
            json!({ "type": "stdio", "command": "npx", "args": ["-y", "docs-mcp"], "env": { "KEY": "v" } })
        );
        let http = McpDraft {
            transport: Transport::Http,
            url: "https://x.example/mcp".into(),
            headers: vec![("Authorization".into(), "Bearer t".into())],
            ..stdio
        };
        assert_eq!(
            definition(&http),
            json!({ "type": "http", "url": "https://x.example/mcp", "headers": { "Authorization": "Bearer t" } })
        );
    }

    #[test]
    fn the_states_map_onto_the_neutral_model() {
        let failed = server(term_claude::control::McpServerState {
            name: "broken".into(),
            status: "failed".into(),
            error: Some("ENOENT".into()),
            transport: Some("stdio".into()),
            scope: Some("user".into()),
            ..Default::default()
        });
        assert_eq!(failed.state, McpState::Failed);
        assert_eq!(failed.scope, Some(Scope::User));
        assert!(!failed.remote && failed.tools.is_none());
    }
}
