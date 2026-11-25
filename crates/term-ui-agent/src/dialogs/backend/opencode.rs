//! The overlays' backend for OpenCode over its v2 HTTP API: the management calls of `term_opencode`, read
//! into the neutral model; OpenCode writes its own configuration. What the API does not offer stays greyed with the
//! reason.

use std::path::PathBuf;
use std::sync::Arc;

use splat_i18n::t;
use term_base::explain::Explained;
use term_opencode::{McpConfig, McpStatus};
use term_thread::Thread;

use super::{Backend, Done, acp};
use crate::dialogs::model::{
    Exported, InstructionFile, McpCaps, McpDraft, McpServer, McpState, Memory, Scope, Settings, StatusSection,
    Transport,
};
use crate::tab::AgentTab;

pub(super) struct OpenCode {
    session: term_opencode::Session,
    thread: Arc<Thread>,
    cwd: PathBuf,
}

impl OpenCode {
    pub(super) fn new(tab: &AgentTab, session: term_opencode::Session) -> OpenCode {
        OpenCode {
            session,
            thread: tab.state.borrow().thread().clone(),
            cwd: tab.cwd.clone(),
        }
    }
}

/// `reply`'s answer through `done`, mapped by `map`.
fn answer<T: Send + 'static, U: Send + 'static>(
    reply: term_opencode::Reply<T>,
    done: Done<U>,
    map: impl FnOnce(T) -> U + Send + 'static,
) {
    reply.then(move |result| {
        done(result.map(map).map_err(|e| Explained(crate::opencode::explained(&e))));
    });
}

impl Backend for OpenCode {
    fn live(&self) -> bool {
        self.session.is_ready() && !self.session.is_ended()
    }

    fn status(&self, done: Done<Vec<StatusSection>>) {
        let caps = self.session.capabilities();
        let cwd = self.cwd.clone();
        answer(self.session.credentials(), done, move |credentials| {
            status(&caps, &cwd, &credentials)
        });
    }

    fn export(&self, done: Done<Exported>) {
        done(Ok(Exported {
            text: acp::export(&self.thread),
            file_name: super::export_name(&self.thread),
        }));
    }

    fn mcp_caps(&self) -> McpCaps {
        McpCaps {
            live: None,
            toggle: None,
            reconnect: None,
            sign_in: Some(t(
                "OpenCode meldet sich bei einem Server mit „opencode mcp auth“ im Terminal an.",
            )),
            add: None,
            remove: None,
            restart: Some(t("Änderungen gelten sofort.")),
            transports: vec![Transport::Stdio, Transport::Http],
            scopes: vec![Scope::User],
        }
    }

    fn mcp_servers(&self, done: Done<Vec<McpServer>>) {
        answer(self.session.mcp_servers(), done, |servers| {
            servers
                .into_iter()
                .map(|server| {
                    let (state, detail) = match server.status {
                        McpStatus::Connected => (McpState::Connected, None),
                        McpStatus::Pending => (McpState::Pending, None),
                        McpStatus::Disabled => (McpState::Disabled, None),
                        McpStatus::NeedsAuth(why) => (McpState::NeedsSignIn, Some(why).filter(|w| !w.is_empty())),
                        McpStatus::Failed(why) => (McpState::Failed, Some(why).filter(|w| !w.is_empty())),
                    };
                    McpServer {
                        name: server.name,
                        state,
                        detail,
                        transport: None,
                        scope: Some(Scope::User),
                        tools: None,
                        remote: false,
                    }
                })
                .collect()
        });
    }

    fn mcp_toggle(&self, name: &str, enabled: bool, done: Done<()>) {
        let reply = if enabled {
            self.session.connect_mcp(name)
        } else {
            self.session.disconnect_mcp(name)
        };
        answer(reply, done, |()| ());
    }

    fn mcp_reconnect(&self, name: &str, done: Done<()>) {
        answer(self.session.connect_mcp(name), done, |()| ());
    }

    fn mcp_add(&self, draft: McpDraft, done: Done<()>) {
        let config = match draft.transport {
            Transport::Stdio => McpConfig::Local {
                command: draft.command,
                environment: draft.env,
                cwd: None,
                disabled: false,
            },
            Transport::Http | Transport::Sse => McpConfig::Remote {
                url: draft.url,
                headers: draft.headers,
                disabled: false,
            },
        };
        answer(self.session.add_mcp(&draft.name, &config), done, |()| ());
    }

