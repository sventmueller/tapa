//! Pictures in a tab of their own: read off the main thread and decoded by splat's service in its sandbox (E22), so no
//! byte of an untrusted file reaches a decoder inside the app.

use std::path::Path;

use dispatch2::MainThreadBound;
use objc2::MainThreadMarker;
use objc2::rc::{Retained, Weak};
use objc2_app_kit::{NSAccessibility, NSColor, NSImageScaling, NSImageView, NSTextField, NSView};
use objc2_foundation::NSSize;
use splat_i18n::{t, tf};
use term_ui::kit::{activate, auto_layout, center_in, label, main_soon, ns, pin_edges};

/// What the service decodes, by extension; ICNS, ICO and PSD stay with Quick Look, which reads them out of process.
const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "heic", "heif", "avif",
];

pub(crate) fn is_image(path: &Path) -> bool {
    super::has_extension(path, EXTENSIONS)
}

/// A view that shows the picture of `path` once the service decoded it, scaled down to fit, or says it cannot.
pub(crate) fn view(mtm: MainThreadMarker, path: &Path, name: &str) -> Retained<NSView> {
    let page = NSView::new(mtm);
    let image = NSImageView::new(mtm);
    image.setImageScaling(NSImageScaling::ScaleProportionallyDown);
    image.setAccessibilityLabel(Some(&ns(&tf("Bild: {}", &[name]))));
    let note = label(mtm, t("Lädt …"));
    note.setTextColor(Some(&NSColor::secondaryLabelColor()));
    page.addSubview(&image);
    page.addSubview(&note);
    auto_layout(&[&image, &note]);
    pin_edges(&image, &page);
    activate(center_in(&note, &page));
    let shown = MainThreadBound::new(Shown(Weak::from(&*image), Weak::from(&*note)), mtm);
    let path = path.to_path_buf();
    let _ = std::thread::Builder::new()
        .name("picture decode".into())
        .spawn(move || {
            let decoded = decode_file(&path);
            main_soon(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    shown.into_inner(mtm).show(decoded);
                }
            });
        });
    page
}

/// The views a decoded picture goes to, held weakly while the decode runs.
struct Shown(Weak<NSImageView>, Weak<NSTextField>);

impl Shown {
    fn show(self, decoded: Option<splat_decoder::Decoded>) {
        let (Some(image), Some(note)) = (self.0.load(), self.1.load()) else {
            return;
        };
        let natural = match &decoded {
            Some(splat_decoder::Decoded::Rgba { natural, .. }) => Some(*natural),
            _ => None,
        };
        match decoded.and_then(splat_decoder::image) {
            Some(picture) => {
                if let Some((width, height)) = natural {
                    picture.setSize(NSSize::new(f64::from(width), f64::from(height)));
                }
                image.setImage(Some(&picture));
                note.setHidden(true);
            }
            None => note.setStringValue(&ns(t("Das Bild lässt sich nicht anzeigen."))),
        }
    }
}

/// The picture of `path` as the service decoded it; `None` for a file too large, unreadable or no picture.
pub(crate) fn decode_file(path: &Path) -> Option<splat_decoder::Decoded> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > splat_decoder::MAX_FILE_BYTES as u64 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    let fit = splat_decoder::Fit::within(splat_decoder::SHOWN_SIDE);
    if splat_render::images::native_only(path, &bytes) {
        splat_decoder::decode_native_to(&bytes, fit)
    } else {
        splat_decoder::decode_to(&bytes, fit)
    }
}
