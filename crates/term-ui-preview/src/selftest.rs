//! What the self-test reads of the previews and asks of the decoding service.

use std::path::Path;

use term_ui_editor::EditorSerial;

use super::{find, sync};

/// The text the preview beside `editor` shows, its attachments as U+FFFC, and how many attachments it has.
pub fn text(editor: EditorSerial) -> Option<(String, usize)> {
    let preview = find(|p| p.editor() == editor)?;
    let storage = &preview.views.storage;
    let text = storage.string().to_string();
    let mut attachments = 0;
    let key = term_render::constant!(objc2_app_kit::NSAttachmentAttributeName);
    for (at, _) in text.char_indices().filter(|(_, c)| *c == '\u{FFFC}') {
        let utf16 = text[..at].encode_utf16().count();
        // SAFETY: An index inside the storage and AppKit's attachment key.
        let value = unsafe { storage.attribute_atIndex_effectiveRange(key, utf16, std::ptr::null_mut()) };
        attachments += usize::from(value.is_some());
    }
    Some((text, attachments))
}

/// The line of text at the top of what the preview beside `editor` shows.
pub fn top_line(editor: EditorSerial) -> Option<String> {
    let preview = find(|p| p.editor() == editor)?;
    sync::follow_due(&preview);
    let at = sync::char_at_top(&preview.views)?;
    let units: Vec<u16> = preview.views.storage.string().to_string().encode_utf16().collect();
    let start = units[..at.min(units.len())]
        .iter()
        .rposition(|u| *u == u16::from(b'\n'))
        .map_or(0, |n| n + 1);
    let end = units[start..]
        .iter()
        .position(|u| *u == u16::from(b'\n'))
        .map_or(units.len(), |n| start + n);
    Some(String::from_utf16_lossy(&units[start..end]))
}

/// The size in pixels the service decoded the picture file `path` to, read and decoded as a picture tab does.
pub fn decode_file(path: &Path) -> Option<(u32, u32)> {
    match super::image::decode_file(path)? {
        splat_decoder::Decoded::Rgba { width, height, .. } => Some((width, height)),
        _ => None,
    }
}

/// Whether the service refused both to read `path` and to open a network connection.
pub fn sandboxed(path: &str) -> Option<bool> {
    Some(!splat_decoder::self_check(false, path)? && !splat_decoder::self_check(true, path)?)
}

/// The processes running a program from the app bundle's XPC services, found by their executable's path.
pub fn service_pids() -> Vec<i32> {
    let services = format!(
        "{}/Contents/XPCServices/",
        objc2_foundation::NSBundle::mainBundle().bundlePath()
    );
    let mut pids = vec![0i32; 4096];
    let size = i32::try_from(pids.len() * std::mem::size_of::<i32>()).unwrap_or(i32::MAX);
    // SAFETY: The buffer holds `size` bytes of pids, which the call fills and counts.
    let count = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
    pids.truncate(usize::try_from(count).unwrap_or(0));
    pids.into_iter()
        .filter(|&pid| path_of(pid).is_some_and(|path| path.starts_with(&services)))
        .collect()
}

fn path_of(pid: i32) -> Option<String> {
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let size = u32::try_from(buffer.len()).ok()?;
    // SAFETY: The buffer holds `size` bytes, into which the call writes the program's path.
    let length = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), size) };
    let length = usize::try_from(length).ok().filter(|&n| n > 0)?;
    Some(String::from_utf8_lossy(&buffer[..length]).into_owned())
}
