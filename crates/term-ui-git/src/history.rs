//! The graph in the Git panel, as VS Code shows it: a row per commit with its dot, subject, author and the branches and
//! tags at it; a commit opens to the files it changed, each with its diff; details on hover; a row that reads more.

use std::path::Path;
use term_ui::workspace_host::Files;

use objc2::rc::{Retained, Weak};
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSFont, NSLayoutConstraintOrientation, NSLineBreakMode, NSPasteboard,
    NSPasteboardTypeString, NSStackView, NSTableCellView, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::NSArray;
use splat_i18n::{t, tf};
use term_git::{Commit, CommitChanges, CommitId};

use super::graph::{GraphRow, GraphView};
use super::hover::HoverCell;
use super::model::RowKey;
use super::model::{HISTORY_PAGE, State, Task};
use super::panel::{GitPanel, with_panel};
use super::rows::{file_text, letter_label, pill_with};
use super::{sheets, work};
use term_render::constant;
use term_ui::kit::ui_size::{font_like, scaled, system};
use term_ui::kit::{activate, center_y, label, ns};

/// How far a row's text keeps from the graph.
const AFTER_GRAPH: f64 = 4.0;
/// How far a commit's files sit in from the graph, so they read as part of the commit.
const FILE_INDENT: f64 = 10.0;
/// How many branches and tags a commit's row shows; the tooltip names them all.
const MAX_BADGES: usize = 3;

/// The menu of a commit or of a file below one.
pub(crate) fn menu(keys: &[RowKey]) -> Vec<(&'static str, Option<Sel>)> {
    match keys {
        [RowKey::Commit(_)] => vec![
            (t("Commit-ID kopieren"), Some(sel!(termGitCopyHash:))),
            (t("Commit-Nachricht kopieren"), Some(sel!(termGitCopyMessage:))),
            ("", None),
            (t("Branch hier anlegen …"), Some(sel!(termGitBranchHere:))),
        ],
        [RowKey::CommitFile(..)] => vec![
            (t("Diff zeigen"), Some(sel!(termGitDiff:))),
            (t("Im Editor öffnen"), Some(sel!(termGitOpen:))),
        ],
        _ => vec![],
    }
}

/// What a name at a commit is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Badge {
    /// The branch HEAD is on.
    Current,
    /// HEAD without a branch.
    Detached,
    Local,
    Remote,
    Tag,
}

/// The branches and tags at `commit`, the current branch first; a remote's own HEAD is left out.
fn badges(refs: &[String], remotes: &[String]) -> Vec<(Badge, String)> {
    let mut found: Vec<(Badge, String)> = refs
        .iter()
        .filter_map(|name| {
            if let Some(branch) = name.strip_prefix("HEAD -> ") {
                Some((Badge::Current, branch.to_string()))
            } else if name == "HEAD" {
                Some((Badge::Detached, name.clone()))
            } else if let Some(tag) = name.strip_prefix("tag: ") {
                Some((Badge::Tag, tag.to_string()))
            } else if name.ends_with("/HEAD") {
                None
            } else if remotes.iter().any(|r| r == name) {
                Some((Badge::Remote, name.clone()))
            } else {
                Some((Badge::Local, name.clone()))
            }
        })
        .collect();
    found.sort_by_key(|(badge, _)| *badge != Badge::Current && *badge != Badge::Detached);
    found
}

fn badge_view(mtm: MainThreadMarker, badge: Badge, name: &str) -> Retained<NSView> {
    let accent = NSColor::controlAccentColor();
    let (symbol, fill, color) = match badge {
        Badge::Current | Badge::Detached => ("target", accent.clone(), NSColor::alternateSelectedControlTextColor()),
        Badge::Local => (
            "arrow.triangle.branch",
            accent.colorWithAlphaComponent(0.18),
            NSColor::labelColor(),
        ),
        Badge::Remote => ("cloud", NSColor::quaternaryLabelColor(), NSColor::secondaryLabelColor()),
        Badge::Tag => ("tag", NSColor::quaternaryLabelColor(), NSColor::secondaryLabelColor()),
    };
    // A remote branch shows as its cloud alone, as in VS Code; its name is in the tooltip and for VoiceOver.
    let text = if badge == Badge::Remote { "" } else { name };
    let view = pill_with(mtm, Some(symbol), text, &fill, &color);
    if text.is_empty() {
        view.setToolTip(Some(&ns(name)));
        view.setAccessibilityLabel(Some(&ns(name)));
    }
    view
}

