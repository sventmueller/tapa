//! Remote workspaces (E25): a workspace whose terminals and agents run on another machine over the system's OpenSSH.
//! Each has a placeholder folder on this Mac, which its tabs and agents start in, and beside it a folder of stand-ins
//! for the agent programs installed there; one master connection carries everything. Files and Git stay away.

mod ask;
pub(crate) mod sheet;

#[cfg(feature = "selftest")]
pub(crate) use ask::answer_shown;

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};
use std::sync::Arc;

use objc2::MainThreadOnly;

use splat_i18n::{t, tf};
use term_remote::{Connection, State, Target};
use term_session::Program;
use term_ui::kit::alert::{Alert, Role};

use crate::state::try_app;

/// The programs a remote workspace looks for there and stands in for: Claude, Codex, OpenCode and the ACP agents the
/// app knows. OpenCode runs over ACP there, as its own server would need a port forwarded and a password (O56).
const AGENTS: [&str; 7] = [
    "claude",
    "codex",
    "codex-acp",
    "opencode",
    "gemini",
    "agy",
    "claude-agent-acp",
];

thread_local! {
    static REMOTES: RefCell<Vec<Weak<Remote>>> = const { RefCell::new(Vec::new()) };
}

thread_local! {
    static LATER: RefCell<Vec<Box<dyn FnOnce()>>> = const { RefCell::new(Vec::new()) };
}

/// Runs `job` on the main thread a moment later, such as a sheet once the one before has gone.
pub(crate) fn later(job: impl FnOnce() + 'static) {
    LATER.with(|later| later.borrow_mut().push(Box::new(job)));
    term_base::main_soon(|| {
        for job in LATER.with(|later| std::mem::take(&mut *later.borrow_mut())) {
            job();
        }
    });
}

/// The app's end of ssh's questions, open as long as the app runs.
static LISTENER: std::sync::Mutex<Option<term_remote::askpass::Listener>> = std::sync::Mutex::new(None);

/// Starts listening for ssh's questions; once, at the start of the app.
pub(crate) fn start() {
    let path = term_base::cache_dir().join("askpass");
    match term_remote::askpass::Listener::start(path, |question| {
        term_base::main_soon(move || ask::answer(question));
    }) {
        Ok(listener) => {
            if let Ok(mut held) = LISTENER.lock() {
                *held = Some(listener);
            }
        }
        Err(error) => term_base::diag::report("listening for ssh's questions", error),
    }
}

/// A remote workspace's machine, its connection and its folders on this Mac.
pub(crate) struct Remote {
    target: RefCell<Target>,
    connection: RefCell<Arc<Connection>>,
    token: String,
    /// The placeholder folder the workspace's tabs and agents start in on this Mac.
    pub(crate) base: PathBuf,
    /// The stand-ins of the agent programs installed there.
    bin: PathBuf,
    state: RefCell<State>,
    /// Whether the password from the keychain went to ssh in this opening already, so a wrong one is asked for.
    pub(super) tried_password: Cell<bool>,
}

impl Remote {
    /// A new remote workspace's machine `target`, with folders of its own below the app's cache.
    pub(crate) fn new(target: Target) -> Rc<Remote> {
        let key = term_remote::askpass::token();
        let base = term_base::cache_dir()
            .join("remote")
            .join(&key[..16])
            .join(target.folder_name());
        Remote::at(target, base)
    }

    /// The machine `target` of a stored workspace whose placeholder folder is `base`.
    pub(crate) fn at(target: Target, base: PathBuf) -> Rc<Remote> {
        let dir = base.parent().map_or_else(|| base.clone(), Path::to_path_buf);
        let bin = dir.join("bin");
        if let Err(error) = std::fs::create_dir_all(&base) {
            term_base::diag::report("making a remote workspace's folder", error);
        }
        term_remote::registry::set(&base, Some(&bin));
        let token = term_remote::askpass::token();
        let connection = connection(&target, &dir, &token);
        let remote = Rc::new(Remote {
            target: RefCell::new(target),
            connection: RefCell::new(connection),
            token,
            base,
            bin,
            state: RefCell::new(State::Closed),
            tried_password: Cell::new(false),
        });
        REMOTES.with(|remotes| {
            let mut remotes = remotes.borrow_mut();
            remotes.retain(|remote| remote.strong_count() > 0);
            remotes.push(Rc::downgrade(&remote));
        });
        remote
    }

    /// The machine and folder.
    pub(crate) fn target(&self) -> Target {
        self.target.borrow().clone()
    }

