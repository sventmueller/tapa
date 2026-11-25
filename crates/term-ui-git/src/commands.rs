//! The Git menu of the menu bar: every command of the stash also has a menu item, greyed out where it cannot run. The
//! items act on the Git panel of the workspace in front, a stash entry's on the one selected in its list.

use std::cell::OnceCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSMenuItem;
use objc2_foundation::NSObjectProtocol;
use splat_i18n::t;

use super::model::Phase;
use super::model::RowKey;
use super::panel::{self, GitPanel};
use term_ui::kit::{guarded, item, separator, set_target, submenu};
use term_ui::workspace_host::Workspace;

thread_local! {
    static COMMANDS: OnceCell<Retained<GitCommands>> = const { OnceCell::new() };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `GitCommands` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermGitCommands"]
    pub(crate) struct GitCommands;

    unsafe impl NSObjectProtocol for GitCommands {}

    impl GitCommands {
        #[unsafe(method(termGitStash:))]
        fn stash(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(GitPanel::ask_stash));
        }

        #[unsafe(method(termGitStashStaged:))]
        fn stash_staged(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(GitPanel::stash_staged));
        }

        #[unsafe(method(termGitApplyStash:))]
        fn apply(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(|panel| panel.apply_stash(&panel.selected_keys(), term_git::Unstash::Apply)));
        }

        #[unsafe(method(termGitPopStash:))]
        fn pop(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(|panel| panel.apply_stash(&panel.selected_keys(), term_git::Unstash::Pop)));
        }

        #[unsafe(method(termGitDropStash:))]
        fn drop_stash(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(|panel| panel.drop_stash(&panel.selected_keys())));
        }

        #[unsafe(method(termGitFinishStash:))]
        fn finish(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(GitPanel::finish_stash_conflict));
        }

        #[unsafe(method(termGitAbortStash:))]
        fn abort(&self, _sender: Option<&AnyObject>) {
            guarded((), || front(GitPanel::abort_stash_conflict));
        }

        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            guarded(false, || {
                let Some(action) = item.action() else { return false };
                front_panel().is_some_and(|panel| panel.can(action, &panel.selected_keys()))
            })
        }
    }
);

/// The Git menu, between the app's own menus and Window.
pub fn menu(mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let commands = COMMANDS.with(|cell| {
        cell.get_or_init(|| {
            // SAFETY: NSObject's initializer, called once on the newly allocated object.
            unsafe { msg_send![GitCommands::alloc(mtm), init] }
        })
        .clone()
    });
    let entries: [(&str, Option<Sel>); 9] = [
        (t("Stash …"), Some(sel!(termGitStash:))),
        (t("Nur Vorgemerktes stashen"), Some(sel!(termGitStashStaged:))),
        ("", None),
        (t("Stash anwenden"), Some(sel!(termGitApplyStash:))),
        (t("Stash anwenden und entfernen"), Some(sel!(termGitPopStash:))),
        (t("Stash löschen …"), Some(sel!(termGitDropStash:))),
        ("", None),
        (t("Stash-Konflikt abschließen"), Some(sel!(termGitFinishStash:))),
        (t("Anwenden des Stashs abbrechen …"), Some(sel!(termGitAbortStash:))),
    ];
    let items: Vec<Retained<NSMenuItem>> = entries
        .into_iter()
        .map(|(title, action)| match action {
            Some(action) => {
                let entry = item(mtm, title, Some(action), "");
                set_target(&entry, &commands);
                entry
            }
            None => separator(mtm),
        })
        .collect();
    submenu(mtm, "Git", items).0
}

/// The Git panel of the workspace in front.
fn front_panel() -> Option<Retained<GitPanel>> {
    panel::of(&Workspace::frontmost()?)
}

/// Opens the Git panel of the workspace in front, where the command's result appears, and runs `f` on it.
fn front(f: impl FnOnce(&GitPanel)) {
    let Some(workspace) = Workspace::frontmost() else {
        return;
    };
    crate::host::host().show_panel(workspace.id());
    let Some(panel) = GitPanel::attach(&workspace) else {
        return;
    };
    f(&panel);
}

impl GitPanel {
    /// Whether the command `action` can run now, a stash entry's on `keys`.
    pub(crate) fn can(&self, action: Sel, keys: &[RowKey]) -> bool {
        let state = self.read();
        let ready = state.phase == Phase::Ready && state.busy.is_none();
        let changed = !state.model.staged.is_empty() || !state.model.changes.is_empty();
        let staged = !state.model.staged.is_empty();
        let conflict = state.stash_apply.is_some();
        drop(state);
        if action == sel!(termGitStash:) {
            ready && changed
        } else if action == sel!(termGitStashStaged:) {
            ready && staged
        } else if [
            sel!(termGitApplyStash:),
            sel!(termGitPopStash:),
            sel!(termGitDropStash:),
        ]
        .contains(&action)
        {
            ready && self.has_stash(keys)
        } else if action == sel!(termGitFinishStash:) || action == sel!(termGitAbortStash:) {
            ready && conflict
        } else {
            true
        }
    }
}
