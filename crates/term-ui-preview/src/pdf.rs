//! PDF files in a tab of their own, shown by PDFKit, which parses them in the app as Preview.app does.

use std::path::Path;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send};
use objc2_app_kit::{NSAccessibility, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSURL};
use splat_i18n::tf;

use term_ui::kit::ns;

// PDFKit has no crate of its own here; its classes are looked up by name once the framework is loaded.
#[link(name = "PDFKit", kind = "framework")]
unsafe extern "C" {}

pub(crate) fn is_pdf(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
}

/// A `PDFView` showing the file, scaled to its width; `None` when PDFKit cannot read it.
pub(crate) fn view(_mtm: MainThreadMarker, path: &Path, name: &str) -> Option<Retained<NSView>> {
    let document_class = AnyClass::get(c"PDFDocument")?;
    let view_class = AnyClass::get(c"PDFView")?;
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    // SAFETY: PDFDocument's class answers `alloc`, and `initWithURL:` takes a file URL and answers nil for a file
    // that is no PDF.
    let document: Option<Retained<AnyObject>> = unsafe {
        let allocated: Allocated<AnyObject> = msg_send![document_class, alloc];
        msg_send![allocated, initWithURL: &*url]
    };
    let document = document?;
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
    // SAFETY: PDFView is an NSView, made on the main thread, which `_mtm` proves, with NSView's initializer; the
    // optional result covers an initializer that answers nil.
    let view: Option<Retained<NSView>> = unsafe {
        let allocated: Allocated<NSView> = msg_send![view_class, alloc];
        msg_send![allocated, initWithFrame: frame]
    };
    let view = view?;
    // SAFETY: PDFView's own setters, with a flag and a PDFDocument.
    unsafe {
        let _: () = msg_send![&*view, setAutoScales: true];
        let _: () = msg_send![&*view, setDocument: &*document];
    }
    view.setAccessibilityLabel(Some(&ns(&tf("PDF: {}", &[name]))));
    Some(view)
}
