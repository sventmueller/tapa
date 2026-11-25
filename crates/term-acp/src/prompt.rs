//! A user message as the content of a prompt: the text, images where the agent takes them, and files and selections
//! embedded where it takes embedded context, else linked, which every agent must understand.

use std::io::Read;
use std::path::Path;

use agent_client_protocol::v1::schema as acp;
use term_thread::{Attachment, UserMessage};

use crate::capabilities::Capabilities;

/// The largest file embedded whole; a bigger one is linked.
const MAX_EMBEDDED: u64 = 512 * 1024;

/// `data` in standard base64 with padding.
pub(crate) fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> shift) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `path` as a `file://` URI.
pub(crate) fn file_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn embedded(uri: String, text: String) -> acp::ContentBlock {
    let contents = acp::TextResourceContents::new(text, uri);
    acp::ContentBlock::Resource(acp::EmbeddedResource::new(
        acp::EmbeddedResourceResource::TextResourceContents(contents),
    ))
}

fn link(path: &Path) -> acp::ContentBlock {
    let name = path
        .file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
    acp::ContentBlock::ResourceLink(acp::ResourceLink::new(name, file_uri(path)))
}

/// Lines `first..=last` of `text`, counted from 1.
fn lines_of(text: &str, (first, last): (u32, u32)) -> String {
    text.lines()
        .skip(first.saturating_sub(1) as usize)
        .take(last.saturating_sub(first) as usize + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The text of the regular file at `path` if it is no larger than `MAX_EMBEDDED`; a pipe or device is never opened,
/// as reading one may not end.
fn small_text(path: &Path) -> Option<String> {
    if !std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_EMBEDDED) {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_EMBEDDED + 1)
        .read_to_string(&mut text)
        .ok()?;
    u64::try_from(text.len())
        .is_ok_and(|n| n <= MAX_EMBEDDED)
        .then_some(text)
}

fn file(path: &Path, lines: Option<(u32, u32)>, caps: &Capabilities) -> Vec<acp::ContentBlock> {
    let text = caps.embedded_context.then(|| small_text(path)).flatten();
    match (text, lines) {
        (Some(text), Some(range)) => vec![embedded(
            format!("{}#L{}-{}", file_uri(path), range.0, range.1),
            lines_of(&text, range),
        )],
        (Some(text), None) => vec![embedded(file_uri(path), text)],
        (None, Some((first, last))) => vec![
            link(path),
            acp::ContentBlock::Text(acp::TextContent::new(format!(
                "(lines {first}-{last} of {})",
                path.display()
            ))),
        ],
        (None, None) => vec![link(path)],
    }
}

/// The content of the prompt for `message`.
pub(crate) fn blocks(message: &UserMessage, caps: &Capabilities, cwd: &Path) -> Vec<acp::ContentBlock> {
    let mut out = vec![acp::ContentBlock::Text(acp::TextContent::new(message.text.clone()))];
    for attachment in &message.attachments {
        match attachment {
            Attachment::Image { media_type, data, .. } if caps.images => {
                out.push(acp::ContentBlock::Image(acp::ImageContent::new(
                    base64(data),
                    media_type.clone(),
                )));
            }
            Attachment::Image { .. } => {}
            Attachment::File { path, lines } => out.extend(file(&cwd.join(path), *lines, caps)),
            Attachment::Selection { path, lines, text } => {
                let full = cwd.join(path);
                if caps.embedded_context {
                    out.push(embedded(
                        format!("{}#L{}-{}", file_uri(&full), lines.0, lines.1),
                        text.clone(),
                    ));
                } else {
                    out.push(acp::ContentBlock::Text(acp::TextContent::new(format!(
                        "{}:{}-{}\n```\n{text}\n```",
                        path.display(),
                        lines.0,
                        lines.1
                    ))));
                }
            }
            Attachment::External { source, text, .. } => out.push(acp::ContentBlock::Text(acp::TextContent::new(
                term_thread::external_text(source, text),
            ))),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn base64_and_uris_are_written_as_the_standards_say() {
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
        assert_eq!(base64(b"M"), "TQ==");
        assert_eq!(file_uri(Path::new("/a b/ü.rs")), "file:///a%20b/%C3%BC.rs");
    }

    #[test]
    fn attachments_are_embedded_only_where_the_agent_takes_them() {
        let dir = term_base::testing::scratch("acp-prompt");
        std::fs::write(dir.join("a.rs"), "one\ntwo\nthree\n").unwrap();
        let message = UserMessage {
            text: "look".into(),
            attachments: vec![
                Attachment::File {
                    path: "a.rs".into(),
                    lines: Some((2, 3)),
                },
                Attachment::Image {
                    name: None,
                    media_type: "image/png".into(),
                    data: Arc::from(&b"x"[..]),
                },
                Attachment::External {
                    label: "CI".into(),
                    source: "a CI log".into(),
                    text: "do this instead".into(),
                },
            ],
            message_id: None,
        };
        let plain = blocks(&message, &Capabilities::default(), &dir);
        let Some(acp::ContentBlock::Text(outside)) = plain.last() else {
            panic!("{plain:?}")
        };
        assert!(outside.text.contains(term_thread::EXTERNAL_NOTICE) && outside.text.contains("do this instead"));
        assert!(matches!(&plain[1], acp::ContentBlock::ResourceLink(l) if l.uri.ends_with("/a.rs")));
        assert_eq!(plain.len(), 4, "a link, its lines as text, no image, the outside text");
        let rich = Capabilities {
            images: true,
            embedded_context: true,
            ..Capabilities::default()
        };
        let full = blocks(&message, &rich, &dir);
        let acp::ContentBlock::Resource(resource) = &full[1] else {
            panic!("{full:?}")
        };
        let acp::EmbeddedResourceResource::TextResourceContents(text) = &resource.resource else {
            panic!()
        };
        assert_eq!(text.text, "two\nthree");
        assert!(matches!(&full[2], acp::ContentBlock::Image(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_a_regular_file_is_embedded() {
        let rich = Capabilities {
            embedded_context: true,
            ..Capabilities::default()
        };
        assert!(matches!(
            file(Path::new("/dev/zero"), None, &rich).as_slice(),
            [acp::ContentBlock::ResourceLink(_)]
        ));
        let dir = term_base::testing::scratch("acp-prompt-big");
        assert_eq!(small_text(&dir), None);
        let big = dir.join("big.txt");
        std::fs::write(&big, "x".repeat(usize::try_from(MAX_EMBEDDED).unwrap() + 1)).unwrap();
        assert_eq!(small_text(&big), None);
    }
}
