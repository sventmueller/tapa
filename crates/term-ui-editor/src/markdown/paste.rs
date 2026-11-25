//! Pasting into a Markdown document as splat does: what Word or a web page put on the pasteboard as HTML or RTF
//! becomes Markdown, pictures it carries along land as files beside the document; plain text stays as it is.

use std::path::Path;

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeHTML, NSPasteboardTypeRTF, NSPasteboardTypeString};
use splat_i18n::t;
use splat_import::{Converted, ImageSource, Pasted, Placement, paste_as_markdown};
use splat_markdown::commands::{self, Edit, link_destination};

use super::{apply, doc_dir, text_and_selection};
use crate::EditorTextView;
use crate::util::constant;

/// Pastes the rich text on `pasteboard` into `view` as Markdown; whether there was rich text to paste.
pub(crate) fn rich(view: &EditorTextView, pasteboard: &NSPasteboard) -> bool {
    let read = |kind| pasteboard.dataForType(kind).map(|data| data.to_vec());
    let html = read(constant!(NSPasteboardTypeHTML)).map(|data| String::from_utf8_lossy(&data).into_owned());
    let rtf = read(constant!(NSPasteboardTypeRTF));
    if html.is_none() && rtf.is_none() {
        return false;
    }
    let plain = pasteboard
        .stringForType(constant!(NSPasteboardTypeString))
        .map(|s| s.to_string());
    let pasted = Pasted {
        html: html.as_deref(),
        rtf: rtf.as_deref(),
        plain: plain.as_deref(),
    };
    let folder = doc_dir(view);
    let converted = paste_as_markdown(&pasted, &mut |image| match image {
        ImageSource::Address(address) => Some((*address).to_string()),
        ImageSource::Data { mime, bytes } => folder.as_deref().and_then(|dir| write_picture(dir, mime, bytes)),
    });
    let Some(Converted { markdown, placement }) = converted else {
        return false;
    };
    let (text, sel) = text_and_selection(view);
    let edit = if placement == Placement::OwnLines {
        commands::insert_block(&text, sel, &markdown, markdown.len())
    } else {
        let end = sel.start + markdown.len();
        Edit {
            range: sel,
            replacement: markdown,
            selection: end..end,
        }
    };
    apply(view, &edit, t("Als Markdown einsetzen"));
    true
}

/// Pastes the plain text of the pasteboard, whatever else it holds.
pub(crate) fn plain(view: &EditorTextView) {
    // SAFETY: NSTextView's own action, sent with no sender, as a menu without a target would send it.
    unsafe { view.pasteAsPlainText(None) };
}

/// Writes a picture a paste carries along beside the document; the address it has from there.
fn write_picture(dir: &Path, mime: &str, bytes: &[u8]) -> Option<String> {
    let extension = match mime {
        "image/png" => "png",
        "image/jpeg" => "jpeg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    };
    let written = super::files::write_new_file(dir, t("Bild"), extension, bytes).ok()?;
    let name = written.file_name()?.to_string_lossy().into_owned();
    Some(link_destination(&name))
}

#[cfg(test)]
mod tests {
    use super::{link_destination, write_picture};

    #[test]
    fn a_pasted_picture_lands_beside_the_document_under_a_name_not_taken() {
        let scratch = term_base::testing::scratch("paste-pictures");
        let dir = scratch.path();
        assert_eq!(
            write_picture(dir, "image/png", b"one"),
            Some(link_destination("Bild.png"))
        );
        assert_eq!(
            write_picture(dir, "image/png", b"two"),
            Some(link_destination("Bild 2.png"))
        );
        assert_eq!(std::fs::read(dir.join("Bild.png")).unwrap(), b"one");
        assert_eq!(std::fs::read(dir.join("Bild 2.png")).unwrap(), b"two");
        for (mime, name) in [
            ("image/jpeg", "Bild.jpeg"),
            ("image/gif", "Bild.gif"),
            ("image/webp", "Bild.webp"),
        ] {
            assert_eq!(write_picture(dir, mime, b"x"), Some(link_destination(name)));
        }
    }

    #[test]
    fn a_picture_of_a_kind_markdown_cannot_show_is_left_out() {
        let scratch = term_base::testing::scratch("paste-tiff");
        let dir = scratch.path();
        assert_eq!(write_picture(dir, "image/tiff", b"x"), None);
        assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0);
    }
}
