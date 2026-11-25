//! The self-test's hooks of `compare.rs`.

use super::*;

/// The text of the comparison tab for `path`, for the self-test.
pub(crate) fn shown(path: &Path) -> Option<String> {
    let key = format!("compare:{}", path.display());
    with_shown(|shown| {
        shown
            .borrow()
            .iter()
            .find(|(k, _)| *k == key)
            .and_then(|(_, view)| view.load())
            .map(|view| view.string().to_string())
    })
}
