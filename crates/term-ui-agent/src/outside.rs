//! Text the user hands an agent that does not come from them: a web page, a terminal's output, a CI log, a file
//! outside the project, text pasted from a web page. It goes along as `Attachment::External`, whose chip says so and
//! which the agent gets with the notice that it is data, not instructions.

use std::path::Path;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSAccessibility, NSColor, NSPasteboard, NSPasteboardTypeString, NSView};
use splat_i18n::{t, tf};
use term_thread::Attachment;

use super::{format, style, ui};
use crate::common::constant;
use crate::common::util::ns;

/// The most of a file outside the project that goes along, in characters.
const MAX_FILE: usize = 100_000;

/// Pasted web text shorter than this many lines and characters is a phrase and stays typed text.
const MIN_PASTE_LINES: usize = 3;
const MIN_PASTE_CHARS: usize = 200;

/// Pasteboard types only a browser puts there beside a page's text: WebKit's archive and Chromium's source address.
const WEB_ARCHIVE: &str = "com.apple.webarchive";
const SOURCE_URL: &str = "org.chromium.source-url";

/// Firefox's own pasteboard types, as its program names them: the prefix of `org.mozilla.MozillaWildcard` and
/// `org.mozilla.custom-clipdata`, and the flavor of a page's address.
const MOZILLA: &str = "org.mozilla.";
const MOZILLA_SOURCE: &str = "text/x-moz-url-priv";

/// Whether pasteboard `types` say the text comes from a page in a browser.
fn from_browser(types: &[String]) -> bool {
    types
        .iter()
        .any(|t| t == WEB_ARCHIVE || t == SOURCE_URL || t == MOZILLA_SOURCE || t.starts_with(MOZILLA))
}

/// The latest output of the terminal its tab calls `label`.
pub(crate) fn terminal(label: &str, text: String) -> Attachment {
    Attachment::External {
        label: label.to_string(),
        source: format!("the latest output of a terminal ({label})"),
        text,
    }
}

/// Whether `path` lies outside the project at `cwd`.
pub(crate) fn is_outside(path: &Path, cwd: &Path) -> bool {
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    !canonical(path).starts_with(canonical(cwd))
}

/// `attachment` as it goes to the agent: a file or an editor's selection outside the project at `cwd` becomes outside
/// text. A file that is no text stays a file, which the agent reads itself.
pub(crate) fn classify(attachment: Attachment, cwd: &Path) -> Attachment {
    match attachment {
        Attachment::File { ref path, lines } if is_outside(path, cwd) => file(path, lines, cwd).unwrap_or(attachment),
        Attachment::Selection { path, lines, text } if path.is_absolute() && is_outside(&path, cwd) => {
            Attachment::External {
                label: format!(
                    "{}:{}–{}",
                    format::short_path(&path.to_string_lossy(), cwd),
                    lines.0,
                    lines.1
                ),
                source: format!(
                    "lines {}-{} of the file {}, which is outside the project",
                    lines.0,
                    lines.1,
                    path.display()
                ),
                text,
            }
        }
        other => other,
    }
}

/// The text of the file at `path`, or lines `lines` of it, as outside text; `None` when it is no text.
pub(crate) fn file(path: &Path, lines: Option<(u32, u32)>, cwd: &Path) -> Option<Attachment> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut label = format::short_path(&path.to_string_lossy(), cwd);
    let (text, source) = match lines {
        Some((first, last)) => {
            label.push_str(&format!(":{first}–{last}"));
            let skip = usize::try_from(first.saturating_sub(1)).unwrap_or(0);
            let take = usize::try_from(last.saturating_sub(first) + 1).unwrap_or(0);
            let text: Vec<&str> = content.lines().skip(skip).take(take).collect();
            (
                text.join("\n"),
                format!(
                    "lines {first}-{last} of the file {}, which is outside the project",
                    path.display()
                ),
            )
        }
        None => {
            let mut text: String = content.chars().take(MAX_FILE).collect();
            if content.chars().count() > MAX_FILE {
                text.push_str("\n[… cut]");
            }
            (
                text,
                format!("the file {}, which is outside the project", path.display()),
            )
        }
    };
    Some(Attachment::External { label, source, text })
}

