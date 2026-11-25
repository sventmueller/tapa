//! A workspace as the UI crates reach it (E3, E20): a handle by the workspace's id, whose calls the app answers through
//! the `WorkspaceHost` it installs, so that no UI crate holds a window or a workspace.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView, NSWindow};

/// What a tab of a workspace shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabKind {
    /// A terminal session with its panes.
    Terminal,
    /// An agent session.
    Agent,
    /// A web page.
    Browser,
    /// A file of the workspace.
    File,
    /// The output of an agent's background task.
    Log,
}

impl TabKind {
    /// The SF Symbol its tabs show.
    pub fn symbol(self) -> &'static str {
        match self {
            TabKind::Terminal => "terminal",
            TabKind::Agent => "sparkle",
            TabKind::Browser => "globe",
            TabKind::File => "doc.text",
            TabKind::Log => "doc.plaintext",
        }
    }
}

/// The state of an agent tab, which its tab and the workspace's row in the sidebar show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentStatus {
    /// Nothing runs and nothing waits.
    #[default]
    Idle,
    /// A turn, a subagent or background work runs.
    Running,
    /// The agent waits for the user's answer, such as an approval.
    Waiting,
    /// The session could not start or ended with an error.
    Failed,
    /// A turn ended while the user looked elsewhere.
    Attention,
}

/// What a workspace is: whether it is open, which one is in front, its base path and its window.
pub trait IdentityHost {
    /// Whether the workspace is open and not closing.
    fn is_open(&self, workspace: WorkspaceId) -> bool;
    /// The workspace in front, where menu commands act.
    fn frontmost(&self) -> Option<WorkspaceId>;
    /// The AppKit window that shows the workspace, for sheets and alerts.
    fn ns_window(&self, workspace: WorkspaceId) -> Option<Retained<NSWindow>>;
    /// The workspace's base path.
    fn project_root(&self, workspace: WorkspaceId) -> Option<PathBuf>;
    /// What the workspace opened, which is its base path too.
    fn project(&self, workspace: WorkspaceId) -> Option<PathBuf> {
        self.project_root(workspace)
    }
}

/// The user's trust in a workspace, which the app keeps in its state (E21).
pub trait TrustHost {
    /// Whether the user trusts the workspace.
    fn is_trusted(&self, workspace: WorkspaceId) -> bool;
    /// Asks the user again whether to trust the workspace.
    fn ask_trust_again(&self, workspace: WorkspaceId);
}

/// The tabs in a workspace's tab bar (E20).
pub trait TabsHost {
    /// Adds `view` as a tab; its id, or `None` once the workspace closed.
    fn add_tab(
        &self,
        workspace: WorkspaceId,
        kind: TabKind,
        key: Option<String>,
        title: &str,
        view: Retained<NSView>,
    ) -> Option<TabId>;
    /// Brings the tab whose key is `key` to the front; whether there is one.
    fn select_tab_with_key(&self, workspace: WorkspaceId, key: &str) -> bool;
    /// Names `tab` with `title`.
    fn set_tab_title(&self, workspace: WorkspaceId, tab: TabId, title: &str);
    /// Brings `tab` to the front.
    fn select_tab(&self, workspace: WorkspaceId, tab: TabId);
    /// Closes `tab`.
    fn close_tab(&self, workspace: WorkspaceId, tab: TabId);
    /// The tab in front.
    fn selected_tab(&self, workspace: WorkspaceId) -> Option<TabId>;
    /// The tab whose key is `key`.
    fn tab_with_key(&self, workspace: WorkspaceId, key: &str) -> Option<TabId>;
    /// Knows `tab` by `key` from now on, as after its file was saved under another name.
    fn rekey_tab(&self, _workspace: WorkspaceId, _tab: TabId, _key: Option<String>) {}
}

/// How a tab shows the state and the colour of what it holds.
pub trait TabDecorHost {
    /// Shows `status` on `tab` and in the workspace's row of the sidebar.
    fn set_tab_status(&self, workspace: WorkspaceId, tab: TabId, status: AgentStatus);
    /// Tints the symbol of `tab`.
    fn set_tab_tint(&self, workspace: WorkspaceId, tab: TabId, color: &NSColor);
    /// Gives the symbol of `tab` the colours of its left, top, right and bottom point instead of a tint.
    fn set_tab_four_colours(&self, workspace: WorkspaceId, tab: TabId, colors: [Retained<NSColor>; 4]);
}

