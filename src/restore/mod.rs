//! Restoring the windows after quitting and starting again (F71): their frames, workspaces (E20), tabs and panes, the
//! windows dragged out of a workspace's tab group and the sidebar, each
//! session's folder and title and, where the settings say so, the last lines of its history. Only shells start again, in the stored
//! folders, agent tabs, which resume their sessions (E21), and browser tabs at their pages (E22); the programs that ran in the shells do not (R7, F149 is
//! after version 1). The state is sealed on disk (E17) in
//! the data folder of the product's identity and read once at start.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

use serde::{Deserialize, Serialize};
use term_base::{IgnoringPoison, LockIgnoringPoison};
use term_panes::layout::Layout;

use crate::state::{app, try_app};
use crate::window::Window;

/// The layout of the stored file; the one before workspaces is taken over, another one is not read.
const VERSION: u32 = 2;
/// The most bytes of history kept per session, however short its lines.
const MAX_SESSION_BYTES: usize = 512 * 1024;
/// The most bytes of history kept for all sessions together.
const MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;
const FILE: &str = "restore.sealed";

/// What is stored of all windows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Saved {
    pub(crate) version: u32,
    pub(crate) windows: Vec<SavedWindow>,
}

/// One window: where it was and its workspaces.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedWindow {
    /// Origin and size in screen points.
    pub(crate) frame: Option<[f64; 4]>,
    pub(crate) workspaces: Vec<SavedWorkspace>,
    /// The index of the workspace selected.
    #[serde(default)]
    pub(crate) selected: Option<usize>,
    /// Whether the user had hidden the sidebar.
    #[serde(default)]
    pub(crate) sidebar_hidden: bool,
    /// How wide the sidebar was, in points.
    #[serde(default)]
    pub(crate) sidebar_width: Option<f64>,
    /// Whether it was stored without its Terminal; before that, its first workspace in the settings' folder was what
    /// the Terminal now is.
    #[serde(default)]
    pub(crate) without_scratch: bool,
}

/// One workspace: its folder, the name the user gave it, and its tabs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedWorkspace {
    pub(crate) base: PathBuf,
    #[serde(default)]
    pub(crate) name: Option<String>,
    pub(crate) layout: Layout<SavedSession>,
    /// Whether the user trusted it (E21).
    #[serde(default)]
    pub(crate) trusted: bool,
    /// Its agent tabs, which resume their sessions (E21).
    #[serde(default)]
    pub(crate) agents: Vec<SavedAgent>,
    /// Its browser tabs, which load their pages again once they first show (E22).
    #[serde(default)]
    pub(crate) browsers: Vec<SavedBrowser>,
    /// Its editor tabs, which read their files again (E22).
    #[serde(default)]
    pub(crate) files: Vec<SavedFile>,
    /// The file list and the Git panel in its sidebar entry, open or not, with their heights (E22).
    #[serde(default)]
    pub(crate) panels: crate::panels::Panels,
    /// The windows the user dragged out of its tab group, which come back beside it.
    #[serde(default)]
    pub(crate) separate: Vec<SavedGroup>,
}

/// A tab group of a workspace's windows apart from its main one: the places of its tabs among the workspace's tabs
/// stored, terminals and agents together, and its frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedGroup {
    pub(crate) tabs: Vec<usize>,
    pub(crate) frame: [f64; 4],
}

/// An agent tab: where it stood among the workspace's tabs and the agent's id of its session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedAgent {
    pub(crate) at: usize,
    pub(crate) session: String,
    #[serde(default)]
    pub(crate) selected: bool,
}

/// A browser tab: where it stood among the workspace's tabs and the address of its page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedBrowser {
    pub(crate) at: usize,
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) selected: bool,
}

/// An editor tab: where it stood among the workspace's tabs and its file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedFile {
    pub(crate) at: usize,
    pub(crate) path: PathBuf,
    #[serde(default)]
    pub(crate) selected: bool,
}

/// A window as the layout before workspaces stored it: its tabs only.
#[derive(Debug, Deserialize)]
struct SavedWindowV1 {
    frame: Option<[f64; 4]>,
    layout: Layout<SavedSession>,
}

impl SavedWindowV1 {
    /// The window with its tabs in one workspace, whose folder is that of the active pane of the first tab.
    fn migrated(self) -> SavedWindow {
        let base = self
            .layout
            .tabs
            .first()
            .and_then(|tab| {
                let sessions: Vec<&SavedSession> =
                    tab.root.panes().into_iter().flat_map(|pane| &pane.documents).collect();
                sessions
                    .iter()
                    .find(|session| session.focused)
                    .or(sessions.first())
                    .map(|session| session.folder.clone())
            })
            .unwrap_or_else(term_base::home);
        SavedWindow {
            frame: self.frame,
            workspaces: vec![SavedWorkspace {
                base,
                name: None,
                layout: self.layout,
                trusted: false,
                agents: Vec::new(),
                browsers: Vec::new(),
                files: Vec::new(),
                panels: crate::panels::Panels::default(),
                separate: Vec::new(),
            }],
            selected: Some(0),
            sidebar_hidden: false,
            sidebar_width: None,
            without_scratch: false,
        }
    }
}

