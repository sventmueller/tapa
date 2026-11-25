//! The self-test's hooks of `diff.rs`.

use super::*;

/// The diff tab showing `key` in `controller`'s workspace.
pub(crate) fn find(controller: &(impl AsWorkspace + ?Sized), key: &str) -> Option<Retained<DiffTab>> {
    let controller = &controller.as_workspace();
    live_tabs().into_iter().find(|tab| {
        let ivars = tab.ivars();
        let own = match &ivars.commit {
            Some(Fixed::Commit(hash)) => commit_key(hash, &ivars.path),
            Some(Fixed::Stash(hash, _)) => stash_key(hash, &ivars.path),
            None => self::key(&ivars.path, ivars.staged),
        };
        own == key && ivars.controller == *controller
    })
}
