//! The sandbox an agent's own commands can run in (E19), off unless a thread turns it on. Each agent has its own
//! switch and its own reach, so the register keeps both per agent and the interface says what the switch does.

use serde_json::Value;

/// How an agent is told to sandbox its commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sandbox {
    /// The agent has no sandbox this app can switch on.
    Unavailable,
    /// Arguments added to the agent's command line.
    Args(&'static [&'static str]),
    /// Variables added to the agent's environment.
    Env(&'static [(&'static str, &'static str)]),
    /// JSON put into the `_meta` of `session/new`, as an adapter that takes its options there reads it.
    SessionMeta(&'static str),
}

/// What switching the sandbox on actually limits, as the interface explains it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// No sandbox for this agent.
    None,
    /// Shell commands are sandboxed; the agent's own file tools are not.
    ShellCommands,
    /// Commands and edits may write only inside the project; the network is off.
    WorkspaceWrite,
    /// Writes are limited to the project; reading anywhere and the network stay open.
    ProjectWritesOnly,
}

impl Sandbox {
    /// The arguments to add when the sandbox is on.
    pub(crate) fn args(&self) -> Vec<String> {
        match self {
            Sandbox::Args(args) => args.iter().map(|a| (*a).to_string()).collect(),
            _ => Vec::new(),
        }
    }

    /// The environment to add when the sandbox is on.
    pub(crate) fn env(&self) -> Vec<(String, String)> {
        match self {
            Sandbox::Env(env) => env.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())).collect(),
            _ => Vec::new(),
        }
    }

    /// The `_meta` of `session/new` when the sandbox is on.
    pub(crate) fn session_meta(&self) -> Option<Value> {
        match self {
            Sandbox::SessionMeta(json) => serde_json::from_str(json).ok(),
            _ => None,
        }
    }

    /// Whether this agent can be sandboxed at all.
    pub fn is_available(&self) -> bool {
        *self != Sandbox::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_kind_of_switch_lands_where_the_agent_reads_it() {
        assert_eq!(Sandbox::Args(&["--sandbox"]).args(), ["--sandbox"]);
        assert_eq!(
            Sandbox::Env(&[("MODE", "workspace-write")]).env(),
            [("MODE".to_string(), "workspace-write".to_string())]
        );
        let meta = Sandbox::SessionMeta(r#"{"a":{"b":true}}"#).session_meta().unwrap();
        assert_eq!(meta["a"]["b"], true);
        assert!(!Sandbox::Unavailable.is_available());
        assert!(Sandbox::Unavailable.args().is_empty() && Sandbox::Unavailable.session_meta().is_none());
    }
}
