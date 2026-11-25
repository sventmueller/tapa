//! The self-test's hooks of `highlight.rs`.

use super::*;

/// Whether the lines on screen show the colours the document has for them, for the self-test.
pub(crate) fn colours_painted(view: &EditorTextView) -> bool {
    let visible = visible_lines(view);
    !view.ivars().paint.unpainted.borrow().meets(&visible)
        && !view.ivars().paint.unstyled.borrow().meets(&on_screen(view, &visible))
}

/// The colour shown at UTF-16 offset `at`: the lexing's or the server's where one paints it, else the storage's.
pub(crate) fn shown_color(view: &EditorTextView, at: usize) -> Option<Retained<AnyObject>> {
    let key = constant!(NSForegroundColorAttributeName);
    let (layout, content) = layout_of(view)?;
    let start = content.documentRange().location();
    let location = content.locationFromLocation_withOffset(&start, isize::try_from(at).ok()?)?;
    let found = std::cell::RefCell::new(None);
    let block = block2::RcBlock::new(
        |_: std::ptr::NonNull<NSTextLayoutManager>,
         attributes: std::ptr::NonNull<objc2_foundation::NSDictionary<objc2_foundation::NSString, AnyObject>>,
         range: std::ptr::NonNull<objc2_app_kit::NSTextRange>| {
            crate::util::guarded(objc2::runtime::Bool::NO, || {
                // SAFETY: TextKit hands a valid dictionary and range for the duration of the call.
                let (attributes, range) = unsafe { (attributes.as_ref(), range.as_ref()) };
                let from = content.offsetFromLocation_toLocation(&start, &range.location());
                if usize::try_from(from).is_ok_and(|from| from <= at) {
                    *found.borrow_mut() = attributes.objectForKey(key);
                }
                objc2::runtime::Bool::NO
            })
        },
    );
    layout.enumerateRenderingAttributesFromLocation_reverse_usingBlock(&location, false, &block);
    found.take().or_else(|| {
        let storage = view.storage()?;
        // SAFETY: The caller's index lies inside the text; no effective range is asked for.
        unsafe { storage.attribute_atIndex_effectiveRange(key, at, std::ptr::null_mut()) }
    })
}
