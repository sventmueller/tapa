//! A tab of a workspace (E20): a terminal with its panes, one session each (E12), and what the tab bar and the
//! sidebar show of each session, kept from what the sessions report.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use objc2::Message;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use term_session::Foreground;
use term_session::event::Progress;
use term_ui::{Change, TerminalView};
use term_ui_panes::Panes;
use term_ui_sidebar::{Entry, Status};

use crate::hosted::Hosted;

/// What the app knows of one session from its reports.
#[derive(Debug, Clone, Default)]
pub(crate) struct Session {
    pub(crate) title: Option<String>,
    pub(crate) folder: PathBuf,
    /// The folder the shell reported last from the other end of ssh, until it reports one on this Mac again.
    pub(crate) elsewhere: Option<PathBuf>,
    pub(crate) branch: Option<String>,
    pub(crate) running: Option<(String, Instant)>,
    pub(crate) last: Option<(String, Duration)>,
    pub(crate) status: Status,
    /// The full line of the command that runs, and of the one before, for the history (E8).
    pub(crate) running_line: Option<String>,
    pub(crate) previous_line: Option<String>,
    pub(crate) previous_exit: Option<i32>,
    pub(crate) progress: Option<Progress>,
    /// A notification the user has not seen, as the sidebar shows it.
    pub(crate) notice: Option<String>,
}

impl Session {
    /// Where its shell is when it belongs to a remote workspace (E25), which titles show in place of the
    /// placeholder folder on this Mac.
    pub(crate) fn place(&self) -> Option<String> {
        crate::remote::place_of(&self.folder, self.elsewhere.as_deref())
    }

    /// Its folder as titles show it, short.
    pub(crate) fn short_folder(&self, home: &std::path::Path) -> String {
        self.place()
            .unwrap_or_else(|| term_ui_sidebar::model::short_folder(&self.folder, home))
    }
}

/// What a workspace's tab is to the tab bar, the sidebar and the window, whatever its kind: a terminal with its
/// panes, or an agent's session in its own view (E21).
pub(crate) trait TabContent {
    /// What the tab bar shows of it and the sidebar sums up: title, icon, status.
    fn entry(&self) -> Entry;
    /// The view that fills the content area while it is in front.
    fn view(&self) -> Retained<NSView>;
    /// The programs that run in it, which closing it would end.
    fn busy(&self) -> Vec<String>;
    /// It came to the front: what needed attention has had it.
    fn seen(&self);
    /// Ends what runs in it, as it closes.
    fn close(&self);
}

/// A tab of a workspace: a terminal, or a view a UI crate handed over (E21).
pub(crate) struct Tab {
    pub(crate) id: u64,
    body: Body,
}

enum Body {
    Terminal(Terminal),
    Hosted(Hosted),
}

/// A terminal with its panes and what the app knows of each pane's session.
pub(crate) struct Terminal {
    pub(crate) panes: Panes,
    pub(crate) sessions: RefCell<HashMap<u64, Session>>,
}

impl Tab {
    /// A terminal tab with the one session `view`, whose pane shows `root`, started in `folder`.
    pub(crate) fn new(id: u64, view: &TerminalView, root: Retained<objc2_app_kit::NSView>, folder: PathBuf) -> Tab {
        let panes = Panes::new(objc2::MainThreadOnly::mtm(view), view.id(), root);
        let tab = Tab::with_panes(id, panes);
        tab.add_session(view.id(), folder);
        tab
    }

    /// A terminal tab showing `panes`, whose sessions the caller adds.
    pub(crate) fn with_panes(id: u64, panes: Panes) -> Tab {
        Tab {
            id,
            body: Body::Terminal(Terminal {
                panes,
                sessions: RefCell::new(HashMap::new()),
            }),
        }
    }

    /// A tab showing what a UI crate handed over.
    pub(crate) fn hosted(id: u64, hosted: Hosted) -> Tab {
        Tab {
            id,
            body: Body::Hosted(hosted),
        }
    }

