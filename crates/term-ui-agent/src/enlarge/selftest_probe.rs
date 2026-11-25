//! The self-test's hooks of `enlarge.rs`.

use super::*;

/// The size the open picture takes, for the self-test.
pub(crate) fn shown() -> Option<NSSize> {
    with_shown(|shown| {
        let shade = shown.borrow().clone()?;
        // SAFETY: Asking for the superview only reads.
        unsafe { shade.superview() }?.layoutSubtreeIfNeeded();
        Some(shade.ivars().picture.frame().size)
    })
}