/// What a workspace tells the user beside its tabs.
pub trait NoticesHost {
    /// Shows `text` briefly over the workspace and has VoiceOver read it.
    fn show_notice(&self, workspace: WorkspaceId, text: &str);
    /// The sidebar reads the workspace's running work again.
    fn refresh_activity(&self, workspace: WorkspaceId);
}

/// The files a tab opens.
pub trait FilesHost {
    /// Opens `path` where the user reads files.
    fn open_file(&self, workspace: WorkspaceId, path: &Path);
}

/// What the app knows beside its workspaces.
pub trait AppEnvHost {
    /// The login shell's environment, waiting up to `wait` for it; any thread may ask.
    fn environment(&self, wait: std::time::Duration) -> Option<Vec<(String, String)>>;
}

/// What the UI crates ask of a workspace, answered by the app for the workspace with the given id.
pub trait WorkspaceHost:
    IdentityHost + TrustHost + TabsHost + TabDecorHost + NoticesHost + FilesHost + AppEnvHost
{
}

impl<T> WorkspaceHost for T where
    T: IdentityHost + TrustHost + TabsHost + TabDecorHost + NoticesHost + FilesHost + AppEnvHost
{
}

static HOST: OnceLock<&'static (dyn WorkspaceHost + Sync)> = OnceLock::new();

/// Makes `host` the app the workspaces belong to; once, before the first workspace opens.
pub fn install(host: &'static (dyn WorkspaceHost + Sync)) {
    let _ = HOST.set(host);
}

fn host() -> &'static (dyn WorkspaceHost + Sync) {
    *HOST
        .get()
        .expect("the app installs its workspace host before the first workspace opens")
}

/// The login shell's environment, waiting up to `wait` for it; empty when it did not come.
pub fn environment(wait: std::time::Duration) -> Vec<(String, String)> {
    host().environment(wait).unwrap_or_default()
}

/// The login shell's environment for an agent working in `folder`, waiting up to `wait` for it: in a remote workspace
/// with the stand-ins of the programs there as its `PATH` (E25).
pub fn environment_for(folder: &std::path::Path, wait: std::time::Duration) -> Vec<(String, String)> {
    term_remote::registry::environment(folder, environment(wait))
}

/// The login shell's environment, waiting up to `wait` for it; `None` while it is not known.
pub fn try_environment(wait: std::time::Duration) -> Option<Vec<(String, String)>> {
    host().environment(wait)
}

/// The number the app gives a workspace; never reused while the app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspaceId(u64);

impl WorkspaceId {
    /// The id the app handed out as `number`.
    pub const fn new(number: u64) -> WorkspaceId {
        WorkspaceId(number)
    }

    /// The number under which the app keeps the workspace.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A tab of a workspace, by the id the workspace gave it when it was added.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TabId(u64);

impl TabId {
    /// The tab the workspace numbered `number`.
    pub const fn new(number: u64) -> TabId {
        TabId(number)
    }

    /// The number under which the workspace keeps the tab.
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// A workspace by its id, which stays valid as a value after the workspace closed; the app then ignores its calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Workspace(WorkspaceId);

impl Workspace {
    /// The workspace with the id the app gave it.
    pub fn new(id: WorkspaceId) -> Workspace {
        Workspace(id)
    }

    /// The workspace in front, where menu commands act.
    pub fn frontmost() -> Option<Workspace> {
        host().frontmost().map(Workspace)
    }

    /// The workspace if it is still open.
    pub fn open(id: WorkspaceId) -> Option<Workspace> {
        host().is_open(id).then_some(Workspace(id))
    }

    /// The id the app gave the workspace.
    pub fn id(self) -> WorkspaceId {
        self.0
    }

    /// Whether the workspace is open and not closing.
    pub fn is_open(self) -> bool {
        host().is_open(self.0)
    }

    /// The workspace while it is open, as a weak reference loads its object.
    pub fn load(self) -> Option<Workspace> {
        self.is_open().then_some(self)
    }

    /// The main-thread marker, which every workspace call needs.
    pub fn mtm(self) -> MainThreadMarker {
        MainThreadMarker::new().expect("workspaces live on the main thread")
    }

    /// The AppKit window that shows the workspace, for sheets and alerts.
    pub fn window(self) -> Option<Retained<NSWindow>> {
        host().ns_window(self.0)
    }

