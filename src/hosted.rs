//! A tab whose view a UI crate made and handed over through the workspace host (E3, E21): an agent's session today,
//! a browser or later a file (E22). The app knows of it only its view, key, title and status.

use std::cell::{Cell, RefCell};
use std::path::PathBuf;

use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView};
use term_ui::workspace_host::{AgentStatus, TabKind};
use term_ui_sidebar::{Entry, Kind, Status};

/// A tab with a view of a UI crate.
pub(crate) struct Hosted {
    kind: TabKind,
    key: RefCell<Option<String>>,
    title: RefCell<String>,
    icon: term_ui_icons::Icon,
    view: Retained<NSView>,
    status: Cell<AgentStatus>,
    folder: PathBuf,
    /// Whether the file it shows has unsaved changes.
    dirty: Cell<bool>,
    /// The colour of its symbol in the tab bar, or the colours of the symbol's four points, as its crate gave them.
    tint: RefCell<Option<Retained<NSColor>>>,
    four: RefCell<Option<[Retained<NSColor>; 4]>>,
}

impl Hosted {
    /// A tab of `kind` showing `view`, known by `key`, named `title`, for what runs in `folder`.
    pub(crate) fn new(
        kind: TabKind,
        key: Option<String>,
        title: &str,
        view: Retained<NSView>,
        folder: PathBuf,
    ) -> Hosted {
        Hosted {
            kind,
            key: RefCell::new(key),
            icon: icon(kind, title),
            title: RefCell::new(title.to_string()),
            view,
            status: Cell::new(AgentStatus::Idle),
            folder,
            dirty: Cell::new(false),
            tint: RefCell::new(None),
            four: RefCell::new(None),
        }
    }

    pub(crate) fn kind(&self) -> TabKind {
        self.kind
    }

    pub(crate) fn key(&self) -> Option<String> {
        self.key.borrow().clone()
    }

    /// Knows the tab by `key` from now on.
    pub(crate) fn set_key(&self, key: Option<String>) {
        *self.key.borrow_mut() = key;
    }

    /// Marks whether its file has unsaved changes; whether that changed the mark.
    pub(crate) fn set_dirty(&self, dirty: bool) -> bool {
        self.dirty.replace(dirty) != dirty
    }

    pub(crate) fn title(&self) -> String {
        self.title.borrow().clone()
    }

    /// Names the tab `title`; whether that changed its name.
    pub(crate) fn set_title(&self, title: &str) -> bool {
        let changed = *self.title.borrow() != title;
        if changed {
            *self.title.borrow_mut() = title.to_string();
        }
        changed
    }

    pub(crate) fn status(&self) -> AgentStatus {
        self.status.get()
    }

    /// Shows `status`; whether that changed what shows.
    pub(crate) fn set_status(&self, status: AgentStatus) -> bool {
        self.status.replace(status) != status
    }

    /// Gives its symbol in the tab bar `tint`.
    pub(crate) fn set_tint(&self, tint: &NSColor) {
        *self.tint.borrow_mut() = Some(objc2::Message::retain(tint));
    }

    /// Gives its symbol in the tab bar the colours of its four points instead of a tint.
    pub(crate) fn set_four_colours(&self, colors: [Retained<NSColor>; 4]) {
        *self.four.borrow_mut() = Some(colors);
    }

    /// The tint of its symbol in the tab bar and the colours of the symbol's four points.
    pub(crate) fn colours(&self) -> (Option<Retained<NSColor>>, Option<[Retained<NSColor>; 4]>) {
        (self.tint.borrow().clone(), self.four.borrow().clone())
    }

    pub(crate) fn view(&self) -> Retained<NSView> {
        self.view.clone()
    }

    /// What the tab bar and the sidebar show of the tab `id`.
    pub(crate) fn entry(&self, id: u64) -> Entry {
        Entry {
            id,
            kind: kind(self.kind),
            icon: self.icon,
            others: Vec::new(),
            title: Some(self.title()),
            folder: self.folder.clone(),
            branch: None,
            last_command: None,
            status: status(self.status()),
            progress: None,
            notice: None,
        }
    }

    /// What closing the tab ends: the agent's session while it works or waits for the user.
    pub(crate) fn busy(&self) -> Vec<String> {
        match self.status() {
            AgentStatus::Running | AgentStatus::Waiting => vec![self.title()],
            AgentStatus::Idle | AgentStatus::Failed | AgentStatus::Attention => Vec::new(),
        }
    }

    /// The tab came to the front.
    pub(crate) fn seen(&self) {
        if self.status() == AgentStatus::Attention {
            self.status.set(AgentStatus::Idle);
        }
    }

    /// Takes the view out of its window, which ends what the crate runs in it.
    pub(crate) fn close(&self) {
        self.view.removeFromSuperview();
    }
}

/// The sidebar's kind of a tab of `kind`.
fn kind(kind: TabKind) -> Kind {
    match kind {
        TabKind::Terminal => Kind::Terminal,
        TabKind::Agent => Kind::Agent,
        TabKind::Browser => Kind::Browser,
        TabKind::File | TabKind::Log => Kind::File,
    }
}

/// The sidebar's dot for an agent's `status`; waiting for an approval shows apart from attention (F132).
pub(crate) fn status(status: AgentStatus) -> Status {
    match status {
        AgentStatus::Idle => Status::Idle,
        AgentStatus::Running => Status::Running,
        AgentStatus::Waiting => Status::Waiting,
        AgentStatus::Failed => Status::Failed,
        AgentStatus::Attention => Status::Attention,
    }
}

/// The icon of a tab of `kind` first named `title`: an agent's logo where the icons know its program.
fn icon(kind: TabKind, title: &str) -> term_ui_icons::Icon {
    match kind {
        TabKind::Agent => {}
        TabKind::Browser => return term_ui_icons::BROWSER,
        TabKind::File => return term_ui_icons::icon_of(title),
        TabKind::Terminal | TabKind::Log => return term_ui_icons::PROGRAM,
    }
    let first = title.split_whitespace().next().unwrap_or_default().to_lowercase();
    let program = match first.as_str() {
        "antigravity" => "agy",
        other => other,
    };
    term_ui_icons::program_icon(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_agent_waiting_for_approval_shows_apart_and_counts_as_busy() {
        assert_eq!(status(AgentStatus::Waiting), Status::Waiting);
        assert!(Status::Waiting.urgency() > Status::Attention.urgency());
        assert_eq!(kind(TabKind::Agent), Kind::Agent);
        assert_eq!(icon(TabKind::Agent, "Claude"), term_ui_icons::program_icon("claude"));
    }
}
