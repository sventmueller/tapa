//! `@` mentions in the composer: a file of the project (`@path`, `@path#5-10`), `@terminal` and `@browser`.

use std::ops::Range;
use std::path::{Path, PathBuf};

use term_thread::Attachment;

/// What a mention names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Mention {
    /// A file, relative to the project folder or absolute, whole or some of its lines.
    File { path: String, lines: Option<(u32, u32)> },
    /// The output of the terminal in view.
    Terminal,
    /// The page of the browser tab in view.
    Browser,
}

/// The words that stand for something other than a file.
pub(crate) const TERMINAL: &str = "terminal";
pub(crate) const BROWSER: &str = "browser";

/// The mention the user types at `caret`, a byte offset: its range in `text` and what follows the `@` up to a `#`.
pub(crate) fn typing(text: &str, caret: usize) -> Option<(Range<usize>, String)> {
    let before = text.get(..caret)?;
    let start = before
        .rfind(char::is_whitespace)
        .map_or(0, |n| n + before[n..].chars().next().map_or(1, char::len_utf8));
    let token = &before[start..];
    let rest = token.strip_prefix('@')?;
    if rest.starts_with('"') {
        return None;
    }
    let end = text[caret..]
        .find(char::is_whitespace)
        .map_or(text.len(), |n| caret + n);
    let query = rest.split('#').next().unwrap_or_default().to_string();
    Some((start..end, query))
}

/// How a mention of `path` is written: quoted when it holds a space, as the CLI reads it.
pub(crate) fn written(path: &str) -> String {
    if path.contains(char::is_whitespace) {
        format!("@\"{path}\"")
    } else {
        format!("@{path}")
    }
}

/// Every mention in `text`, with its range.
pub(crate) fn parse(text: &str) -> Vec<(Range<usize>, Mention)> {
    let mut found = Vec::new();
    let mut at = 0;
    while let Some(offset) = text[at..].find('@') {
        let start = at + offset;
        at = start + 1;
        if text[..start].chars().next_back().is_some_and(|c| !c.is_whitespace()) {
            continue;
        }
        let rest = &text[start + 1..];
        let (name, used) = match rest.strip_prefix('"') {
            Some(quoted) => match quoted.find('"') {
                Some(close) => (&quoted[..close], close + 2),
                None => continue,
            },
            None => {
                let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
                let token = &rest[..end];
                let name = token.split('#').next().unwrap_or_default();
                // Punctuation after a mention belongs to the sentence.
                let name = if token.contains('#') {
                    name
                } else {
                    name.trim_end_matches([',', '.', ';', ':', ')', '!', '?'])
                };
                (name, name.len())
            }
        };
        let after = &rest[used..];
        let (lines, extra) = match after.strip_prefix('#') {
            Some(spec) => {
                let end = spec
                    .find(|c: char| !(c.is_ascii_digit() || c == '-' || c == '–'))
                    .unwrap_or(spec.len());
                (line_range(&spec[..end]), end + 1)
            }
            None => (None, 0),
        };
        let end = start + 1 + used + extra;
        let mention = match name {
            "" => continue,
            TERMINAL => Mention::Terminal,
            BROWSER => Mention::Browser,
            path => Mention::File {
                path: path.to_string(),
                lines,
            },
        };
        found.push((start..end, mention));
        at = end;
    }
    found
}

/// `5-10` or `5`, counted from 1.
fn line_range(spec: &str) -> Option<(u32, u32)> {
    let mut parts = spec.splitn(2, ['-', '–']);
    let first: u32 = parts.next()?.trim().parse().ok()?;
    let last: u32 = match parts.next() {
        Some(last) => last.trim().parse().ok()?,
        None => first,
    };
    (first >= 1 && last >= first).then_some((first, last))
}

/// `text` with the line ranges of file mentions taken out and handed over as attachments, since the CLI's own
/// mentions name whole files; a whole file stays a mention, which the CLI reads itself. A file outside the project
/// goes along as outside text and its mention as a plain path, so the agent does not read it unmarked.
pub(crate) fn resolve_files(text: &str, cwd: &Path) -> (String, Vec<Attachment>) {
    let mut out = String::new();
    let mut attachments = Vec::new();
    let mut copied = 0;
    for (range, mention) in parse(text) {
        let Mention::File { path, lines } = mention else {
            continue;
        };
        let full: PathBuf = if Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            cwd.join(&path)
        };
        if !full.is_file() {
            continue;
        }
        let outside = super::outside::is_outside(&full, cwd);
        let attachment = if outside {
            super::outside::file(&full, lines, cwd)
        } else {
            lines.map(|lines| Attachment::File {
                path: full,
                lines: Some(lines),
            })
        };
        let Some(attachment) = attachment else {
            continue;
        };
        out.push_str(&text[copied..range.start]);
        out.push_str(&if outside { path } else { written(&path) });
        copied = range.end;
        attachments.push(attachment);
    }
    out.push_str(&text[copied..]);
    (out, attachments)
}

/// Whether `text` mentions `which`.
pub(crate) fn mentions(text: &str, which: &Mention) -> bool {
    parse(text).iter().any(|(_, m)| m == which)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mention_is_found_while_it_is_typed() {
        let text = "look at @src/ma and";
        let caret = "look at @src/ma".len();
        assert_eq!(typing(text, caret), Some((8..15, "src/ma".to_string())));
        assert_eq!(typing("mail@example", 12), None);
        assert_eq!(typing("plain", 5), None);
        assert_eq!(typing("@a.rs#5-", 8), Some((0..8, "a.rs".to_string())));
    }

    #[test]
    fn mentions_name_files_with_lines_terminal_and_browser() {
        let found: Vec<Mention> = parse("see @src/a.rs#5-10, @\"my file.md\" and @terminal plus @browser; not a@b")
            .into_iter()
            .map(|(_, m)| m)
            .collect();
        assert_eq!(
            found,
            [
                Mention::File {
                    path: "src/a.rs".into(),
                    lines: Some((5, 10))
                },
                Mention::File {
                    path: "my file.md".into(),
                    lines: None
                },
                Mention::Terminal,
                Mention::Browser,
            ]
        );
    }

    #[test]
    fn a_line_range_becomes_an_attachment_and_the_mention_stays_whole() {
        let dir = term_base::testing::scratch("mention");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/a.rs"), "fn a() {}\n").unwrap();
        let (text, attachments) = resolve_files("fix @src/a.rs#2-3 and @src/a.rs please", &dir);
        assert_eq!(text, "fix @src/a.rs and @src/a.rs please");
        assert_eq!(
            attachments,
            [Attachment::File {
                path: dir.join("src/a.rs"),
                lines: Some((2, 3))
            }]
        );
    }

    #[test]
    fn a_file_outside_the_project_goes_along_as_outside_text() {
        let base = term_base::testing::scratch("mention-outside");
        std::fs::create_dir_all(base.join("project")).unwrap();
        let notes = base.join("notes.md");
        std::fs::write(&notes, "one\ntwo\n").unwrap();
        let typed = format!("read @{}#2 now", notes.display());
        let (text, attachments) = resolve_files(&typed, &base.join("project"));
        assert_eq!(text, format!("read {} now", notes.display()));
        assert!(matches!(&attachments[..], [Attachment::External { text, .. }] if text == "two"));
    }
}