    /// Its terminal, unless it is a hosted tab.
    pub(crate) fn terminal(&self) -> Option<&Terminal> {
        match &self.body {
            Body::Terminal(terminal) => Some(terminal),
            Body::Hosted(_) => None,
        }
    }

    /// What a UI crate handed over, unless it is a terminal.
    pub(crate) fn as_hosted(&self) -> Option<&Hosted> {
        match &self.body {
            Body::Hosted(hosted) => Some(hosted),
            Body::Terminal(_) => None,
        }
    }

    pub(crate) fn add_session(&self, view: u64, folder: PathBuf) {
        if let Some(terminal) = self.terminal() {
            terminal.add_session(view, folder);
        }
    }

    /// The session of the active pane.
    pub(crate) fn focused(&self) -> Option<u64> {
        self.terminal()?.panes.focused()
    }

    /// The view of the active pane's session.
    pub(crate) fn focused_view(&self) -> Option<Retained<TerminalView>> {
        self.focused().and_then(term_ui::find)
    }

    /// Whether one of its panes shows session view `view`.
    pub(crate) fn holds(&self, view: u64) -> bool {
        self.terminal().is_some_and(|terminal| terminal.panes.holds(view))
    }

    /// Takes up what session `view` reported; whether the sidebar shows something new.
    pub(crate) fn changed(&self, view: u64, change: &Change, in_front: bool) -> bool {
        self.terminal()
            .is_some_and(|terminal| terminal.changed(view, change, in_front))
    }

    /// The title of the tab for its window: the active session's title, else its folder.
    pub(crate) fn title(&self) -> String {
        let entry = self.entry();
        entry.heading(&term_base::home())
    }

    /// The window's subtitle for the tab: for a terminal the command that runs in the active pane, else its folder
    /// when the title does not already name it.
    pub(crate) fn subtitle(&self, entry: &Entry, home: &std::path::Path) -> String {
        match &self.body {
            Body::Terminal(terminal) => terminal.subtitle(),
            Body::Hosted(_) => entry.detail(home),
        }
    }

    /// Where the user types in session `view`, for the history's suggestions.
    pub(crate) fn context(&self, view: u64) -> Option<term_suggest::Context> {
        self.terminal()?.context(view)
    }
}

impl Terminal {
    fn add_session(&self, view: u64, folder: PathBuf) {
        let branch = term_ui_sidebar::git::branch(&folder);
        self.sessions.borrow_mut().insert(
            view,
            Session {
                folder,
                branch,
                ..Session::default()
            },
        );
    }

    fn focused(&self) -> Option<u64> {
        self.panes.focused()
    }

