//! The pasteboard: copying the selection, pasting and dropping text, files and images (E9), and text programs copy
//! with OSC 52.
//!
//! Files come first, as Finder puts their names and icons on the pasteboard too; text comes before an image, as many
//! apps add a picture of what was copied.

use std::path::PathBuf;

use objc2::ClassType;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeHTML, NSPasteboardTypePNG, NSPasteboardTypeRTF,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardURLReadingFileURLsOnlyKey,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSURL};
use splat_i18n::tf;
use term_session::{PasteAction, Refusal};

use super::view::TerminalView;
use crate::kit::ns;
use term_render::constant;

mod rich;

/// Puts `text` on the general pasteboard.
pub(crate) fn store(text: &str) {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    pasteboard.setString_forType(&ns(text), constant!(NSPasteboardTypeString));
}

pub(crate) fn text(pasteboard: &NSPasteboard) -> Option<String> {
    pasteboard
        .stringForType(constant!(NSPasteboardTypeString))
        .map(|text| text.to_string())
}

/// The files on `pasteboard`, as Finder copies or drags them.
pub(crate) fn files(pasteboard: &NSPasteboard) -> Vec<PathBuf> {
    let classes = NSArray::from_slice(&[NSURL::class()]);
    let yes = NSNumber::numberWithBool(true);
    let value: &AnyObject = &yes;
    let only_files = NSDictionary::from_slices(&[constant!(NSPasteboardURLReadingFileURLsOnlyKey)], &[value]);
    // SAFETY: Pasteboards read NSURL objects, and the option takes a boolean number.
    let urls = unsafe { pasteboard.readObjectsForClasses_options(&classes, Some(&only_files)) };
    let Some(urls) = urls else { return vec![] };
    urls.iter()
        .filter_map(|object| object.downcast::<NSURL>().ok())
        .filter_map(|url| url.path())
        .map(|path| PathBuf::from(path.to_string()))
        .collect()
}

/// The image on `pasteboard` as PNG data; a TIFF, as screenshots come, turns into PNG in splat's sandboxed decoding
/// service (E22), and nothing the pasteboard holds is decoded in the app.
pub(crate) fn png(pasteboard: &NSPasteboard) -> Option<Vec<u8>> {
    if let Some(data) = pasteboard.dataForType(constant!(NSPasteboardTypePNG)) {
        return Some(data.to_vec());
    }
    let tiff = pasteboard.dataForType(constant!(NSPasteboardTypeTIFF))?;
    splat_decoder::png_of_pasted_tiff(&tiff.to_vec())
}

/// Whether `pasteboard` holds something the terminal takes.
pub(crate) fn accepts(pasteboard: &NSPasteboard) -> bool {
    pasteboard
        .availableTypeFromArray(&NSArray::from_slice(&[
            constant!(NSPasteboardTypeString),
            constant!(NSPasteboardTypeFileURL),
            constant!(NSPasteboardTypePNG),
            constant!(NSPasteboardTypeTIFF),
        ]))
        .is_some()
}

impl TerminalView {
    pub(super) fn copy_selection(&self) {
        if let Some(text) = self.selected_text() {
            store(&rich::trimmed(&text));
        }
    }

    /// Copies the selection as plain text and, where the session knows its colours, as RTF and HTML too (F114).
    pub fn copy_formatted(&self) {
        let Some(styled) = self.with_session(term_session::Session::selected_styled).flatten() else {
            return self.copy_selection();
        };
        let plain = styled.plain();
        if plain.is_empty() {
            return;
        }
        let face = super::host().config().face();
        let size = self.font_size();
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        pasteboard.setString_forType(&ns(&plain), constant!(NSPasteboardTypeString));
        let rtf = NSData::with_bytes(rich::rtf(&styled, &face.family, size).as_bytes());
        pasteboard.setData_forType(Some(&rtf), constant!(NSPasteboardTypeRTF));
        pasteboard.setString_forType(
            &ns(&rich::html(&styled, &face.family, size)),
            constant!(NSPasteboardTypeHTML),
        );
    }

    /// Pastes what `pasteboard` holds: files as their paths, text, else an image as `Session::paste_image` decides,
    /// which is returned.
    pub(super) fn paste_from(&self, pasteboard: &NSPasteboard) -> Option<PasteAction> {
        let paths = files(pasteboard);
        if !paths.is_empty() {
            self.with_session(|session| session.paste_paths(&paths));
            return None;
        }
        if let Some(text) = text(pasteboard) {
            self.paste_text(&text);
            return None;
        }
        let png = png(pasteboard)?;
        let dir = term_session::paste::default_image_dir();
        let pasted = self.with_session(|session| session.paste_image(&png, &dir))?;
        match &pasted {
            Ok(PasteAction::Refuse(Refusal::Remote(program))) => self.notify(&tf(
                "Kein Bild eingefügt: „{}“ läuft auf einem anderen Rechner, der den Pfad nicht kennt.",
                &[program],
            )),
            Err(error) => self.notify(&tf(
                "Das Bild ließ sich nicht einfügen: {}",
                &[&term_base::explain::Explain::explain(error)],
            )),
            Ok(_) => {}
        }
        pasted.ok()
    }

    /// Pastes files dropped onto the view as their paths, or dropped text; whether there was either.
    pub(super) fn drop_from(&self, pasteboard: &NSPasteboard) -> bool {
        let paths = files(pasteboard);
        let text = text(pasteboard);
        if paths.is_empty() && text.is_none() {
            return false;
        }
        match text {
            _ if !paths.is_empty() => {
                self.with_session(|session| session.paste_paths(&paths));
            }
            Some(text) => {
                self.paste_text(&text);
            }
            None => {}
        }
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        true
    }
}