/// What hovering over a commit tells: author and time, the whole message, the branches, what it changed once read,
/// and its hash.
fn details(
    commit: &Commit,
    names: &[(Badge, String)],
    changes: Option<&CommitChanges>,
    dates: &objc2_foundation::NSDateFormatter,
) -> String {
    let date = objc2_foundation::NSDate::dateWithTimeIntervalSince1970(commit.author_time as f64);
    let mut text = tf(
        "{}, {} ({})",
        &[
            &commit.author_name,
            &super::text::ago(now() - commit.author_time),
            &dates.stringFromDate(&date).to_string(),
        ],
    );
    text.push_str("\n\n");
    text.push_str(&commit.subject);
    if !commit.body.is_empty() {
        text.push_str("\n\n");
        text.push_str(&commit.body);
    }
    if let Some(changes) = changes {
        let files = match changes.files.len() {
            1 => t("1 Datei geändert").to_string(),
            n => tf("{} Dateien geändert", &[&n.to_string()]),
        };
        text.push_str("\n\n");
        text.push_str(&tf(
            "{}, {} Einfügungen (+), {} Löschungen (−)",
            &[&files, &changes.insertions.to_string(), &changes.deletions.to_string()],
        ));
    }
    if !names.is_empty() {
        let all: Vec<&str> = names.iter().map(|(_, name)| name.as_str()).collect();
        text.push_str("\n\n");
        text.push_str(&all.join(", "));
    }
    text.push_str("\n\n");
    text.push_str(commit.hash.short());
    text
}

/// The remote branches among the names at `commit`.
pub(crate) fn remotes_at(state: &State, commit: &Commit) -> Vec<String> {
    state
        .branches
        .iter()
        .filter(|b| b.remote && commit.refs.contains(&b.name))
        .map(|b| b.name.clone())
        .collect()
}

/// Seconds since 1970.
pub(crate) fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Puts `graph` at the leading edge of `cell`, as tall as the row.
fn add_graph(mtm: MainThreadMarker, cell: &NSView, graph: GraphRow) -> Retained<GraphView> {
    let width = GraphView::width(&graph);
    let view = GraphView::new(mtm, graph);
    cell.addSubview(&view);
    view.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        view.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
        view.topAnchor().constraintEqualToAnchor(&cell.topAnchor()),
        view.bottomAnchor().constraintEqualToAnchor(&cell.bottomAnchor()),
        view.widthAnchor().constraintEqualToConstant(width),
    ]);
    view
}

