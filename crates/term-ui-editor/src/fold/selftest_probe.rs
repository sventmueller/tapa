//! What the self-test reads from and does with the parts of `fold.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

impl Gutter {
    /// The folded lines, for the self-test.
    pub(crate) fn folded(&self) -> impl Iterator<Item = usize> + '_ {
        self.folded.iter().copied()
    }
}

/// The lines of `view` that can fold, with whether they are folded, for the self-test.
pub(crate) fn gutter(view: &EditorTextView) -> Gutter {
    let lines = view.document().text().line_count();
    gutter_within(view, 0..lines)
}

/// The lines TextKit lays out, in order, for the self-test: folded ones are missing.
pub(crate) fn laid_out_lines(view: &EditorTextView) -> Vec<usize> {
    use objc2_app_kit::{NSTextLayoutFragment, NSTextLayoutFragmentEnumerationOptions};
    let (Some(layout), Some(content)) = (view.textLayoutManager(), view.textContentStorage()) else {
        return vec![];
    };
    let start = content.documentRange().location();
    let lines = std::cell::RefCell::new(vec![]);
    let block = block2::RcBlock::new(
        |fragment: std::ptr::NonNull<NSTextLayoutFragment>| -> objc2::runtime::Bool {
            crate::util::guarded(objc2::runtime::Bool::NO, || {
                // SAFETY: TextKit keeps the fragment alive for the call.
                let fragment = unsafe { fragment.as_ref() };
                let offset = content.offsetFromLocation_toLocation(&start, &fragment.rangeInElement().location());
                if let Ok(offset) = usize::try_from(offset) {
                    lines.borrow_mut().push(line_of(view, offset));
                }
                objc2::runtime::Bool::YES
            })
        },
    );
    let _ = layout.enumerateTextLayoutFragmentsFromLocation_options_usingBlock(
        Some(&start),
        NSTextLayoutFragmentEnumerationOptions::EnsuresLayout,
        &block,
    );
    drop(block);
    lines.into_inner()
}

/// How many folds are folded, for the self-test.
pub(crate) fn folded(view: &EditorTextView) -> usize {
    view.ivars().folds.borrow().hidden.len()
}
