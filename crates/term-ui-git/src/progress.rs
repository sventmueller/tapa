//! The branch popup, the distance to the upstream, and what the panel shows while something runs.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use objc2::MainThreadOnly;
use objc2_app_kit::{NSAccessibility, NSControlStateValueOn, NSMenuItem};
use splat_i18n::{t, tf};
use term_git::{Operation, Progress};

use super::model::Task;
use super::panel::{GitPanel, with_panel};
use term_ui::kit::{item, main_soon, ns, separator, set_target};

/// How often progress reaches the main thread at most, unless git moves to another phase.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// The tag of the menu item that asks for a new branch.
pub(crate) const NEW_BRANCH_TAG: isize = 1;
/// The tag of the menu item that asks for a new worktree.
pub(crate) const NEW_WORKTREE_TAG: isize = 2;

/// Hands git's progress to the panel `id`, at most every `PROGRESS_INTERVAL`.
pub(crate) fn sink(id: u64) -> impl FnMut(&Progress) {
    let mut last: Option<(Instant, String)> = None;
    move |progress: &Progress| {
        let now = Instant::now();
        let due = last
            .as_ref()
            .is_none_or(|(at, phase)| *phase != progress.phase || now.duration_since(*at) >= PROGRESS_INTERVAL);
        if !due {
            return;
        }
        last = Some((now, progress.phase.clone()));
        let progress = progress.clone();
        main_soon(move || with_panel(id, |panel| panel.show_progress(&progress)));
    }
}

impl GitPanel {
    /// The branch popup with local branches, remote ones without a local branch, "New Branch" and "New Worktree";
    /// the distance to the upstream; which remote operations make sense.
    pub(crate) fn update_header(&self) {
        let ui = self.ui();
        let state = self.read();
        let mtm = self.mtm();
        let popup = &ui.branch;
        popup.removeAllItems();
        let Some(menu) = popup.menu() else { return };
        let current = state.branch.head.clone();
        if current.is_none() {
            let short = state.branch.commit.as_ref().map_or("", term_git::CommitId::short);
            let detached = item(mtm, &tf("Losgelöst bei {}", &[short]), None, "");
            detached.setEnabled(false);
            menu.addItem(&detached);
        }
        let locals: Vec<&str> = state
            .branches
            .iter()
            .filter(|b| !b.remote)
            .map(|b| b.name.as_str())
            .collect();
        let mut names: Vec<&str> = locals.clone();
        if let Some(head) = current.as_deref()
            && !names.contains(&head)
        {
            names.insert(0, head);
        }
        for name in &names {
            menu.addItem(&self.branch_item(name, name));
        }
        let local: HashSet<&str> = locals.iter().copied().collect();
        let remotes: Vec<&str> = state
            .branches
            .iter()
            .filter(|b| b.remote && !b.name.ends_with("/HEAD"))
            .map(|b| b.name.as_str())
            .filter(|name| name.split_once('/').is_none_or(|(_, short)| !local.contains(short)))
            .collect();
        if !remotes.is_empty() {
            menu.addItem(&separator(mtm));
            let header = item(mtm, t("Remote-Branches"), None, "");
            header.setEnabled(false);
            menu.addItem(&header);
            for name in remotes {
                menu.addItem(&self.branch_item(name, name));
            }
        }
        menu.addItem(&separator(mtm));
        let new = item(mtm, t("Neuer Branch …"), Some(objc2::sel!(termGitBranch:)), "");
        new.setTag(NEW_BRANCH_TAG);
        set_target(&new, self);
        menu.addItem(&new);
        let tree = item(mtm, t("Neuer Worktree …"), Some(objc2::sel!(termGitBranch:)), "");
        tree.setTag(NEW_WORKTREE_TAG);
        tree.setEnabled(state.branch.commit.is_some());
        set_target(&tree, self);
        menu.addItem(&tree);
        menu.setAutoenablesItems(false);
        let selected = current
            .as_deref()
            .and_then(|name| popup.itemArray().iter().find(|i| i.title().to_string() == name));
        match selected {
            Some(item) => popup.selectItem(Some(&item)),
            None => popup.selectItemAtIndex(0),
        }
        let detached = current.is_none();
        let ahead_behind = state.branch.ahead_behind;
        drop(state);
        self.update_distance(ahead_behind);
        ui.pull.setEnabled(!detached);
        ui.push.setEnabled(!detached);
        self.refresh_toolbar();
    }

