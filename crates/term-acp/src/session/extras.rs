//! What the overlays ask an ACP session besides the conversation (E146, `docs/acp.md`): the MCP servers handed over
//! when it opens, signing out, and a question beside the conversation, asked in a fork of the session whose output
//! the main thread never sees and whose tool calls are refused.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::mcp::{McpServerSpec, McpTransport};
use agent_client_protocol::v1::schema as acp;

use super::{SETTING, Session, SessionError, ask};
use crate::capabilities::Capabilities;

/// How long a question beside the conversation may take.
const ASIDE: Duration = Duration::from_secs(300);

/// What gets the answer to a question beside the conversation.
type Answered = Box<dyn FnOnce(Result<String, SessionError>) + Send>;

/// A fork of the session that answers a question beside the conversation.
pub(super) struct Side {
    /// The fork's id, once the agent forked; none while it forks.
    pub(super) session: Option<acp::SessionId>,
    /// What the agent said in it so far.
    text: String,
}

impl Side {
    /// Whether `session` is this fork.
    pub(super) fn is(&self, session: &acp::SessionId) -> bool {
        self.session.as_ref() == Some(session)
    }

    /// Takes in `update` of the fork: its words; the rest stays out of the thread.
    pub(super) fn take(&mut self, update: &acp::SessionUpdate) {
        if let acp::SessionUpdate::AgentMessageChunk(chunk) = update
            && let acp::ContentBlock::Text(text) = &chunk.content
        {
            self.text.push_str(&text.text);
        }
    }
}

impl Session {
    /// Signs out of the agent, where its capabilities announce it; `done` gets the outcome on a worker thread.
    pub fn logout(&self, done: impl FnOnce(Result<(), SessionError>) + Send + 'static) -> Result<(), SessionError> {
        if !self.capabilities().logout {
            return Err(SessionError::Agent("logout is not offered".to_string()));
        }
        let connection = self.shared.connection().ok_or(SessionError::Closed)?;
        ask(&connection, acp::LogoutRequest::new(), Some(SETTING), move |result| {
            done(result.map(|_| ()).map_err(|e| SessionError::Agent(e.message)));
        });
        Ok(())
    }

    /// Asks `question` in a fork of the session, which neither shows in the thread nor runs tools; `done` gets the
    /// agent's words on a worker thread. One question at a time.
    pub fn ask_aside(
        &self,
        question: &str,
        done: impl FnOnce(Result<String, SessionError>) + Send + 'static,
    ) -> Result<(), SessionError> {
        if !self.capabilities().fork {
            return Err(SessionError::Agent("session/fork is not offered".to_string()));
        }
        let (connection, session) = self.ready()?;
        {
            // Taken before the fork is asked for, so a second question waits for this one to end.
            let mut state = self.shared.state();
            if state.side.is_some() {
                return Err(SessionError::Agent(
                    "a question beside the conversation is still open".to_string(),
                ));
            }
            state.side = Some(Side {
                session: None,
                text: String::new(),
            });
        }
        let shared = self.shared.clone();
        let question = question.to_string();
        let done: Answered = Box::new(done);
        let fork = acp::ForkSessionRequest::new(session, self.shared.launch.cwd.clone());
        ask(&connection, fork, Some(SETTING), move |result| match result {
            Ok(forked) => {
                if let Some(side) = shared.state().side.as_mut() {
                    side.session = Some(forked.session_id.clone());
                }
                let prompt = acp::PromptRequest::new(
                    forked.session_id.clone(),
                    vec![acp::ContentBlock::Text(acp::TextContent::new(question))],
                );
                let Some(connection) = shared.connection() else {
                    shared.state().side = None;
                    return done(Err(SessionError::Closed));
                };
                let close = shared.state().caps.close;
                let closing = connection.clone();
                ask(&connection, prompt, Some(ASIDE), move |result| {
                    let side = shared.state().side.take_if(|side| side.is(&forked.session_id));
                    if close {
                        ask(
                            &closing,
                            acp::CloseSessionRequest::new(forked.session_id),
                            Some(SETTING),
                            |_| {},
                        );
                    }
                    let text = side.map(|side| side.text).unwrap_or_default();
                    done(result.map(|_| text).map_err(|e| SessionError::Agent(e.message)));
                });
            }
            Err(error) => {
                shared.state().side = None;
                done(Err(SessionError::Agent(error.message)));
            }
        });
        Ok(())
    }
}

