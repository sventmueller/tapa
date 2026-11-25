//! What the self-test reads from and does with the parts of `acp.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

/// The launch every ACP tab gets in the self-test; process-wide, since agents are looked up on other threads.
pub(super) static TEST_LAUNCH: std::sync::Mutex<Option<Option<Launch>>> = std::sync::Mutex::new(None);

/// Starts every ACP tab with `launch`, as the self-test runs its test agent; with `None` no agent is found at all,
/// so the self-test never starts an agent installed on the Mac.
pub fn set_test_launch(launch: Option<Launch>) {
    *TEST_LAUNCH.lock_ignoring_poison() = Some(launch);
}

/// The registry the self-test stands in for the one fetched from the network.
static TEST_REGISTER: std::sync::Mutex<Option<term_acp::Register>> = std::sync::Mutex::new(None);
/// The folder of the agents the self-test installs, never the app's own.
static TEST_INSTALLS: std::sync::Mutex<Option<std::path::PathBuf>> = std::sync::Mutex::new(None);
/// The files the self-test's stand-in network serves, by address; any other address fails.
static TEST_FILES: std::sync::Mutex<Vec<(String, Vec<u8>)>> = std::sync::Mutex::new(Vec::new());

/// Has the registry read as `register`, as if the user had fetched it.
pub fn set_test_register(register: term_acp::Register) {
    *TEST_REGISTER.lock_ignoring_poison() = Some(register);
}

/// Has the app install agents into `root` and serve `files` instead of the network, which no self-test reaches.
pub fn set_test_installs(root: std::path::PathBuf, files: Vec<(String, Vec<u8>)>) {
    *TEST_INSTALLS.lock_ignoring_poison() = Some(root);
    *TEST_FILES.lock_ignoring_poison() = files;
}

pub(crate) fn test_register() -> Option<term_acp::Register> {
    TEST_REGISTER.lock_ignoring_poison().clone()
}

pub(crate) fn test_installs() -> Option<std::path::PathBuf> {
    TEST_INSTALLS.lock_ignoring_poison().clone()
}

/// The stand-in network: the files `set_test_installs` gave, nothing else.
struct TestNet;

impl term_net::Net for TestNet {
    fn get(&self, url: &str, accept: &str) -> std::io::Result<term_net::Download> {
        let files = TEST_FILES.lock_ignoring_poison();
        match files.iter().find(|(own, _)| own == url) {
            Some((_, bytes)) => Ok(term_net::Download {
                length: Some(bytes.len() as u64),
                body: Box::new(std::io::Cursor::new(bytes.clone())),
            }),
            None => term_net::Offline.get(url, accept),
        }
    }
}

pub(crate) fn test_net() -> std::sync::Arc<dyn term_net::Net> {
    std::sync::Arc::new(TestNet)
}

impl AgentTab {
    /// What the ACP agent offers, as the tab last learned it.
    pub fn acp_capabilities(&self) -> Option<term_acp::Capabilities> {
        self.state.borrow().acp_caps().cloned()
    }

    /// The titles of the model and mode pills and the sandbox switch's explanation.
    pub fn pills(&self) -> (String, String, String) {
        (
            self.composer.model.title().to_string(),
            self.composer.mode.title().to_string(),
            self.composer
                .sandbox
                .toolTip()
                .map(|t| t.to_string())
                .unwrap_or_default(),
        )
    }

    /// What keeps the session from running, by name.
    pub fn problem_name(&self) -> Option<&'static str> {
        self.state.borrow().problem().map(|p| match p {
            Problem::Untrusted(_) => "untrusted",
            Problem::NotFound => "not found",
            Problem::SignIn(_) | Problem::AcpSignIn => "sign in",
            Problem::Start(_) => "start",
            Problem::Missing(_) => "missing",
            Problem::Installing(_) => "installing",
            Problem::Exited(_) => "exited",
        })
    }

    /// Sets the agent's option `option` to `value`, as picking it in the model menu does.
    pub fn choose_option(&self, option: &str, value: &str) {
        self.acp_set_option(option, value);
    }

    /// Switches the agent to mode `id`, as picking it in the mode menu does.
    pub fn choose_mode(&self, id: &str) {
        self.acp_set_mode(id.to_string());
    }

    /// The agent's name, as the tab shows it.
    pub fn agent_name(&self) -> String {
        self.agent.name().to_string()
    }
}
