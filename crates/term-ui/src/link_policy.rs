//! Which links a ⌘-click opens (F182): a program names any URL in OSC 8, and a scheme another app registered can
//! start that app with arguments the program chose, so only web, mail and local file links open.

use std::path::PathBuf;

use objc2_app_kit::NSWorkspace;
use objc2_foundation::NSURL;
use splat_i18n::tf;

use super::view::TerminalView;
use crate::kit::ns;

/// Where a clicked link goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A web address, for the browser.
    Web(String),
    /// A `mailto:` address, for the mail app.
    Mail(String),
    /// A file on this Mac, opened as a clicked path is.
    File(PathBuf),
    /// A link that is not opened, with its scheme.
    Refused(String),
}

impl TerminalView {
    /// Opens the link `url` a ⌘-click hit, if it is one that opens.
    pub(super) fn open_url(&self, url: &str) {
        match target(url) {
            Target::Web(url) => super::host().opened(self.window_id(), super::Opened::Url(url)),
            Target::Mail(url) => {
                if let Some(url) = NSURL::URLWithString(&ns(&url)) {
                    NSWorkspace::sharedWorkspace().openURL(&url);
                }
            }
            Target::File(path) => super::host().opened(
                self.window_id(),
                super::Opened::File {
                    path,
                    line: 1,
                    column: 1,
                },
            ),
            Target::Refused(scheme) => self.notify(&tf("Links der Art „{}:“ öffnet das Terminal nicht.", &[&scheme])),
        }
    }
}

/// Where clicking `url` goes.
pub(crate) fn target(url: &str) -> Target {
    let scheme = url
        .split_once(':')
        .map_or("", |(scheme, _)| scheme)
        .to_ascii_lowercase();
    match scheme.as_str() {
        "http" | "https" => Target::Web(url.to_string()),
        "mailto" => Target::Mail(url.to_string()),
        "file" => local_file(url).map_or_else(|| Target::Refused("file".into()), Target::File),
        _ => Target::Refused(scheme),
    }
}

/// The path of a `file:` URL whose host is this Mac or empty; a file on another host has no path here.
fn local_file(url: &str) -> Option<PathBuf> {
    let rest = url.get(5..)?.strip_prefix("//")?;
    let (host, path) = rest.split_at(rest.find('/')?);
    let path = PathBuf::from(percent_decode(path.split(['?', '#']).next().unwrap_or(path)));
    let place = term_session::WorkingDirectory {
        host: host.to_string(),
        path,
    };
    place.is_local().then_some(place.path)
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = text
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_mail_and_local_file_links_open() {
        assert_eq!(
            target("https://example.com/a"),
            Target::Web("https://example.com/a".into())
        );
        assert_eq!(target("HTTP://x"), Target::Web("HTTP://x".into()));
        assert_eq!(target("mailto:a@b.example"), Target::Mail("mailto:a@b.example".into()));
        assert_eq!(
            target("file:///Users/me/a%20b.txt"),
            Target::File("/Users/me/a b.txt".into())
        );
        assert_eq!(target("file://localhost/tmp/x"), Target::File("/tmp/x".into()));
        assert_eq!(target("file://elsewhere.example/tmp/x"), Target::Refused("file".into()));
        for refused in [
            "x-apple.systempreferences:com.apple.preference",
            "ssh://evil",
            "javascript:alert(1)",
            "vscode://x",
        ] {
            assert!(matches!(target(refused), Target::Refused(_)), "{refused}");
        }
        assert_eq!(target("no scheme"), Target::Refused(String::new()));
    }
}