/// `servers` as `session/new` and `session/load` take them: the enabled ones in a transport the agent announced, a
/// program found on `env`'s PATH as the protocol wants it absolute.
pub(super) fn handover(
    servers: &[McpServerSpec],
    caps: &Capabilities,
    env: &[(String, String)],
    cwd: &Path,
) -> Vec<acp::McpServer> {
    servers
        .iter()
        .filter(|server| server.enabled)
        .filter_map(|server| match server.transport {
            McpTransport::Stdio => {
                let command = program(&server.command, env, cwd)?;
                let env = server
                    .env
                    .iter()
                    .map(|(name, value)| acp::EnvVariable::new(name.clone(), value.clone()))
                    .collect();
                Some(acp::McpServer::Stdio(
                    acp::McpServerStdio::new(server.name.clone(), command)
                        .args(server.args.clone())
                        .env(env),
                ))
            }
            McpTransport::Http if caps.mcp_http => Some(acp::McpServer::Http(
                acp::McpServerHttp::new(server.name.clone(), server.url.clone()).headers(headers(server)),
            )),
            McpTransport::Sse if caps.mcp_sse => Some(acp::McpServer::Sse(
                acp::McpServerSse::new(server.name.clone(), server.url.clone()).headers(headers(server)),
            )),
            McpTransport::Http | McpTransport::Sse => None,
        })
        .collect()
}

fn headers(server: &McpServerSpec) -> Vec<acp::HttpHeader> {
    server
        .headers
        .iter()
        .map(|(name, value)| acp::HttpHeader::new(name.clone(), value.clone()))
        .collect()
}

/// `command` as an absolute path: itself when it is one, against the project folder `cwd` when it names a folder
/// (`./bin/mcp`), as Claude Code reads `.mcp.json`, else the first match on `env`'s PATH.
fn program(command: &str, env: &[(String, String)], cwd: &Path) -> Option<PathBuf> {
    if command.is_empty() {
        return None;
    }
    if Path::new(command).is_absolute() {
        return Some(PathBuf::from(command));
    }
    if command.contains('/') {
        return Some(cwd.join(command)).filter(|path| path.is_file());
    }
    let path = env
        .iter()
        .find(|(k, _)| k == "PATH")
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    std::env::split_paths(path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::McpOrigin;

    fn server(name: &str, transport: McpTransport) -> McpServerSpec {
        McpServerSpec {
            name: name.into(),
            transport,
            command: "sh".into(),
            args: vec!["-c".into(), "true".into()],
            url: "https://x.example".into(),
            env: vec![("K".into(), "v".into())],
            headers: vec![("A".into(), "b".into())],
            origin: McpOrigin::User,
            enabled: true,
        }
    }

    #[test]
    fn only_enabled_servers_in_announced_transports_go_over() {
        let mut off = server("off", McpTransport::Stdio);
        off.enabled = false;
        let servers = [
            server("prog", McpTransport::Stdio),
            server("web", McpTransport::Http),
            server("events", McpTransport::Sse),
            off,
        ];
        let env = vec![("PATH".to_string(), "/usr/bin:/bin".to_string())];
        let caps = Capabilities {
            mcp_http: true,
            ..Capabilities::default()
        };
        let sent = handover(&servers, &caps, &env, Path::new("/"));
        assert_eq!(sent.len(), 2);
        let json = serde_json::to_value(&sent).unwrap();
        assert!(json[0]["command"].as_str().unwrap().ends_with("/sh"));
        assert_eq!(json[0]["env"][0]["name"], "K");
        assert_eq!(json[1]["type"], "http");
        assert_eq!(json[1]["headers"][0]["value"], "b");
    }

    #[test]
    fn a_program_named_by_a_folder_is_found_in_the_project() {
        let project = term_base::testing::scratch("acp-mcp-relative");
        std::fs::create_dir_all(project.join("scripts")).unwrap();
        std::fs::write(project.join("scripts/mcp"), "#!/bin/sh\n").unwrap();
        let env = vec![("PATH".to_string(), "/usr/bin:/bin".to_string())];
        assert_eq!(
            program("./scripts/mcp", &env, &project),
            Some(project.join("./scripts/mcp"))
        );
        assert_eq!(program("./scripts/missing", &env, &project), None);
        assert_eq!(program("sh", &env, &project), Some(PathBuf::from("/bin/sh")));
    }
}