impl GitPanel {
    /// The row of the commit `hash`.
    pub(crate) fn commit_cell(&self, mtm: MainThreadMarker, hash: &CommitId) -> Option<Retained<NSView>> {
        let state = self.read();
        let (commit, graph) = state.model.commit_row(hash)?;
        let graph = graph.clone();
        let names = badges(&commit.refs, &remotes_at(&state, commit));
        let (words, by) = (commit.subject.clone(), commit.author_name.clone());
        let age = super::text::ago(now() - commit.author_time);
        drop(state);
        let cell = HoverCell::new(mtm);
        let head = graph.head;
        let graph_view = add_graph(mtm, &cell, graph);
        let subject = label(mtm, &words);
        subject.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let font = if head {
            NSFont::boldSystemFontOfSize(scaled(NSFont::systemFontSize()))
        } else {
            system(NSFont::systemFontSize())
        };
        subject.setFont(Some(&font));
        subject
            .setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
        let author = label(mtm, &by);
        author.setFont(Some(&font_like(&term_ui::kit::style::small_system())));
        author.setTextColor(Some(&NSColor::secondaryLabelColor()));
        author.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        author.setContentCompressionResistancePriority_forOrientation(90.0, NSLayoutConstraintOrientation::Horizontal);
        let text = NSStackView::new(mtm);
        text.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        text.setSpacing(scaled(term_ui::kit::style::space::S));
        text.addArrangedSubview(&subject);
        text.addArrangedSubview(&author);
        let pills = NSStackView::new(mtm);
        pills.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        pills.setSpacing(scaled(term_ui::kit::style::space::TIGHT));
        for (badge, name) in names.iter().take(MAX_BADGES) {
            pills.addArrangedSubview(&badge_view(mtm, *badge, name));
        }
        for view in [&*text as &NSView, &pills] {
            cell.addSubview(view);
            center_y(view, &cell);
        }
        activate([
            text.leadingAnchor()
                .constraintEqualToAnchor_constant(&graph_view.trailingAnchor(), scaled(AFTER_GRAPH)),
            pills.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
            text.trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&pills.leadingAnchor(), -scaled(6.0)),
        ]);
        let short = hash.short();
        cell.setAccessibilityLabel(Some(&ns(&tf("Commit {}: {}, von {}, {}", &[short, &words, &by, &age]))));
        let key = RowKey::Commit(hash.clone());
        let custom = self.custom_actions(&key, &menu(std::slice::from_ref(&key)));
        cell.setAccessibilityCustomActions(Some(&NSArray::from_retained_slice(&custom)));
        let (panel, theirs) = (Weak::from(self), hash.clone());
        cell.set_tip(move || panel.load()?.commit_details(&theirs));
        let (id, theirs) = (self.id(), hash.clone());
        cell.on_rest(move || with_panel(id, |panel| panel.read_changes(&theirs)));
        // SAFETY: The label is a subview of the cell, which keeps it.
        unsafe { cell.setTextField(Some(&subject)) };
        Some(cell.into_super().into_super())
    }

    /// The tooltip of the commit `hash`.
    fn commit_details(&self, hash: &CommitId) -> Option<String> {
        let state = self.read();
        let commit = state.model.commit(hash)?;
        let names = badges(&commit.refs, &remotes_at(&state, commit));
        Some(details(commit, &names, state.commit_changes.get(hash), self.dates()))
    }

    /// The row of a file the commit `hash` changed.
    pub(crate) fn commit_file_cell(
        &self,
        mtm: MainThreadMarker,
        hash: &CommitId,
        path: &Path,
    ) -> Option<Retained<NSView>> {
        let state = self.read();
        let lanes = state.model.commit_row(hash)?.1.through();
        let change = state
            .commit_changes
            .get(hash)?
            .files
            .iter()
            .find(|f| f.path == path)?
            .change;
        drop(state);
        let mark = super::rows::change_mark(change);
        let cell = NSTableCellView::new(mtm);
        let graph_view = add_graph(mtm, &cell, lanes);
        let (text, name) = file_text(mtm, path, mark == super::model::Mark::Deleted);
        let letter = letter_label(mtm, mark);
        for view in [&*text as &NSView, &letter] {
            cell.addSubview(view);
            center_y(view, &cell);
        }
        activate([
            text.leadingAnchor()
                .constraintEqualToAnchor_constant(&graph_view.trailingAnchor(), scaled(FILE_INDENT)),
            letter.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
            text.trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&letter.leadingAnchor(), -scaled(4.0)),
        ]);
        let described = tf("{}, {}", &[&path.display().to_string(), mark.word()]);
        cell.setToolTip(Some(&ns(&described)));
        cell.setAccessibilityLabel(Some(&ns(&described)));
        let key = RowKey::CommitFile(hash.clone(), path.to_path_buf());
        let custom = self.custom_actions(&key, &menu(std::slice::from_ref(&key)));
        cell.setAccessibilityCustomActions(Some(&NSArray::from_retained_slice(&custom)));
        // SAFETY: The label is a subview of the cell, which keeps it.
        unsafe { cell.setTextField(Some(&name)) };
        Some(cell.into_super())
    }

    /// The row at the end of the graph that reads more of the history.
    pub(crate) fn more_cell(&self, mtm: MainThreadMarker, lanes: GraphRow) -> Retained<NSView> {
        let cell = NSTableCellView::new(mtm);
        let graph_view = add_graph(mtm, &cell, lanes);
        let more = label(mtm, &tf("{} weitere laden …", &[&HISTORY_PAGE.to_string()]));
        more.setTextColor(Some(&NSColor::linkColor()));
        more.setFont(Some(&font_like(&term_ui::kit::style::small_system())));
        cell.addSubview(&more);
        more.setTranslatesAutoresizingMaskIntoConstraints(false);
        activate([
            more.leadingAnchor()
                .constraintEqualToAnchor_constant(&graph_view.trailingAnchor(), scaled(AFTER_GRAPH)),
            more.centerYAnchor().constraintEqualToAnchor(&cell.centerYAnchor()),
        ]);
        // SAFETY: The label is a subview of the cell, which keeps it.
        unsafe { cell.setTextField(Some(&more)) };
        cell.into_super()
    }

    /// Reads what the commit `hash` changed, once; the commit's row then names it on hover, and an open row lists the
    /// files.
    pub(crate) fn read_changes(&self, hash: &CommitId) {
        {
            let mut state = self.state();
            if state.commit_changes.contains_key(hash) || !state.reading_changes.insert(hash.clone()) {
                return;
            }
        }
        let Some(repo) = self.repo() else {
            self.state().reading_changes.remove(hash);
            return;
        };
        let id = self.id();
        let theirs = hash.clone();
        let mine = hash.clone();
        let queued = work::local(
            move || repo.commit_changes(&theirs),
            move |result| with_panel(id, |panel| panel.changes_read(&mine, result)),
        );
        if !queued {
            self.state().reading_changes.remove(hash);
        }
    }

    fn changes_read(&self, hash: &CommitId, result: Option<term_git::Result<CommitChanges>>) {
        self.state().reading_changes.remove(hash);
        let changes = match result {
            Some(Ok(changes)) => changes,
            Some(Err(error)) => return self.report(&error),
            None => return,
        };
        if !self.state().note_commit_changes(hash, changes) {
            return;
        }
        let item = self.item_for(&RowKey::Commit(hash.clone()));
        let outline = &self.ui().outline;
        // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
        let row = unsafe { outline.rowForItem(Some(&item)) };
        if row < 0 {
            return;
        }
        // SAFETY: The item comes from the panel's item cache, which the outline view already knows and the panel keeps alive.
        unsafe { outline.reloadItem_reloadChildren(Some(&item), true) };
        if self.read().open_commits.contains(hash) {
            // SAFETY: The same cached item, which the reload just above kept in the view.
            unsafe { outline.expandItem(Some(&item)) };
        }
    }

    /// Reads the next page of the history.
    pub(crate) fn read_more(&self) {
        self.state().more_pages += 1;
        self.start_refresh();
    }

    pub(crate) fn show_commit_diff(&self, hash: &CommitId, file: &Path, preview: bool) {
        let (Some(controller), Some(repo)) = (self.controller(), self.repo()) else {
            return;
        };
        super::diff::show_commit(&controller, repo.root(), hash, file, preview);
    }

    /// Opens the files below commits as they are now, if they still exist.
    pub(crate) fn open_commit_files(&self, keys: &[RowKey]) {
        let (Some(controller), Some(repo)) = (self.controller(), self.repo()) else {
            return;
        };
        for key in keys {
            if let RowKey::CommitFile(_, path) = key {
                let full = repo.root().join(path);
                if full.is_file() {
                    controller.open_file(&full);
                }
            }
        }
    }

    /// Runs a menu action of the graph on `keys`.
    pub(crate) fn run_history_action(&self, action: Sel, keys: &[RowKey]) {
        let Some(hash) = keys.iter().find_map(|key| match key {
            RowKey::Commit(hash) => Some(hash.clone()),
            _ => None,
        }) else {
            return;
        };
        if action == sel!(termGitCopyHash:) {
            copy(hash.as_str());
        } else if action == sel!(termGitCopyMessage:) {
            let message = self.read().model.commit(&hash).map(|c| {
                if c.body.is_empty() {
                    c.subject.clone()
                } else {
                    format!("{}\n\n{}", c.subject, c.body)
                }
            });
            if let Some(message) = message {
                copy(&message);
            }
        } else if action == sel!(termGitBranchHere:) {
            self.branch_at(hash);
        }
    }

    /// Asks for a name and makes a branch at the commit `hash`, then switches to it.
    fn branch_at(&self, hash: CommitId) {
        let Some(window) = self.controller().and_then(|c| c.window()) else {
            return;
        };
        let info = tf(
            "Er beginnt bei Commit {}, und Git wechselt gleich zu ihm.",
            &[hash.short()],
        );
        let id = self.id();
        sheets::branch_name(&window, t("Neuer Branch"), &info, move |name| {
            with_panel(id, |panel| {
                let Some(repo) = panel.repo() else { return };
                if !panel.begin(Task::Switch, None) {
                    return;
                }
                work::long(
                    move || repo.switch_to_new_branch(&name, Some(hash.as_str())),
                    move |result| with_panel(id, |panel| panel.branch_made(result)),
                );
            });
        });
    }

    fn branch_made(&self, result: Option<term_git::Result<()>>) {
        self.end();
        if let Some(Err(error)) = result {
            self.report(&error);
        }
        self.start_refresh();
    }
}

/// Puts `text` on the clipboard.
fn copy(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setString_forType(&ns(text), constant!(NSPasteboardTypeString));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badges_tell_branches_remotes_and_tags_apart_and_put_the_current_branch_first() {
        let commit = Commit {
            hash: CommitId::parse(&"a".repeat(40)).unwrap(),
            parents: vec![],
            subject: String::new(),
            body: String::new(),
            author_name: String::new(),
            author_email: String::new(),
            author_time: 0,
            refs: vec![
                "tag: v1".into(),
                "origin/main".into(),
                "HEAD -> main".into(),
                "origin/HEAD".into(),
                "topic".into(),
            ],
        };
        let found = badges(&commit.refs, &["origin/main".to_string()]);
        assert_eq!(
            found,
            [
                (Badge::Current, "main".to_string()),
                (Badge::Tag, "v1".to_string()),
                (Badge::Remote, "origin/main".to_string()),
                (Badge::Local, "topic".to_string()),
            ]
        );
    }
}