    fn changed(&self, view: u64, change: &Change, in_front: bool) -> bool {
        let mut sessions = self.sessions.borrow_mut();
        let Some(session) = sessions.get_mut(&view) else {
            return false;
        };
        match change {
            Change::Title(title) => session.title.clone_from(title),
            Change::Directory(folder) => {
                session.branch = term_ui_sidebar::git::branch(folder);
                session.folder.clone_from(folder);
                session.elsewhere = None;
            }
            Change::Elsewhere(folder) => {
                if session.elsewhere.as_ref() == Some(folder) {
                    return false;
                }
                session.elsewhere = Some(folder.clone());
            }
            Change::Prompt => {
                if session.status == Status::Running {
                    session.status = Status::Idle;
                }
                session.progress = None;
                session.branch = term_ui_sidebar::git::branch(&session.folder);
            }
            Change::Input | Change::Focused | Change::Field => return false,
            // A banner for every `vite` would be noise in a terminal; Dango opens them in its browser tab.
            Change::DevServer(_) => return false,
            Change::CommandStarted(command) => {
                let line = command
                    .as_deref()
                    .map(term_ui_sidebar::model::short_command)
                    .unwrap_or_default();
                session.running = Some((line, Instant::now()));
                session.running_line.clone_from(command);
                session.status = Status::Running;
            }
            Change::CommandFinished(code) => {
                if let Some(line) = session.running_line.take() {
                    let line = line.trim().to_string();
                    crate::state::app().history.borrow_mut().record(term_suggest::Entry {
                        line: line.clone(),
                        folder: session.folder.clone(),
                        branch: session.branch.clone(),
                        previous: session.previous_line.clone(),
                        exit_code: *code,
                        started: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.as_secs()),
                    });
                    session.previous_line = Some(line);
                    session.previous_exit = *code;
                }
                if let Some((line, started)) = session.running.take() {
                    session.last = (!line.is_empty()).then(|| (line, started.elapsed()));
                }
                session.status = match code {
                    Some(0) | None => Status::Succeeded,
                    Some(_) => Status::Failed,
                };
                session.progress = None;
            }
            Change::Progress(progress) => {
                if session.progress == *progress {
                    return false;
                }
                session.progress = *progress;
            }
            Change::Notification(notification) => {
                if in_front {
                    return false;
                }
                session.status = Status::Attention;
                session.notice = Some(crate::notify::summary(notification));
                let heading = session
                    .title
                    .clone()
                    .filter(|title| worth_showing(title, &session.folder))
                    .unwrap_or_else(|| session.short_folder(&term_base::home()));
                crate::notify::post(view, notification, &heading);
            }
            Change::Bell => {
                if in_front {
                    return false;
                }
                session.status = Status::Attention;
            }
            Change::Exited(_) => {
                session.status = Status::Ended;
                session.progress = None;
            }
        }
        true
    }

    fn entry(&self, id: u64) -> Entry {
        let focused = self.focused();
        let sessions = self.sessions.borrow();
        let icon_of = |view: u64| {
            term_ui::find(view)
                .and_then(|view| view.with_session(term_session::Session::foreground))
                .map_or(term_ui_icons::SHELL, |front| icon(&front))
        };
        let main = focused.and_then(|id| sessions.get(&id)).cloned().unwrap_or_default();
        let others = self
            .panes
            .sessions()
            .into_iter()
            .filter(|id| Some(*id) != focused)
            .map(icon_of)
            .collect();
        let status = sessions
            .values()
            .map(|session| session.status)
            .max_by_key(|status| status.urgency())
            .unwrap_or_default();
        let progress = main
            .progress
            .or_else(|| sessions.values().find_map(|session| session.progress));
        let notice = main
            .notice
            .clone()
            .or_else(|| sessions.values().find_map(|session| session.notice.clone()));
        let last_command = match &main.running {
            Some((line, started)) if !line.is_empty() => Some((line.clone(), started.elapsed())),
            _ => main.last.clone(),
        };
        Entry {
            id,
            kind: term_ui_sidebar::Kind::Terminal,
            icon: focused.map_or(term_ui_icons::SHELL, icon_of),
            others,
            title: main.title.clone().filter(|title| worth_showing(title, &main.folder)),
            place: main.place(),
            folder: main.folder,
            branch: main.branch,
            last_command,
            status,
            progress,
            notice,
        }
    }

    fn subtitle(&self) -> String {
        let focused = self.focused();
        let sessions = self.sessions.borrow();
        let Some(session) = focused.and_then(|id| sessions.get(&id)) else {
            return String::new();
        };
        match &session.running {
            Some((line, _)) if !line.is_empty() => line.clone(),
            _ if session
                .title
                .as_deref()
                .is_some_and(|title| worth_showing(title, &session.folder)) =>
            {
                session.short_folder(&term_base::home())
            }
            _ => session.branch.clone().unwrap_or_default(),
        }
    }

    fn context(&self, view: u64) -> Option<term_suggest::Context> {
        let sessions = self.sessions.borrow();
        let session = sessions.get(&view)?;
        Some(term_suggest::Context {
            folder: session.folder.clone(),
            branch: session.branch.clone(),
            previous: session.previous_line.clone(),
            exit_code: session.previous_exit,
        })
    }
}