    /// Opens the connection unless it is open or opening.
    pub(crate) fn connect(&self) {
        if matches!(self.connection.borrow().state(), State::Connecting | State::Connected) {
            return;
        }
        self.tried_password.set(false);
        let token = self.token.clone();
        self.connection.borrow().open(move |state| {
            let (token, state) = (token.clone(), state.clone());
            term_base::main_soon(move || changed(&token, state));
        });
        self.show(State::Connecting);
    }

    /// Closes the connection, which ends the workspace's terminals and agents.
    pub(crate) fn disconnect(&self) {
        let connection = self.connection.borrow().clone();
        term_base::thread::detach_or_run("closing a remote connection", move || connection.close());
        self.show(State::Closed);
    }

    /// Points the workspace at `target` from now on, with a new connection opened.
    pub(crate) fn retarget(&self, target: Target) {
        self.disconnect();
        let dir = self.base.parent().map_or_else(|| self.base.clone(), Path::to_path_buf);
        *self.connection.borrow_mut() = connection(&target, &dir, &self.token);
        *self.target.borrow_mut() = target;
        self.connect();
    }

    /// Forgets the workspace: its connection closes and its folders on this Mac go.
    pub(crate) fn forget(&self) {
        self.disconnect();
        term_remote::registry::set(&self.base, None);
        if let Some(dir) = self.base.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// The row's view of it in the sidebar.
    pub(crate) fn sidebar(&self) -> term_ui_sidebar::Remote {
        let target = self.target.borrow();
        term_ui_sidebar::Remote {
            account: target.account(),
            folder: target.folder.clone(),
            connection: match &*self.state.borrow() {
                State::Connecting => term_ui_sidebar::Connection::Connecting,
                State::Connected => term_ui_sidebar::Connection::Connected,
                State::Closed | State::Disconnected(_) => term_ui_sidebar::Connection::Disconnected,
                State::Failed(_) => term_ui_sidebar::Connection::Failed,
            },
        }
    }

    /// The program of a terminal of the workspace: a local shell that waits for the connection, then ssh with the
    /// shell integration into the folder there.
    pub(crate) fn terminal_program(&self) -> Program {
        let target = self.target.borrow();
        let command = term_session::integration::remote_shell(&target.folder);
        let waiting = tf("Verbinde mit {} …", &[&target.account()]);
        let (program, args) = self.connection.borrow().terminal_program(&command, &waiting);
        Program::Command { program, args }
    }

    /// The programs it stands in for there, sorted, for the self-test.
    #[cfg(feature = "selftest")]
    pub(crate) fn stand_ins(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(&self.bin)
            .map(|entries| {
                entries
                    .flatten()
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| !name.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// Starts the workspace's terminals again whose shell ended with the connection, and has its agents continue.
    fn resume(self: &Rc<Self>) {
        let Some(app) = try_app() else { return };
        let windows = app.windows.borrow().clone();
        for window in windows {
            for workspace in window.workspaces() {
                if !workspace.remote_machine().is_some_and(|own| Rc::ptr_eq(&own, self)) {
                    continue;
                }
                for tab in workspace.tabs() {
                    let Some(terminal) = tab.terminal() else { continue };
                    for id in terminal.panes.sessions() {
                        if let Some(view) = term_ui::find(id).filter(|view| view.has_exited()) {
                            crate::window::session::restart(&view);
                        }
                    }
                }
            }
        }
        term_ui_agent::resume_in(&self.base);
    }

    fn show(&self, state: State) {
        *self.state.borrow_mut() = state;
        refresh_windows();
    }
}

/// The connection to `target` with its socket in `dir`, its questions asked for `token`.
fn connection(target: &Target, dir: &Path, token: &str) -> Arc<Connection> {
    let socket = term_base::cache_dir().join("askpass");
    let app = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tapa"));
    let askpass = term_remote::askpass::environment(&app, &socket, token);
    let options = test_options();
    Connection::with_options(target.clone(), dir.join("c"), askpass, options)
}

/// Options of the self-test's own ssh configuration, from `TAPA_REMOTE_SSH_OPTIONS`, separated by spaces.
fn test_options() -> Vec<String> {
    #[cfg(feature = "selftest")]
    if let Some(options) = std::env::var_os("TAPA_REMOTE_SSH_OPTIONS") {
        return options
            .to_string_lossy()
            .split_whitespace()
            .map(str::to_string)
            .collect();
    }
    Vec::new()
}

/// The remote workspace whose questions come with `token`.
pub(super) fn by_token(token: &str) -> Option<Rc<Remote>> {
    REMOTES.with(|remotes| {
        remotes
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|remote| remote.token == token)
    })
}

/// The remote workspace `folder` belongs to, if it belongs to one.
fn of_folder(folder: &Path) -> Option<Rc<Remote>> {
    REMOTES.with(|remotes| {
        remotes
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|remote| folder.starts_with(&remote.base))
    })
}

/// The program of a new terminal in `folder` where it is a remote workspace's.
pub(crate) fn terminal_program_for(folder: &Path) -> Option<Program> {
    of_folder(folder).map(|remote| remote.terminal_program())
}

/// Whether `folder` is a remote workspace's, whose files are not on this Mac.
pub(crate) fn is_remote(folder: &Path) -> bool {
    of_folder(folder).is_some()
}

/// The connection of the workspace with `token` is now `state`.
fn changed(token: &str, state: State) {
    let Some(remote) = by_token(token) else { return };
    remote.show(state.clone());
    match state {
        State::Connected => find_agents(&remote),
        State::Failed(message) => report(
            &remote,
            &tf("Verbindung zu {} fehlgeschlagen", &[&remote.target().account()]),
            &message,
        ),
        State::Disconnected(message) => report(
            &remote,
            &tf("Verbindung zu {} getrennt", &[&remote.target().account()]),
            &message,
        ),
        State::Closed | State::Connecting => {}
    }
}

/// Looks for the agent programs there and writes their stand-ins, so that the agent menu offers them.
fn find_agents(remote: &Rc<Remote>) {
    let connection = remote.connection.borrow().clone();
    let bin = remote.bin.clone();
    let token = remote.token.clone();
    let app = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("tapa"));
    term_base::thread::detach_or_run("finding agents there", move || {
        let installed = connection.installed(&AGENTS);
        if let Err(error) = term_remote::stand_in::write(&bin, &app, &connection.forward(), &installed) {
            term_base::diag::report("writing a remote workspace's stand-ins", error);
        }
        term_base::main_soon(move || {
            refresh_windows();
            if let Some(remote) = by_token(&token) {
                remote.resume();
            }
        });
    });
}

/// Tells about the end of the connection in the window of the workspace, offering to connect again.
fn report(remote: &Rc<Remote>, message: &str, detail: &str) {
    let Some(window) = ns_window_of(remote) else { return };
    let weak = Rc::downgrade(remote);
    Alert::new(window.mtm(), message)
        .warning()
        .informative(detail)
        .button(t("Neu verbinden"), Role::Plain)
        .button(t("Schließen"), Role::CancelOnEscape)
        .sheet(&window, move |response| {
            if term_ui::kit::alert::first(response)
                && let Some(remote) = weak.upgrade()
            {
                remote.connect();
            }
        });
}

/// The window that shows `remote`'s workspace, else the one in front.
pub(super) fn ns_window_of(remote: &Rc<Remote>) -> Option<objc2::rc::Retained<objc2_app_kit::NSWindow>> {
    let app = try_app()?;
    let windows = app.windows.borrow().clone();
    let owner = windows.iter().find(|window| {
        window
            .workspaces()
            .iter()
            .any(|workspace| workspace.remote_machine().is_some_and(|own| Rc::ptr_eq(&own, remote)))
    });
    owner.cloned().or_else(|| app.front())?.front_ns_window()
}

fn refresh_windows() {
    if let Some(app) = try_app() {
        let windows = app.windows.borrow().clone();
        for window in windows {
            window.refresh();
        }
    }
}

/// The keychain account of the password for `target`.
pub(super) fn account(target: &Target) -> String {
    format!("ssh:{}", target.account())
}

/// Where passwords are kept: the keychain, in the self-test memory.
pub(super) fn secrets() -> &'static dyn term_seal::secrets::SecretStore {
    #[cfg(feature = "selftest")]
    {
        static MEMORY: std::sync::OnceLock<term_seal::secrets::Memory> = std::sync::OnceLock::new();
        MEMORY.get_or_init(term_seal::secrets::Memory::default)
    }
    #[cfg(not(feature = "selftest"))]
    {
        &term_seal::secrets::Keychain
    }
}

/// Keeps `password` for `target` in the keychain; an empty one forgets it.
pub(crate) fn keep_password(target: &Target, password: &str) {
    let account = account(target);
    let kept = if password.is_empty() {
        secrets().delete(&account)
    } else {
        secrets().set(&account, password)
    };
    if let Err(error) = kept {
        term_base::diag::report("keeping an ssh password", format!("{error:?}"));
    }
}
