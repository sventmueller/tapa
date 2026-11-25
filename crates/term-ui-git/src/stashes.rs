//! The stash in the Git panel: a row per entry with its files below it and their diffs in a tab; applying an
//! entry, applying and removing it, deleting it after a question; parking changes from the menu beside "Commit". Every
//! stash action first offers to save open documents with unsaved changes, since git sees only the files on disk
//! (review C14). An apply that meets conflicts shows as "Stash-Konflikt" until the user finishes or abandons it.

use std::path::{Path, PathBuf};

use objc2::rc::{Retained, Weak};
use objc2::runtime::Sel;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSColor, NSLayoutConstraintOrientation, NSLineBreakMode, NSStackView, NSTableCellView,
    NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::NSArray;
use splat_i18n::{t, tf};
use term_git::{Applied, CommitId, Stash, StashOptions};

use super::hover::HoverCell;
use super::model::RowKey;
use super::model::Task;
use super::panel::{GitPanel, with_panel};
use super::rows::{file_text, letter_label};
use super::text::Problem;
use super::{sheets, work};
use term_ui::kit::ui_size::{self, font_like, scaled, system};
use term_ui::kit::{activate, center_y, label, ns, symbol_at, symbol_button};

const ACTION_POINTS: f64 = 12.0;

/// The menu of a stash entry or of a file below one.
pub(crate) fn menu(keys: &[RowKey]) -> Vec<(&'static str, Option<Sel>)> {
    match keys {
        [RowKey::Stash(_)] => vec![
            (t("Anwenden"), Some(sel!(termGitApplyStash:))),
            (t("Anwenden und entfernen"), Some(sel!(termGitPopStash:))),
            ("", None),
            (t("Löschen …"), Some(sel!(termGitDropStash:))),
        ],
        [RowKey::StashFile(..)] => vec![(t("Diff zeigen"), Some(sel!(termGitDiff:)))],
        _ => vec![],
    }
}

/// The buttons a stash row shows on hover: symbol, title, action.
fn actions() -> [(&'static str, &'static str, Sel); 3] {
    [
        ("trash", t("Löschen …"), sel!(termGitDropStash:)),
        ("arrow.down.doc", t("Anwenden"), sel!(termGitApplyStash:)),
        (
            "arrow.down.to.line",
            t("Anwenden und entfernen"),
            sel!(termGitPopStash:),
        ),
    ]
}

/// The entry's own words, without the branch git puts before them, and the branch.
pub(crate) fn title_of(stash: &Stash) -> (String, Option<String>) {
    let message = stash.message.as_str();
    let rest = message.strip_prefix("WIP on ").or_else(|| message.strip_prefix("On "));
    match rest.and_then(|rest| rest.split_once(": ")) {
        Some((branch, words)) if !words.is_empty() => (words.to_string(), Some(branch.to_string())),
        _ => (message.to_string(), None),
    }
}

/// `stash@{n}`, as git names an entry.
fn name_of(stash: &Stash) -> String {
    format!("stash@{{{}}}", stash.index)
}

impl GitPanel {
    /// The row of the stash entry `hash`: its words, its age and, on hover, what it offers.
    pub(crate) fn stash_cell(&self, mtm: MainThreadMarker, hash: &CommitId) -> Option<Retained<NSView>> {
        let state = self.read();
        let stash = state.model.stash(hash)?.clone();
        drop(state);
        let (words, _) = title_of(&stash);
        let age = super::text::ago(now() - stash.time);
        let cell = HoverCell::new(mtm);
        let icon = objc2_app_kit::NSImageView::new(mtm);
        icon.setImage(symbol_at("archivebox", "", scaled(ui_size::DEFAULT)).as_deref());
        icon.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        term_ui::kit::hide_from_voiceover(&icon);
        let title = label(mtm, &words);
        title.setFont(Some(&system(term_ui::kit::style::BODY)));
        title.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        title.setContentCompressionResistancePriority_forOrientation(100.0, NSLayoutConstraintOrientation::Horizontal);
        let when = label(mtm, &age);
        when.setFont(Some(&font_like(&term_ui::kit::style::small_system())));
        when.setTextColor(Some(&NSColor::secondaryLabelColor()));
        when.setContentHuggingPriority_forOrientation(750.0, NSLayoutConstraintOrientation::Horizontal);
        let text = NSStackView::new(mtm);
        text.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        text.setSpacing(scaled(term_ui::kit::style::space::SNUG));
        text.addArrangedSubview(&icon);
        text.addArrangedSubview(&title);
        for view in [&*text as &NSView, &when] {
            cell.addSubview(view);
            center_y(view, &cell);
        }
        activate([
            text.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
            when.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
        ]);
        let panel = Weak::from(self);
        let build = move || Some(panel.load()?.stash_buttons(mtm).into_super());
        cell.set_actions(build, &text, &when, scaled(4.0), scaled(6.0));
        let described = tf("Stash {}: {}, {}", &[&name_of(&stash), &words, &age]);
        cell.setAccessibilityLabel(Some(&ns(&described)));
        let key = RowKey::Stash(stash.hash.clone());
        let custom = self.custom_actions(&key, &menu(std::slice::from_ref(&key)));
        cell.setAccessibilityCustomActions(Some(&NSArray::from_retained_slice(&custom)));
        let (panel, theirs) = (Weak::from(self), stash.hash.clone());
        cell.set_tip(move || {
            let panel = panel.load()?;
            let state = panel.read();
            let files = state.stash_changes.get(&theirs).map(Vec::len);
            let stash = state.model.stash(&theirs)?;
            Some(details(stash, title_of(stash).1.as_deref(), files))
        });
        let id = self.id();
        cell.on_rest(move || with_panel(id, |panel| panel.read_stash_changes(&stash.hash)));
        // SAFETY: The label is a subview of the cell, which keeps it.
        unsafe { cell.setTextField(Some(&title)) };
        Some(cell.into_super().into_super())
    }

