//! A user message as the body of `POST /api/session/{id}/prompt`: the text, images as `data:` URIs where the model
//! takes them, and files and selections as text after the message. OpenCode turns a `data:` URI into an inline
//! attachment itself; a `file://` one would be read by the server, which a file outside the project must not be.

use std::io::Read;
use std::path::Path;

use base64::Engine;
use serde_json::{Value, json};
use term_thread::{Attachment, UserMessage};

/// The largest file embedded whole; a bigger one is named by its path for OpenCode to read itself.
const MAX_EMBEDDED: u64 = 512 * 1024;

/// Lines `first..=last` of `content`, counted from 1.
fn lines_of(content: &str, (first, last): (u32, u32)) -> String {
    content
        .lines()
        .skip(first.saturating_sub(1) as usize)
        .take(last.saturating_sub(first) as usize + 1)
        .collect::<Vec<_>>()
        .join("\n")
}

fn fenced(name: &str, content: &str) -> String {
    let fence = if content.contains("```") { "````" } else { "```" };
    format!("{name}\n{fence}\n{content}\n{fence}")
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

fn file(shown: &Path, full: &Path, lines: Option<(u32, u32)>) -> String {
    let content = small_text(full);
    match (content, lines) {
        (Some(content), Some(range)) => fenced(
            &format!("{}:{}-{}", shown.display(), range.0, range.1),
            &lines_of(&content, range),
        ),
        (Some(content), None) => fenced(&shown.display().to_string(), &content),
        (None, Some((first, last))) => format!("See lines {first}-{last} of {}.", full.display()),
        (None, None) => format!("See {}.", full.display()),
    }
}

/// The prompt body for `message`; `images` says whether the model takes them, `steer` adds it to the running turn.
pub(crate) fn body(message: &UserMessage, images: bool, cwd: &Path, steer: bool) -> Value {
    let mut parts = vec![message.text.clone()];
    let mut files = Vec::new();
    for attachment in &message.attachments {
        match attachment {
            Attachment::Image { name, media_type, data } if images => {
                let encoded = base64::engine::general_purpose::STANDARD.encode(data);
                let mut file = json!({"uri": format!("data:{media_type};base64,{encoded}")});
                if let Some(name) = name {
                    file["name"] = json!(name);
                }
                files.push(file);
            }
            Attachment::Image { .. } => {}
            Attachment::File { path, lines } => parts.push(file(path, &cwd.join(path), *lines)),
            Attachment::Selection {
                path,
                lines,
                text: selected,
            } => parts.push(fenced(&format!("{}:{}-{}", path.display(), lines.0, lines.1), selected)),
            Attachment::External {
                source, text: content, ..
            } => parts.push(term_thread::external_text(source, content)),
        }
    }
    let mut body = json!({"text": parts.join("\n\n")});
    if !files.is_empty() {
        body["files"] = Value::Array(files);
    }
    if steer {
        body["delivery"] = json!("steer");
    }
    body
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn attachments_become_text_and_images_data_uris() {
        let dir = std::env::temp_dir().join(format!("term-opencode-prompt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "one\ntwo\nthree\n").unwrap();
        let message = UserMessage {
            text: "Look".into(),
            attachments: vec![
                Attachment::Image {
                    name: Some("shot.png".into()),
                    media_type: "image/png".into(),
                    data: Arc::from(&b"abc"[..]),
                },
                Attachment::File {
                    path: "a.rs".into(),
                    lines: Some((2, 3)),
                },
                Attachment::Selection {
                    path: "b.rs".into(),
                    lines: (4, 4),
                    text: "let x = 1;".into(),
                },
            ],
            message_id: None,
        };
        let sent = body(&message, true, &dir, false);
        assert_eq!(
            sent["text"],
            "Look\n\na.rs:2-3\n```\ntwo\nthree\n```\n\nb.rs:4-4\n```\nlet x = 1;\n```"
        );
        assert_eq!(
            sent["files"],
            json!([{"uri": "data:image/png;base64,YWJj", "name": "shot.png"}])
        );
        assert!(sent.get("delivery").is_none());
        let steered = body(&message, false, &dir, true);
        assert!(steered.get("files").is_none());
        assert_eq!(steered["delivery"], "steer");
    }

    #[test]
    fn only_a_regular_file_is_embedded() {
        assert_eq!(file(Path::new("zero"), Path::new("/dev/zero"), None), "See /dev/zero.");
        let dir = std::env::temp_dir().join(format!("term-opencode-prompt-big-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(small_text(&dir), None);
        let big = dir.join("big.txt");
        std::fs::write(&big, "x".repeat(usize::try_from(MAX_EMBEDDED).unwrap() + 1)).unwrap();
        assert_eq!(small_text(&big), None);
        let _ = std::fs::remove_dir_all(dir);
    }
}