    fn mcp_remove(&self, name: &str, _scope: Scope, done: Done<()>) {
        answer(self.session.remove_mcp(name), done, |()| ());
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
        done(Ok(Settings {
            settings: acp::settings(&crate::opencode::capabilities(&caps)),
            styles: None,
            files: vec![(t("Projekt").to_string(), self.cwd.join("opencode.json"))],
        }));
    }

    fn set_setting(&self, key: &str, value: &str, done: Done<()>) {
        let result = if key == "mode" {
            self.session.set_mode(value)
        } else {
            self.session.set_option(key, value)
        };
        done(result.map_err(|e| Explained(crate::opencode::explained(&e))));
    }
}

fn status(
    caps: &term_opencode::Capabilities,
    cwd: &std::path::Path,
    credentials: &[term_opencode::Credential],
) -> Vec<StatusSection> {
    let mut server = vec![(Some(t("Agent").to_string()), "OpenCode".to_string())];
    if let Some(version) = &caps.server_version {
        server.push((Some(t("Version").to_string()), version.clone()));
    }
    server.push((Some(t("Protokoll").to_string()), "HTTP v2".to_string()));
    let mut session = vec![(Some(t("Ordner").to_string()), cwd.display().to_string())];
    if let Some(mode) = caps.mode() {
        session.push((Some(t("Modus").to_string()), mode.to_string()));
    }
    if let Some(model) = caps.effective_model() {
        session.push((
            Some(t("Modell").to_string()),
            format!("{}/{}", model.provider, model.model),
        ));
    }
    let providers: Vec<String> = credentials
        .iter()
        .filter(|c| c.active)
        .map(|c| {
            if c.label.is_empty() {
                c.integration.clone()
            } else {
                format!("{} ({})", c.integration, c.label)
            }
        })
        .collect();
    vec![
        StatusSection {
            title: "OpenCode".to_string(),
            rows: server,
        },
        StatusSection {
            title: t("Sitzung").to_string(),
            rows: session,
        },
        StatusSection {
            title: t("Anbieter").to_string(),
            rows: vec![(
                None,
                if providers.is_empty() {
                    "—".to_string()
                } else {
                    providers.join(", ")
                },
            )],
        },
    ]
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use term_opencode::{Capabilities, Credential, CredentialKind, ModelRef};

    use super::*;

    fn row(label: &str, value: &str) -> (Option<String>, String) {
        (Some(label.to_string()), value.to_string())
    }

    fn credential(integration: &str, label: &str, active: bool) -> Credential {
        Credential {
            id: format!("cred_{integration}"),
            integration: integration.into(),
            label: label.into(),
            active,
            kind: CredentialKind::Key,
        }
    }

    #[test]
    fn the_status_names_the_session_and_only_the_providers_in_use() {
        let caps = Capabilities {
            server_version: Some("2.2.6".into()),
            agent: Some("build".into()),
            default_model: Some(ModelRef {
                provider: "anthropic".into(),
                model: "claude-opus".into(),
                variant: None,
            }),
            ..Capabilities::default()
        };
        let credentials = [
            credential("anthropic", "Work", true),
            credential("openai", "", true),
            credential("google", "Old", false),
        ];
        let sections = status(&caps, Path::new("/p"), &credentials);
        assert_eq!(sections[0].rows[1], row(t("Version"), "2.2.6"));
        assert_eq!(
            sections[1].rows,
            [
                row(t("Ordner"), "/p"),
                row(t("Modus"), "build"),
                row(t("Modell"), "anthropic/claude-opus")
            ]
        );
        assert_eq!(sections[2].rows, [(None, "anthropic (Work), openai".to_string())]);
    }

    #[test]
    fn a_status_without_a_model_or_providers_says_so() {
        let sections = status(
            &Capabilities::default(),
            Path::new("/p"),
            &[credential("google", "", false)],
        );
        assert_eq!(sections[0].rows.len(), 2, "no version");
        assert_eq!(sections[1].rows, [row(t("Ordner"), "/p")]);
        assert_eq!(sections[2].rows, [(None, "—".to_string())]);
    }
}