    /// The buttons a stash row shows on hover.
    fn stash_buttons(&self, mtm: MainThreadMarker) -> Retained<NSStackView> {
        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        buttons.setSpacing(scaled(term_ui::kit::style::space::XXS));
        for (name, description, action) in actions() {
            let points = scaled(ACTION_POINTS);
            let button: Retained<NSButton> = symbol_button(mtm, name, description, points, Some(self), action);
            button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
            buttons.addArrangedSubview(&button);
        }
        buttons
    }

    /// The row of a file the stash entry `hash` changed.
    pub(crate) fn stash_file_cell(
        &self,
        mtm: MainThreadMarker,
        hash: &CommitId,
        path: &Path,
    ) -> Option<Retained<NSView>> {
        let change = self
            .read()
            .stash_changes
            .get(hash)?
            .iter()
            .find(|f| f.path == path)?
            .change;
        let mark = super::rows::change_mark(change);
        let cell = NSTableCellView::new(mtm);
        let (text, name) = file_text(mtm, path, mark == super::model::Mark::Deleted);
        let letter = letter_label(mtm, mark);
        for view in [&*text as &NSView, &letter] {
            cell.addSubview(view);
            center_y(view, &cell);
        }
        activate([
            text.leadingAnchor().constraintEqualToAnchor(&cell.leadingAnchor()),
            letter.trailingAnchor().constraintEqualToAnchor(&cell.trailingAnchor()),
            text.trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&letter.leadingAnchor(), -scaled(4.0)),
        ]);
        let described = tf("{}, {}", &[&path.display().to_string(), mark.word()]);
        cell.setToolTip(Some(&ns(&described)));
        cell.setAccessibilityLabel(Some(&ns(&described)));
        let key = RowKey::StashFile(hash.clone(), path.to_path_buf());
        let custom = self.custom_actions(&key, &menu(std::slice::from_ref(&key)));
        cell.setAccessibilityCustomActions(Some(&NSArray::from_retained_slice(&custom)));
        // SAFETY: The label is a subview of the cell, which keeps it.
        unsafe { cell.setTextField(Some(&name)) };
        Some(cell.into_super())
    }

    /// Reads what the stash entry `hash` changed, once; an open row then lists the files.
    pub(crate) fn read_stash_changes(&self, hash: &CommitId) {
        {
            let mut state = self.state();
            if state.stash_changes.contains_key(hash) || !state.reading_stashes.insert(hash.clone()) {
                return;
            }
        }
        let Some(repo) = self.repo() else {
            self.state().reading_stashes.remove(hash);
            return;
        };
        let (id, theirs, mine) = (self.id(), hash.clone(), hash.clone());
        let queued = work::local(
            move || repo.stash_changes(&theirs),
            move |result| with_panel(id, |panel| panel.stash_changes_read(&mine, result)),
        );
        if !queued {
            self.state().reading_stashes.remove(hash);
        }
    }

    fn stash_changes_read(&self, hash: &CommitId, result: Option<term_git::Result<Vec<term_git::ChangedFile>>>) {
        self.state().reading_stashes.remove(hash);
        let files = match result {
            Some(Ok(files)) => files,
            Some(Err(error)) => return self.report(&error),
            None => return,
        };
        if !self.state().note_stash_changes(hash, files) {
            return;
        }
        let item = self.item_for(&RowKey::Stash(hash.clone()));
        let outline = &self.ui().outline;
        // SAFETY: The item is one the data source hands to this view, kept alive by the panel.
        let row = unsafe { outline.rowForItem(Some(&item)) };
        if row < 0 {
            return;
        }
        // SAFETY: The item comes from the panel's item cache, which the outline view already knows and the panel keeps alive.
        unsafe { outline.reloadItem_reloadChildren(Some(&item), true) };
        if self.read().open_stashes.contains(hash) {
            // SAFETY: The same cached item, which the reload just above kept in the view.
            unsafe { outline.expandItem(Some(&item)) };
        }
    }

    /// Shows what the stash entry `hash` changed in `file` in a diff tab.
    pub(crate) fn show_stash_diff(&self, hash: &CommitId, file: &Path, preview: bool) {
        let (Some(controller), Some(repo)) = (self.controller(), self.repo()) else {
            return;
        };
        let name = self.read().model.stash(hash).map(name_of).unwrap_or_default();
        super::diff::show_stash(&controller, repo.root(), hash, &name, file, preview);
    }

    /// The stash entries among `keys`, or the one a file among them belongs to.
    fn stash_of(&self, keys: &[RowKey]) -> Option<Stash> {
        let state = self.read();
        keys.iter().find_map(|key| match key {
            RowKey::Stash(hash) | RowKey::StashFile(hash, _) => state.model.stash(hash).cloned(),
            _ => None,
        })
    }

    /// Whether `keys` name a stash entry the menus can act on.
    pub(crate) fn has_stash(&self, keys: &[RowKey]) -> bool {
        self.stash_of(keys).is_some()
    }

    /// Offers to save the open documents below the repository with unsaved changes, then runs `then`; nothing runs
    /// when the user cancels or a document could not be saved.
    fn before_stash(&self, then: impl FnOnce(&GitPanel) + 'static) {
        let Some(root) = self.repo().map(|r| r.root().to_path_buf()) else {
            return;
        };
        let unsaved = crate::host::host().unsaved_below(&root);
        if unsaved.is_empty() {
            return then(self);
        }
        let Some(window) = self.controller().and_then(|c| c.window()) else {
            return;
        };
        let id = self.id();
        let names: Vec<String> = unsaved.iter().map(|p| shown(&root, p)).collect();
        sheets::save_first(&window, &names, move || {
            crate::host::host().save_below(
                &root,
                Box::new(move |saved| {
                    with_panel(id, |panel| {
                        if saved {
                            then(panel);
                        } else {
                            panel.set_problem(Some(Problem::plain(t(
                                "Nicht alle Dokumente ließen sich sichern; der Stash wartet.",
                            ))));
                        }
                    });
                }),
            );
        });
    }

    /// "Stash …": asks for the message and whether untracked files go too, then parks the changes.
    pub(crate) fn ask_stash(&self) {
        self.before_stash(|panel| {
            let Some(window) = panel.controller().and_then(|c| c.window()) else {
                return;
            };
            let id = panel.id();
            sheets::stash_options(&window, move |message, untracked| {
                with_panel(id, |panel| {
                    panel.stash_now(StashOptions {
                        message,
                        untracked,
                        staged_only: false,
                    });
                });
            });
        });
    }

    /// "Nur Vorgemerktes stashen": parks what is staged and leaves the rest.
    pub(crate) fn stash_staged(&self) {
        if self.read().model.staged.is_empty() {
            self.set_problem(Some(Problem::plain(t("Nichts vorgemerkt."))));
            return;
        }
        self.before_stash(|panel| {
            panel.stash_now(StashOptions {
                staged_only: true,
                ..StashOptions::default()
            });
        });
    }

    fn stash_now(&self, options: StashOptions) {
        let Some(repo) = self.repo() else { return };
        if !self.begin(Task::Stash, None) {
            return;
        }
        let id = self.id();
        work::long(
            move || repo.stash(&options),
            move |result| {
                with_panel(id, |panel| {
                    panel.end();
                    panel.editors_follow();
                    match result {
                        Some(Ok(true)) => {}
                        Some(Ok(false)) => panel.set_problem(Some(Problem::plain(t("Keine Änderungen zum Stashen.")))),
                        Some(Err(error)) => panel.report(&error),
                        None => panel.set_problem(Some(Problem::plain(t("Git ist unerwartet abgebrochen.")))),
                    }
                    panel.start_refresh();
                });
            },
        );
    }

    /// Applies the stash entry among `keys`, keeping or removing it as `how` says.
    pub(crate) fn apply_stash(&self, keys: &[RowKey], how: term_git::Unstash) {
        let Some(stash) = self.stash_of(keys) else { return };
        self.before_stash(move |panel| {
            let Some(repo) = panel.repo() else { return };
            if !panel.begin(Task::Stash, None) {
                return;
            }
            let id = panel.id();
            work::long(
                move || repo.apply_stash(&stash, how),
                move |result| with_panel(id, |panel| panel.stash_applied(result)),
            );
        });
    }

    fn stash_applied(&self, result: Option<term_git::Result<Applied>>) {
        self.end();
        self.editors_follow();
        match result {
            Some(Ok(Applied::Clean)) => self.set_problem(None),
            Some(Ok(Applied::Conflicts(_))) => {}
            Some(Err(error)) => self.report(&error),
            None => self.set_problem(Some(Problem::plain(t("Git ist unerwartet abgebrochen.")))),
        }
        self.start_refresh();
    }

    /// Asks, then deletes the stash entry among `keys` for good.
    pub(crate) fn drop_stash(&self, keys: &[RowKey]) {
        let (Some(stash), Some(window)) = (self.stash_of(keys), self.controller().and_then(|c| c.window())) else {
            return;
        };
        let (words, _) = title_of(&stash);
        let id = self.id();
        sheets::confirm_delete(
            &window,
            &tf("Stash „{}“ löschen?", &[&words]),
            t("Seine Änderungen gehen verloren. Das lässt sich nicht widerrufen."),
            t("Löschen"),
            move || {
                with_panel(id, |panel| {
                    panel.before_stash(move |panel| {
                        panel.run_stash(move |repo| repo.destructive().drop_stash(&stash));
                    });
                });
            },
        );
    }

    /// "Fertig": the conflicts of the stash apply count as resolved.
    pub(crate) fn finish_stash_conflict(&self) {
        self.before_stash(|panel| panel.run_stash(term_git::Repository::finish_stash_apply));
    }

    /// Asks, then puts back the state from before the stash apply that met conflicts.
    pub(crate) fn abort_stash_conflict(&self) {
        let Some(window) = self.controller().and_then(|c| c.window()) else {
            return;
        };
        let id = self.id();
        sheets::confirm_delete(
            &window,
            t("Anwenden des Stashs abbrechen?"),
            t(
                "Git stellt den Stand von vor dem Anwenden wieder her; deine bisherigen Konfliktlösungen gehen verloren. Der Stash bleibt erhalten.",
            ),
            t("Anwenden abbrechen"),
            move || {
                with_panel(id, |panel| {
                    panel.before_stash(|panel| panel.run_stash(|repo| repo.destructive().abort_stash_apply()));
                });
            },
        );
    }

    /// Lets the open editors below the repository take what a stash action changed on disk.
    fn editors_follow(&self) {
        if let Some(repo) = self.repo() {
            crate::host::host().check_disk_below(repo.root());
        }
    }

    fn run_stash(&self, operation: impl FnOnce(&term_git::Repository) -> term_git::Result<()> + Send + 'static) {
        let Some(repo) = self.repo() else { return };
        if !self.begin(Task::Stash, None) {
            return;
        }
        let id = self.id();
        work::long(
            move || operation(&repo),
            move |result| {
                with_panel(id, |panel| {
                    panel.end();
                    panel.editors_follow();
                    panel.local_done(result);
                });
            },
        );
    }
}