/// One session of a pane.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct SavedSession {
    pub(crate) folder: PathBuf,
    #[serde(default)]
    pub(crate) title: Option<String>,
    /// Whether its pane was the active one of its tab.
    #[serde(default)]
    pub(crate) focused: bool,
    /// The last rows of its history and screen, oldest first.
    #[serde(default)]
    pub(crate) rows: Vec<String>,
    /// Whether its input field was on (F209); none from before that was stored, which follows the settings.
    #[serde(default)]
    pub(crate) field: Option<bool>,
}

/// How many bytes of history the sessions still stored may take.
pub(crate) struct Budget(usize);

impl Budget {
    pub(crate) fn new() -> Budget {
        Budget(MAX_TOTAL_BYTES)
    }

    /// The remaining allowance for one session.
    pub(crate) fn available(&self) -> usize {
        self.0.min(MAX_SESSION_BYTES)
    }

    /// Accounts for the bounded tail copied for one session.
    pub(crate) fn spent(&mut self, rows: &[String]) {
        self.0 -= rows.iter().map(|row| row.len() + 1).sum::<usize>();
    }
}

thread_local! {
    /// The stored rows of restored sessions until their shells start, by session view.
    static ROWS: RefCell<HashMap<u64, Vec<String>>> = RefCell::new(HashMap::new());
}

/// Keeps `rows` for the session view `view` until its shell starts.
pub(crate) fn put_rows(view: u64, rows: Vec<String>) {
    if !rows.is_empty() {
        ROWS.with(|cell| cell.borrow_mut().insert(view, rows));
    }
}

/// The stored rows of session view `view`, once; none for a new session.
pub(crate) fn take_rows(view: u64) -> Vec<String> {
    ROWS.with(|cell| cell.borrow_mut().remove(&view)).unwrap_or_default()
}