    fn branch_item(&self, title: &str, name: &str) -> objc2::rc::Retained<NSMenuItem> {
        let entry = item(self.mtm(), title, Some(objc2::sel!(termGitBranch:)), "");
        // SAFETY: A menu item can represent an object of any type.
        unsafe { entry.setRepresentedObject(Some(&ns(name))) };
        set_target(&entry, self);
        if self.read().branch.head.as_deref() == Some(name) {
            entry.setState(NSControlStateValueOn);
        }
        entry
    }

    fn update_distance(&self, ahead_behind: Option<(u32, u32)>) {
        let distance = &self.ui().distance;
        match ahead_behind {
            Some((ahead, behind)) if ahead > 0 || behind > 0 => {
                distance.setStringValue(&ns(&format!("↑{ahead} ↓{behind}")));
                let spoken = tf(
                    "{} Commits voraus, {} zurück",
                    &[&ahead.to_string(), &behind.to_string()],
                );
                term_ui::kit::set_label(distance, Some(&spoken));
                distance.setToolTip(Some(&ns(&spoken)));
                distance.setHidden(false);
            }
            _ => distance.setHidden(true),
        }
    }

    /// Disables what may not start while something runs and shows the progress row for it.
    pub(crate) fn update_busy(&self) {
        let ui = self.ui();
        let busy = self.read().busy.as_ref().map(|b| (b.task, b.cancel.is_some()));
        let idle = busy.is_none();
        let detached = self.read().branch.head.is_none();
        let stepping = matches!(
            self.read().operation,
            Some(Operation::Rebase | Operation::CherryPick | Operation::Revert)
        );
        ui.fetch.setEnabled(idle);
        ui.commit.setEnabled(idle && !stepping);
        let stash_conflict = self.read().stash_apply.is_some();
        ui.banner
            .resume
            .setEnabled(idle && (stash_conflict || self.read().model.conflicts.is_empty()));
        ui.banner.abort.setEnabled(idle);
        ui.pull.setEnabled(idle && !detached);
        ui.push.setEnabled(idle && !detached);
        ui.branch.setEnabled(idle);
        ui.progress_row.setHidden(idle);
        let Some((task, cancellable)) = busy else {
            // SAFETY: A nil sender is what AppKit's own callers pass.
            unsafe { ui.progress.stopAnimation(None) };
            return;
        };
        ui.cancel.setHidden(!cancellable);
        ui.progress.setIndeterminate(true);
        // SAFETY: A nil sender is what AppKit's own callers pass.
        unsafe { ui.progress.startAnimation(None) };
        let text = match task {
            Task::Commit => t("Commit läuft …"),
            Task::Fetch => t("Fetch läuft …"),
            Task::Pull => t("Pull läuft …"),
            Task::Push => t("Push läuft …"),
            Task::Switch => t("Branch wird gewechselt …"),
            Task::Continue => t("Wird fortgesetzt …"),
            Task::Abort => t("Wird abgebrochen …"),
            Task::Stash => t("Stash läuft …"),
            Task::Worktree => t("Worktree wird angelegt …"),
        };
        ui.progress_label.setStringValue(&ns(text));
        term_ui::kit::set_label(&ui.progress_label, Some(text));
    }