    /// The workspace's base path.
    pub fn project_root(self) -> Option<PathBuf> {
        host().project_root(self.0)
    }

    /// What the workspace opened, which is its base path too.
    pub fn project(self) -> Option<PathBuf> {
        host().project(self.0)
    }
}

/// The user's trust in a workspace (E21).
pub trait Trust: AsWorkspace {
    /// Whether the user trusts the workspace.
    fn is_trusted(&self) -> bool {
        host().is_trusted(self.as_workspace().0)
    }

    /// Asks the user again whether to trust the workspace.
    fn ask_trust_again(&self) {
        host().ask_trust_again(self.as_workspace().0);
    }
}

impl Trust for Workspace {}

/// The tabs in a workspace's tab bar.
pub trait Tabs: AsWorkspace {
    /// Adds `view` as a tab; its id, or `None` once the workspace closed.
    fn add_tab(&self, kind: TabKind, key: Option<String>, title: &str, view: Retained<NSView>) -> Option<TabId> {
        host().add_tab(self.as_workspace().0, kind, key, title, view)
    }

    /// Brings the tab whose key is `key` to the front; whether there is one.
    fn select_tab(&self, key: &str) -> bool {
        host().select_tab_with_key(self.as_workspace().0, key)
    }

    /// Names `tab` with `title`.
    fn set_tab_title(&self, tab: TabId, title: &str) {
        host().set_tab_title(self.as_workspace().0, tab, title);
    }

    /// Brings `tab` to the front.
    fn select_tab_by_id(&self, tab: TabId) {
        host().select_tab(self.as_workspace().0, tab);
    }

    /// Closes `tab`.
    fn close_tab(&self, tab: TabId) {
        host().close_tab(self.as_workspace().0, tab);
    }

    /// The tab in front.
    fn selected_tab(&self) -> Option<TabId> {
        host().selected_tab(self.as_workspace().0)
    }

    /// The tab whose key is `key`.
    fn tab_with_key(&self, key: &str) -> Option<TabId> {
        host().tab_with_key(self.as_workspace().0, key)
    }

    /// Knows `tab` by `key` from now on, as after its file was saved under another name.
    fn rekey_tab(&self, tab: TabId, key: Option<String>) {
        host().rekey_tab(self.as_workspace().0, tab, key);
    }
}

impl Tabs for Workspace {}

/// How a tab shows the state and the colour of what it holds.
pub trait TabDecor: AsWorkspace {
    /// Shows `status` on `tab` and in the workspace's row of the sidebar.
    fn set_tab_status(&self, tab: TabId, status: AgentStatus) {
        host().set_tab_status(self.as_workspace().0, tab, status);
    }

    /// Tints the symbol of `tab`.
    fn set_tab_tint(&self, tab: TabId, color: &NSColor) {
        host().set_tab_tint(self.as_workspace().0, tab, color);
    }

    /// Gives the symbol of `tab` the colours of its left, top, right and bottom point instead of a tint.
    fn set_tab_four_colours(&self, tab: TabId, colors: [Retained<NSColor>; 4]) {
        host().set_tab_four_colours(self.as_workspace().0, tab, colors);
    }
}

impl TabDecor for Workspace {}

/// What a workspace tells the user beside its tabs.
pub trait Notices: AsWorkspace {
    /// Shows `text` briefly over the workspace and has VoiceOver read it.
    fn show_notice(&self, text: &str) {
        host().show_notice(self.as_workspace().0, text);
    }

    /// The sidebar reads the workspace's running work again.
    fn refresh_activity(&self) {
        host().refresh_activity(self.as_workspace().0);
    }
}

impl Notices for Workspace {}

/// The files a tab opens.
pub trait Files: AsWorkspace {
    /// Opens `path` where the user reads files.
    fn open_file(&self, path: &Path) {
        host().open_file(self.as_workspace().0, path);
    }
}

impl Files for Workspace {}

/// What names a workspace.
pub trait AsWorkspace {
    /// The workspace it names.
    fn as_workspace(&self) -> Workspace;
}

impl AsWorkspace for Workspace {
    fn as_workspace(&self) -> Workspace {
        *self
    }
}

impl<T: AsWorkspace + ?Sized> AsWorkspace for Retained<T> {
    fn as_workspace(&self) -> Workspace {
        (**self).as_workspace()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Workspace 1 is open at `/work` and trusted, every other one closed; it keeps the calls it got.
    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
    }