impl TabContent for Tab {
    fn entry(&self) -> Entry {
        match &self.body {
            Body::Terminal(terminal) => terminal.entry(self.id),
            Body::Hosted(hosted) => hosted.entry(self.id),
        }
    }

    fn view(&self) -> Retained<NSView> {
        match &self.body {
            Body::Terminal(terminal) => terminal.panes.view().retain(),
            Body::Hosted(hosted) => hosted.view(),
        }
    }

    fn busy(&self) -> Vec<String> {
        match &self.body {
            Body::Terminal(terminal) => terminal.busy(),
            Body::Hosted(hosted) => hosted.busy(),
        }
    }

    fn seen(&self) {
        match &self.body {
            Body::Terminal(terminal) => terminal.seen(),
            Body::Hosted(hosted) => hosted.seen(),
        }
    }

    fn close(&self) {
        match &self.body {
            Body::Terminal(terminal) => terminal.close(),
            Body::Hosted(hosted) => hosted.close(),
        }
    }
}

impl Terminal {
    /// Whether a program other than the shell runs in one of the tab's panes, which closing would end (F213).
    fn busy(&self) -> Vec<String> {
        self.panes
            .sessions()
            .into_iter()
            .filter_map(term_ui::find)
            .filter_map(|view| view.with_session(term_session::Session::foreground))
            .filter_map(|front| match front {
                Foreground::Agent(agent) => Some(format!("{agent:?}").to_lowercase()),
                Foreground::Remote(name) | Foreground::Other(name) => Some(name),
                Foreground::Shell(_) | Foreground::Unknown => None,
            })
            .collect()
    }

    fn seen(&self) {
        for session in self.sessions.borrow_mut().values_mut() {
            if session.status == Status::Attention {
                session.status = Status::Idle;
            }
            session.notice = None;
        }
    }

    fn close(&self) {
        for view in self.panes.sessions() {
            drop(crate::restore::take_rows(view));
            if let Some(view) = term_ui::find(view) {
                view.with_session(term_session::Session::close);
            }
        }
    }
}

/// The icon of the program in front.
pub(crate) fn icon(front: &Foreground) -> term_ui_icons::Icon {
    match front {
        Foreground::Agent(agent) => term_ui_icons::program_icon(match agent {
            term_session::Agent::Claude => "claude",
            term_session::Agent::Codex => "codex",
            term_session::Agent::Gemini => "gemini",
            term_session::Agent::Antigravity => "agy",
            term_session::Agent::OpenCode => "opencode",
        }),
        Foreground::Remote(_) => term_ui_icons::REMOTE,
        Foreground::Shell(name) => term_ui_icons::shell_icon(name),
        Foreground::Other(name) => term_ui_icons::program_icon(name),
        Foreground::Unknown => term_ui_icons::SHELL,
    }
}

/// Whether a title says more than the folder the sidebar shows anyway, as shells' default titles such as
/// `user@host:~/dir` do not.
pub(crate) fn worth_showing(title: &str, folder: &std::path::Path) -> bool {
    let title = title.trim();
    if title.is_empty() {
        return false;
    }
    let name = folder
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let looks_like_a_prompt = title.contains('@') && title.contains(':');
    !(looks_like_a_prompt
        || (!name.is_empty() && title.ends_with(&name))
        || title.starts_with('~')
        || title.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shells_default_title_is_not_worth_showing_but_an_agents_is() {
        let folder = std::path::Path::new("/Users/me/tapa");
        assert!(!worth_showing("me@mac:~/tapa", folder));
        assert!(!worth_showing("~/tapa", folder));
        assert!(!worth_showing("zsh tapa", folder));
        assert!(worth_showing("✳ Refactoring the parser", folder));
    }

    #[test]
    fn what_needs_the_user_shows_first() {
        assert!(Status::Attention.urgency() > Status::Failed.urgency());
        assert!(Status::Running.urgency() > Status::Succeeded.urgency());
    }
}