    pub(crate) fn show_progress(&self, progress: &Progress) {
        if self.read().busy.is_none() {
            return;
        }
        let ui = self.ui();
        match progress.percent {
            Some(percent) => {
                ui.progress.setIndeterminate(false);
                ui.progress.setDoubleValue(f64::from(percent));
                ui.progress_label
                    .setStringValue(&ns(&format!("{} {percent} %", progress.phase)));
            }
            None => {
                ui.progress.setIndeterminate(true);
                ui.progress_label.setStringValue(&ns(&progress.phase));
            }
        }
    }

    /// Shows the banner while a merge, rebase or stash apply waits, saying how many conflicts are left.
    pub(crate) fn update_banner(&self) {
        let banner = &self.ui().banner;
        let (operation, stash, open) = {
            let state = self.read();
            (state.operation, state.stash_apply.clone(), state.model.conflicts.len())
        };
        let left = match open {
            1 => t("1 Konflikt ist noch offen.").to_string(),
            n => tf("{} Konflikte sind noch offen.", &[&n.to_string()]),
        };
        if let Some(stash) = stash {
            let mut detail = match open {
                0 => t("Alle Konflikte gelöst. Fertig schließt das Anwenden ab.").to_string(),
                _ => left,
            };
            if stash.pop {
                detail.push(' ');
                detail.push_str(t("Der Stash bleibt erhalten, weil sein Anwenden Konflikte hatte."));
            }
            banner.title.setStringValue(&ns(t("Stash-Konflikt")));
            banner.resume.setTitle(&ns(t("Fertig")));
            banner.abort.setTitle(&ns(t("Abbrechen …")));
            banner.detail.setStringValue(&ns(&detail));
            banner
                .detail
                .setPreferredMaxLayoutWidth(self.ui().text_width(super::ui::BANNER_INSET));
            banner.view.setHidden(false);
            self.update_busy();
            return;
        }
        let Some(operation) = operation else {
            banner.view.setHidden(true);
            return;
        };
        banner.title.setStringValue(&ns(running(operation)));
        banner.resume.setTitle(&ns(t("Fortsetzen")));
        banner.abort.setTitle(&ns(t("Vorgang abbrechen …")));
        let detail = match open {
            0 => t("Alle Konflikte gelöst. Fortsetzen schließt den Vorgang ab.").to_string(),
            _ => left,
        };
        banner.detail.setStringValue(&ns(&detail));
        banner
            .detail
            .setPreferredMaxLayoutWidth(self.ui().text_width(super::ui::BANNER_INSET));
        banner.view.setHidden(false);
        self.update_busy();
    }

    /// The placeholder shows while the message is empty and says what an empty message does when amending.
    pub(crate) fn update_placeholder(&self) {
        let ui = self.ui();
        let amend = ui.amend.state() == NSControlStateValueOn;
        let text = if amend {
            t("Nachricht (leer: bisherige behalten)")
        } else {
            t("Nachricht (⌘↩ zum Committen)")
        };
        ui.placeholder.setStringValue(&ns(text));
        ui.placeholder.setHidden(!ui.message.text().is_empty());
        ui.message.setAccessibilityPlaceholderValue(Some(&ns(text)));
    }
}

// Whole sentences per operation: languages inflect an operation's name differently in each sentence and differently
// from the button that starts it.

/// What the banner says while `operation` waits.
fn running(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => t("Zusammenführung läuft"),
        Operation::Rebase => t("Rebase läuft"),
        Operation::CherryPick => t("Cherry-Pick läuft"),
        Operation::Revert => t("Revert läuft"),
    }
}

/// The question before `operation` is abandoned.
pub(crate) fn abort_question(operation: Operation) -> &'static str {
    match operation {
        Operation::Merge => t("Zusammenführung abbrechen?"),
        Operation::Rebase => t("Rebase abbrechen?"),
        Operation::CherryPick => t("Cherry-Pick abbrechen?"),
        Operation::Revert => t("Revert abbrechen?"),
    }
}