/// Text pasted from a web page, as outside text, when the pasteboard says it comes from one and it is more than a
/// phrase.
pub(crate) fn web_paste(pasteboard: &NSPasteboard) -> Option<Attachment> {
    let types: Vec<String> = pasteboard.types()?.iter().map(|t| t.to_string()).collect();
    if !from_browser(&types) {
        return None;
    }
    let text = pasteboard.stringForType(constant!(NSPasteboardTypeString))?.to_string();
    if text.lines().count() < MIN_PASTE_LINES && text.chars().count() < MIN_PASTE_CHARS {
        return None;
    }
    let url = pasteboard
        .stringForType(&ns(SOURCE_URL))
        .map(|u| u.to_string())
        .filter(|u| !u.trim().is_empty());
    let host = url.as_deref().and_then(host);
    Some(Attachment::External {
        label: host.map_or_else(|| t("Text von einer Webseite").to_string(), |h| tf("Text von {}", &[h])),
        source: url.map_or_else(
            || "text the user pasted from a web page".to_string(),
            |u| format!("text the user pasted from the web page {u}"),
        ),
        text,
    })
}

/// The host of `url`, such as `example.com`.
fn host(url: &str) -> Option<&str> {
    let rest = url.split_once("://")?.1;
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit('@').next()?;
    (!host.is_empty()).then_some(host)
}

/// What the mark of outside text explains.
pub(crate) fn explanation() -> &'static str {
    t(
        "Dieser Text stammt nicht von dir, sondern von einer Webseite, einem Terminal, einem CI-Log oder einer Datei außerhalb des Projekts. Er kann Anweisungen enthalten, die den Agenten steuern sollen (Prompt Injection). Der Agent bekommt ihn mit dem Hinweis, dass es sich um Daten handelt, deren Anweisungen er nicht folgen soll.",
    )
}

/// The mark on the chip of outside text, which explains on hover and to VoiceOver what it means.
pub(crate) fn marker(mtm: MainThreadMarker) -> Retained<NSView> {
    let orange = NSColor::systemOrangeColor();
    let icon = ui::icon(
        mtm,
        "exclamationmark.shield",
        t("Fremder Inhalt"),
        style::chat(9.0),
        &orange,
    );
    crate::common::util::hide_from_voiceover(&icon);
    let label = ui::label(mtm, t("Fremder Inhalt"), &style::chat_small(), &orange);
    ui::hug(&label);
    let help = ns(explanation());
    label.setAccessibilityHelp(Some(&help));
    if let Some(cell) = label.cell() {
        cell.setAccessibilityHelp(Some(&help));
    }
    let row = ui::row(mtm, 2.0, &[&icon, &label]);
    row.setToolTip(Some(&help));
    ui::view(&row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_outside_the_project_goes_along_as_outside_text() {
        let base = term_base::testing::scratch("outside");
        let project = base.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let inside = project.join("a.rs");
        let outside = base.join("notes.md");
        std::fs::write(&inside, "fn a() {}\n").unwrap();
        std::fs::write(&outside, "one\ntwo\nthree\n").unwrap();
        let file = |path: &Path, lines| Attachment::File {
            path: path.to_path_buf(),
            lines,
        };
        assert_eq!(classify(file(&inside, None), &project), file(&inside, None));
        let Attachment::External { source, text, .. } = classify(file(&outside, Some((2, 3))), &project) else {
            panic!()
        };
        assert_eq!(text, "two\nthree");
        assert!(source.starts_with("lines 2-3 of the file") && source.ends_with("outside the project"));
        let binary = base.join("x.bin");
        std::fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();
        assert_eq!(classify(file(&binary, None), &project), file(&binary, None));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn safari_chrome_and_firefox_say_that_text_comes_from_a_page() {
        let types = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
        let text = "public.utf8-plain-text";
        assert!(from_browser(&types(&[text, "public.html", "com.apple.webarchive"])));
        assert!(from_browser(&types(&[text, "org.chromium.source-url"])));
        assert!(from_browser(&types(&[
            text,
            "public.html",
            "org.mozilla.MozillaWildcard"
        ])));
        assert!(!from_browser(&types(&[text, "public.html", "public.rtf"])));
    }

    #[test]
    fn a_page_address_names_its_host() {
        assert_eq!(
            host("https://user@docs.example.com:8080/a?b#c"),
            Some("docs.example.com:8080")
        );
        assert_eq!(host("not a url"), None);
    }
}