    impl Fake {
        fn heard(&self, call: String) {
            self.calls.lock().unwrap().push(call);
        }
    }

    impl IdentityHost for Fake {
        fn is_open(&self, workspace: WorkspaceId) -> bool {
            workspace.get() == 1
        }
        fn frontmost(&self) -> Option<WorkspaceId> {
            Some(WorkspaceId::new(1))
        }
        fn ns_window(&self, _: WorkspaceId) -> Option<Retained<NSWindow>> {
            None
        }
        fn project_root(&self, workspace: WorkspaceId) -> Option<PathBuf> {
            self.is_open(workspace).then(|| PathBuf::from("/work"))
        }
    }

    impl TrustHost for Fake {
        fn is_trusted(&self, workspace: WorkspaceId) -> bool {
            workspace.get() == 1
        }
        fn ask_trust_again(&self, workspace: WorkspaceId) {
            self.heard(format!("ask {}", workspace.get()));
        }
    }

    impl TabsHost for Fake {
        fn add_tab(
            &self,
            _: WorkspaceId,
            _: TabKind,
            _: Option<String>,
            _: &str,
            _: Retained<NSView>,
        ) -> Option<TabId> {
            None
        }
        fn select_tab_with_key(&self, workspace: WorkspaceId, key: &str) -> bool {
            self.heard(format!("select {} {key}", workspace.get()));
            true
        }
        fn set_tab_title(&self, workspace: WorkspaceId, tab: TabId, title: &str) {
            self.heard(format!("title {} {} {title}", workspace.get(), tab.get()));
        }
        fn select_tab(&self, _: WorkspaceId, _: TabId) {}
        fn close_tab(&self, _: WorkspaceId, _: TabId) {}
        fn selected_tab(&self, _: WorkspaceId) -> Option<TabId> {
            None
        }
        fn tab_with_key(&self, _: WorkspaceId, _: &str) -> Option<TabId> {
            None
        }
    }

    impl TabDecorHost for Fake {
        fn set_tab_status(&self, workspace: WorkspaceId, tab: TabId, status: AgentStatus) {
            self.heard(format!("status {} {} {status:?}", workspace.get(), tab.get()));
        }
        fn set_tab_tint(&self, _: WorkspaceId, _: TabId, _: &NSColor) {}
        fn set_tab_four_colours(&self, _: WorkspaceId, _: TabId, _: [Retained<NSColor>; 4]) {}
    }

    impl NoticesHost for Fake {
        fn show_notice(&self, _: WorkspaceId, _: &str) {}
        fn refresh_activity(&self, _: WorkspaceId) {}
    }

    impl FilesHost for Fake {
        fn open_file(&self, _: WorkspaceId, _: &Path) {}
    }

    impl AppEnvHost for Fake {
        fn environment(&self, _: std::time::Duration) -> Option<Vec<(String, String)>> {
            None
        }
    }

    fn fake() -> &'static Fake {
        static FAKE: OnceLock<Fake> = OnceLock::new();
        let fake = FAKE.get_or_init(Fake::default);
        install(fake);
        fake
    }

    #[test]
    fn a_closed_workspace_stays_a_value_but_loads_as_none() {
        fake();
        let closed = Workspace::new(WorkspaceId::new(2));
        assert!(closed.load().is_none());
        assert!(Workspace::open(closed.id()).is_none());
        let open = Workspace::frontmost().unwrap();
        assert_eq!(open.load(), Some(open));
        assert_eq!(
            open.project(),
            Some(PathBuf::from("/work")),
            "what it opened is its base path"
        );
        assert!(open.is_trusted() && !closed.is_trusted());
    }

    #[test]
    fn calls_reach_the_host_with_the_workspace_they_name() {
        let fake = fake();
        let open = Workspace::new(WorkspaceId::new(1));
        assert!(open.select_tab("agent:3"));
        open.set_tab_title(TabId::new(4), "Fix it");
        open.set_tab_status(TabId::new(4), AgentStatus::Waiting);
        open.ask_trust_again();
        let calls = fake.calls.lock().unwrap().clone();
        for call in ["select 1 agent:3", "title 1 4 Fix it", "status 1 4 Waiting", "ask 1"] {
            assert!(calls.iter().any(|c| c == call), "{call} in {calls:?}");
        }
        assert!(
            environment(std::time::Duration::ZERO).is_empty(),
            "no environment yet reads as none"
        );
    }
}
