//! Icons, sound, films, fonts and office documents in a tab of their own, shown by Quick Look as Finder and
//! Preview.app show them; Quick Look reads them in a process of its own.

use std::path::Path;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::AnyClass;
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{NSAccessibility, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSURL};
use splat_i18n::tf;

use term_ui::kit::ns;

// QLPreviewView lives in Quartz's QuickLookUI and has no crate here; its class is looked up by name.
#[link(name = "Quartz", kind = "framework")]
unsafe extern "C" {}

/// Chosen by name, not by UTType: the system types `.ts` and `.mts` as video and `.key` as Keynote, which here are
/// TypeScript and private keys.
const EXTENSIONS: &[&str] = &[
    "ico", "icns", "psd", "mp3", "wav", "m4a", "aac", "aif", "aiff", "flac", "mp4", "mov", "m4v", "ttf", "otf", "ttc",
    "doc", "docx", "xls", "xlsx", "ppt", "pptx", "pages", "numbers",
];

pub(crate) fn is_quick_look(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// A `QLPreviewView` showing the file; `None` when Quick Look is missing.
pub(crate) fn view(_mtm: MainThreadMarker, path: &Path, name: &str) -> Option<Retained<NSView>> {
    let class = AnyClass::get(c"QLPreviewView")?;
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
    // SAFETY: QLPreviewView is an NSView, made on the main thread, which `_mtm` proves; style 0 is
    // QLPreviewViewStyleNormal, and an NSURL is a QLPreviewItem through Quick Look's category on it.
    let view: Option<Retained<NSView>> = unsafe {
        let allocated: Allocated<NSView> = msg_send![class, alloc];
        msg_send![allocated, initWithFrame: frame, style: 0isize]
    };
    let view = view?;
    // SAFETY: QLPreviewView's own setter, with an NSURL.
    unsafe {
        let _: () = msg_send![&*view, setPreviewItem: &*url];
    }
    view.setAccessibilityLabel(Some(&ns(&tf("Vorschau: {}", &[name]))));
    Some(view)
}
