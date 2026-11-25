//! What is left of Dango's insets (text shown that is not in the file, such as inlay hints and blame) without them:
//! the line fragments of a layout fragment, which the line numbers, whitespace marks and jumps read.

use objc2::rc::Retained;
use objc2_app_kit::{NSTextLayoutFragment, NSTextLineFragment};
use objc2_foundation::NSArray;

/// The line fragments of `fragment`; none before TextKit laid it out, when it answers nil although its header says it
/// never does.
pub(crate) fn line_fragments(fragment: &NSTextLayoutFragment) -> Option<Retained<NSArray<NSTextLineFragment>>> {
    // SAFETY: `textLineFragments` takes no arguments and returns an array of line fragments, or nil before layout.
    unsafe { objc2::msg_send![fragment, textLineFragments] }
}