/// Where the state is kept: `TAPA_RESTORE_DIR` if set, the self-test's folder in a self-test, else the data folder.
pub(crate) fn folder() -> PathBuf {
    if let Some(dir) = std::env::var_os("TAPA_RESTORE_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(feature = "selftest")]
    if std::env::var_os("TAPA_SELFTEST").is_some()
        && let Some(dir) = std::env::var_os("TAPA_SELFTEST_DIR")
    {
        return PathBuf::from(dir);
    }
    term_base::data_dir()
}

/// The key; a self-test keeps its own beside the state, so it never asks for the keychain.
fn key(folder: &Path) -> Result<[u8; term_seal::KEY_LEN], term_seal::SealError> {
    #[cfg(feature = "selftest")]
    if std::env::var_os("TAPA_SELFTEST").is_some() {
        let path = folder.join("restore.key");
        if let Some(key) = std::fs::read(&path).ok().and_then(|bytes| bytes.try_into().ok()) {
            return Ok(key);
        }
        let key = term_seal::new_key()?;
        let _ = term_base::atomic_write::write(&path, &key);
        return Ok(key);
    }
    let _ = folder;
    term_seal::key()
}

/// The key that seals what the app keeps on disk (E17), the restore's and the editors' copies of unsaved text alike.
pub(crate) fn seal_key() -> Result<[u8; term_seal::KEY_LEN], term_seal::SealError> {
    key(&folder())
}

/// Stores every window but the quick terminal's, as the app quits; removes what was stored when the settings turn
/// restoring off. The size new windows take is kept either way.
pub(crate) fn save() {
    if crate::quit::left() {
        return;
    }
    let Some(app) = try_app() else { return };
    crate::window::store_last_window();
    let config = app.config();
    let folder = folder();
    if !config.restore.enabled {
        queue_save(Save { folder, saved: None });
        return;
    }
    let lines = config.restore.lines();
    let mut budget = Budget::new();
    let windows: Vec<SavedWindow> = app
        .windows
        .borrow()
        .iter()
        .filter(|window| !window.is_quick())
        .filter_map(|window| window.snapshot(lines, &mut budget))
        .collect();
    let saved = Saved {
        version: VERSION,
        windows,
    };
    queue_save(Save {
        folder,
        saved: Some(saved),
    });
}

/// The final snapshot supersedes queued ones and must reach disk before the app can exit.
pub(crate) fn save_at_quit() {
    save();
    let state = SAVE.0.lock_ignoring_poison();
    drop(SAVE.1.wait_while(state, |state| state.running).ignoring_poison());
}

struct Save {
    folder: PathBuf,
    saved: Option<Saved>,
}

struct Saves {
    pending: Option<Save>,
    running: bool,
}

static SAVE: (Mutex<Saves>, Condvar) = (
    Mutex::new(Saves {
        pending: None,
        running: false,
    }),
    Condvar::new(),
);

fn queue_save(save: Save) {
    let start = {
        let mut state = SAVE.0.lock_ignoring_poison();
        state.pending = Some(save);
        !std::mem::replace(&mut state.running, true)
    };
    if start
        && let Err(unstarted) = term_base::thread::detach("tapa-restore", || {
            term_base::priority::utility();
            save_pending();
        })
    {
        term_base::diag::report("starting the restore writer", &unstarted.error);
        save_pending();
    }
}

fn save_pending() {
    loop {
        let job = {
            let mut state = SAVE.0.lock_ignoring_poison();
            let Some(job) = state.pending.take() else {
                state.running = false;
                SAVE.1.notify_all();
                return;
            };
            job
        };
        let path = job.folder.join(FILE);
        if let Some(saved) = job.saved {
            if let Err(error) = write(&job.folder, &path, &saved) {
                term_base::diag::report("storing the windows", &error);
            }
        } else {
            let _ = std::fs::remove_file(&path);
        }
    }
}

fn write(folder: &Path, path: &Path, saved: &Saved) -> Result<(), String> {
    let plain = serde_json::to_vec(saved).map_err(|error| error.to_string())?;
    let key = key(folder).map_err(|error| error.to_string())?;
    let sealed = term_seal::seal(&key, &plain).map_err(|error| error.to_string())?;
    term_base::atomic_write::write(path, &sealed).map_err(|error| error.to_string())
}

/// Opens the windows stored when the app last quit; whether one opened. The file stays, as it holds the workspaces
/// the user keeps, until the next store replaces it.
pub(crate) fn reopen() -> bool {
    if !app().config().restore.enabled {
        return false;
    }
    let folder = folder();
    let path = folder.join(FILE);
    let Ok(sealed) = std::fs::read(&path) else {
        return false;
    };
    let saved = key(&folder)
        .map_err(|error| error.to_string())
        .and_then(|key| term_seal::open(&key, &sealed).map_err(|error| error.to_string()))
        .and_then(|plain| read(&plain));
    match saved {
        Ok(saved) => {
            for window in &saved.windows {
                Window::reopen(window);
            }
            !saved.windows.is_empty()
        }
        Err(error) => {
            term_base::diag::report("restoring the windows", &error);
            false
        }
    }
}

fn read(plain: &[u8]) -> Result<Saved, String> {
    #[derive(Deserialize)]
    struct Version {
        version: u32,
    }
    #[derive(Deserialize)]
    struct SavedV1 {
        windows: Vec<SavedWindowV1>,
    }
    let Version { version } = serde_json::from_slice(plain).map_err(|error| error.to_string())?;
    match version {
        VERSION => serde_json::from_slice(plain).map_err(|error| error.to_string()),
        1 => {
            let old: SavedV1 = serde_json::from_slice(plain).map_err(|error| error.to_string())?;
            Ok(Saved {
                version: VERSION,
                windows: old.windows.into_iter().map(SavedWindowV1::migrated).collect(),
            })
        }
        _ => Err(format!("stored by a layout {version} this build does not read")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_panes::layout::{Node, Pane, WindowTab};

    fn session(folder: &str, rows: &[&str]) -> SavedSession {
        SavedSession {
            folder: folder.into(),
            title: Some("✳ agent".into()),
            focused: false,
            rows: rows.iter().map(|row| (*row).to_string()).collect(),
            field: Some(true),
        }
    }

    fn split_tab() -> WindowTab<SavedSession> {
        WindowTab {
            root: Node::Split {
                side_by_side: true,
                children: vec![
                    Node::Pane(Pane {
                        documents: vec![session("/tmp", &["$ ls", "a b"])],
                        selected: 0,
                    }),
                    Node::Pane(Pane {
                        documents: vec![SavedSession {
                            focused: true,
                            ..session("/", &[])
                        }],
                        selected: 0,
                    }),
                ],
                shares: vec![0.3, 0.7],
            },
        }
    }

    #[test]
    fn workspaces_with_their_tabs_survive_the_round_trip_through_the_seal() {
        let saved = Saved {
            version: VERSION,
            windows: vec![SavedWindow {
                frame: Some([10.0, 20.0, 800.0, 600.0]),
                workspaces: vec![
                    SavedWorkspace {
                        base: "/tmp".into(),
                        name: Some("Scratch".into()),
                        layout: Layout {
                            tabs: vec![split_tab(), split_tab()],
                            selected: Some(1),
                        },
                        trusted: true,
                        agents: vec![SavedAgent {
                            at: 1,
                            session: "4f1c".into(),
                            selected: false,
                        }],
                        browsers: vec![SavedBrowser {
                            at: 3,
                            url: "http://localhost:5173/".into(),
                            selected: false,
                        }],
                        files: vec![SavedFile {
                            at: 4,
                            path: "/tmp/notes.md".into(),
                            selected: true,
                        }],
                        panels: crate::panels::Panels {
                            files: crate::panels::State { open: true },
                            git: crate::panels::State::default(),
                        },
                        separate: vec![SavedGroup {
                            tabs: vec![2],
                            frame: [900.0, 20.0, 700.0, 500.0],
                        }],
                    },
                    SavedWorkspace {
                        base: "/".into(),
                        name: None,
                        layout: Layout {
                            tabs: vec![split_tab()],
                            selected: Some(0),
                        },
                        trusted: false,
                        agents: Vec::new(),
                        browsers: Vec::new(),
                        files: Vec::new(),
                        panels: crate::panels::Panels::default(),
                        separate: Vec::new(),
                    },
                ],
                selected: Some(1),
                sidebar_hidden: true,
                sidebar_width: Some(260.0),
                without_scratch: true,
            }],
        };
        let key = [9u8; term_seal::KEY_LEN];
        let plain = serde_json::to_vec(&saved).unwrap();
        let sealed = term_seal::seal(&key, &plain).unwrap();
        let back = read(&term_seal::open(&key, &sealed).unwrap()).unwrap();
        assert_eq!(back, saved);
    }

    #[test]
    fn a_window_stored_before_workspaces_comes_back_as_one_workspace() {
        let old = serde_json::json!({
            "version": 1,
            "windows": [{
                "frame": [10.0, 20.0, 800.0, 600.0],
                "layout": { "tabs": [split_tab(), split_tab()], "selected": 1 },
            }],
        });
        let back = read(&serde_json::to_vec(&old).unwrap()).unwrap();
        let [window] = &back.windows[..] else {
            panic!("{back:?}")
        };
        let [workspace] = &window.workspaces[..] else {
            panic!("{window:?}")
        };
        assert_eq!(workspace.base, PathBuf::from("/"), "the active pane's folder");
        assert_eq!(workspace.layout.tabs.len(), 2);
        assert_eq!(workspace.layout.selected, Some(1));
        assert_eq!(window.selected, Some(0));
        assert!(!workspace.trusted, "a workspace is not trusted until the user says so");
        assert!(workspace.agents.is_empty());
    }

    #[test]
    fn a_window_stored_before_native_tabs_still_opens() {
        let old = serde_json::json!({
            "version": 2,
            "windows": [{
                "frame": [10.0, 20.0, 800.0, 600.0],
                "workspaces": [{
                    "base": "/tmp",
                    "layout": { "tabs": [split_tab()], "selected": 0 },
                }],
                "selected": 0,
                "sidebar_hidden": true,
            }],
        });
        let back = read(&serde_json::to_vec(&old).unwrap()).unwrap();
        let [window] = &back.windows[..] else {
            panic!("{back:?}")
        };
        assert!(window.sidebar_hidden);
        assert_eq!(window.sidebar_width, None, "the sidebar opens as wide as a new one");
        assert!(
            window.workspaces[0].separate.is_empty(),
            "every tab back in its workspace's group"
        );
    }

    #[test]
    fn a_window_with_only_its_terminal_keeps_its_frame_and_sidebar() {
        let saved = Saved {
            version: VERSION,
            windows: vec![SavedWindow {
                frame: Some([40.0, 60.0, 1200.0, 800.0]),
                workspaces: Vec::new(),
                selected: None,
                sidebar_hidden: true,
                sidebar_width: Some(300.0),
                without_scratch: true,
            }],
        };
        let back = read(&serde_json::to_vec(&saved).unwrap()).unwrap();
        assert_eq!(back, saved);
    }

    #[test]
    fn a_session_stored_before_its_field_was_follows_the_settings() {
        let old = serde_json::json!({ "folder": "/tmp", "rows": ["$ ls"] });
        let session: SavedSession = serde_json::from_value(old).unwrap();
        assert_eq!(session.field, None);
        let off = SavedSession {
            field: Some(false),
            ..session
        };
        let back: SavedSession = serde_json::from_slice(&serde_json::to_vec(&off).unwrap()).unwrap();
        assert_eq!(back.field, Some(false));
    }

    #[test]
    fn another_layout_is_not_read() {
        let plain = br#"{"version":99,"windows":[]}"#;
        assert!(read(plain).is_err());
    }

    #[test]
    fn the_total_budget_runs_out_and_each_session_is_capped() {
        assert_eq!(Budget::new().available(), MAX_SESSION_BYTES);
        let mut budget = Budget(10);
        budget.spent(&["67890".to_string(), "abc".to_string()]);
        assert_eq!(budget.available(), 0);
    }
}