/// What hovering over an entry tells: its words, branch, place, date and, once read, how many files it changed.
fn details(stash: &Stash, branch: Option<&str>, files: Option<usize>) -> String {
    let (words, _) = title_of(stash);
    let mut text = words;
    text.push_str("\n\n");
    if let Some(branch) = branch {
        text.push_str(&tf("Auf {}", &[branch]));
        text.push_str(" · ");
    }
    text.push_str(&name_of(stash));
    text.push_str(" · ");
    text.push_str(&super::text::ago(now() - stash.time));
    if let Some(files) = files {
        text.push('\n');
        text.push_str(&match files {
            1 => t("1 Datei geändert").to_string(),
            n => tf("{} Dateien geändert", &[&n.to_string()]),
        });
    }
    text
}

/// `path` relative to `root`, as the user knows it.
fn shown(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map_or_else(|_| path.to_path_buf(), PathBuf::from)
        .display()
        .to_string()
}

/// Seconds since 1970.
fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stash(message: &str) -> Stash {
        Stash {
            index: 2,
            hash: CommitId::parse(&"a".repeat(40)).unwrap(),
            message: message.into(),
            time: 0,
        }
    }

    #[test]
    fn an_entry_shows_its_own_words_and_keeps_its_branch_apart() {
        assert_eq!(
            title_of(&stash("On main: half a feature")),
            ("half a feature".to_string(), Some("main".to_string()))
        );
        assert_eq!(
            title_of(&stash("WIP on feature/x: abc1234 Subject: with colon")),
            ("abc1234 Subject: with colon".to_string(), Some("feature/x".to_string()))
        );
        assert_eq!(title_of(&stash("odd")), ("odd".to_string(), None));
        assert_eq!(name_of(&stash("x")), "stash@{2}");
    }
}
