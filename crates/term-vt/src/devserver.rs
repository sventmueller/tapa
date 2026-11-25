//! Local web servers announced in a line of output, such as Vite's `Local: http://localhost:5173/`.

use std::sync::LazyLock;

use memchr::memmem::Finder;

/// Host names a dev server announces for this machine, and how the app opens them.
const HOSTS: &[(&str, &str)] = &[
    ("localhost", "localhost"),
    ("127.0.0.1", "127.0.0.1"),
    ("[::1]", "[::1]"),
    ("0.0.0.0", "localhost"),
    ("[::]", "localhost"),
];

/// Phrases servers print before a port, as in `listening on port 3000` or `running at :8080`.
const PHRASES: &[&str] = &[
    "listening on",
    "listening at",
    "running on",
    "running at",
    "serving on",
    "serving at",
    "started on",
    "started at",
    "available on",
    "available at",
];

static SCHEME: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new("://"));
static ON: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(" on "));
static AT: LazyLock<Finder<'static>> = LazyLock::new(|| Finder::new(" at "));

/// The local server addresses in `line`, ready to open.
pub(crate) fn detect(line: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let scheme = SCHEME.find(line).is_some();
    let phrase = has_phrase_end(line);
    if !scheme && !phrase {
        return found;
    }
    let lower = line.to_ascii_lowercase();
    if scheme {
        urls(&lower, line, &mut found);
    }
    if phrase {
        phrases(&lower, &mut found);
    }
    found
}

/// Every phrase ends in " on" or " at"; most lines of output have neither and no scheme either.
fn has_phrase_end(text: &[u8]) -> bool {
    ON.find(text).is_some() || AT.find(text).is_some()
}

/// Whether `text` holds anything `detect` needs in a line to find an address in it; `text` may span lines.
pub(crate) fn may_announce(text: &[u8]) -> bool {
    SCHEME.find(text).is_some() || has_phrase_end(text)
}

/// The key two announcements of the same server share.
pub(crate) fn identity(url: &str) -> &str {
    url.trim_end_matches('/')
}

fn urls(lower: &[u8], line: &[u8], found: &mut Vec<String>) {
    let mut from = 0;
    while let Some(at) = SCHEME.find(&lower[from..]).map(|i| from + i) {
        from = at + 3;
        let scheme = if lower[..at].ends_with(b"https") {
            "https"
        } else if lower[..at].ends_with(b"http") {
            "http"
        } else {
            continue;
        };
        let Some((host, mut end)) = host_at(lower, from) else {
            continue;
        };
        let port = match port_at(lower, end) {
            Some((port, after)) => {
                end = after;
                format!(":{port}")
            }
            None if lower.get(end) == Some(&b':') => continue,
            None => String::new(),
        };
        let path = path_at(line, end);
        found.push(format!("{scheme}://{host}{port}{path}"));
    }
}

/// The host at `at` as the app opens it, and where it ends.
fn host_at(lower: &[u8], at: usize) -> Option<(&'static str, usize)> {
    let rest = &lower[at..];
    let (written, open) = HOSTS.iter().find(|(written, _)| rest.starts_with(written.as_bytes()))?;
    let end = at + written.len();
    let continues = lower
        .get(end)
        .is_some_and(|&b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
    (!continues).then_some((open, end))
}

/// A port after a colon at `at`, and where it ends.
fn port_at(lower: &[u8], at: usize) -> Option<(u16, usize)> {
    if lower.get(at) != Some(&b':') {
        return None;
    }
    digits_at(lower, at + 1)
}

fn digits_at(lower: &[u8], at: usize) -> Option<(u16, usize)> {
    let length = lower[at.min(lower.len())..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    if length == 0 || length > 5 || lower.get(at + length).is_some_and(u8::is_ascii_alphanumeric) {
        return None;
    }
    let text = std::str::from_utf8(&lower[at..at + length]).ok()?;
    text.parse::<u16>()
        .ok()
        .filter(|&port| port > 0)
        .map(|port| (port, at + length))
}

/// The path after the address, without punctuation that ends the sentence around it.
fn path_at(line: &[u8], at: usize) -> String {
    if line.get(at) != Some(&b'/') {
        return String::new();
    }
    let length = line[at..]
        .iter()
        .take_while(|&&b| b > b' ' && !b"\"'<>()[]{}|\\^`".contains(&b))
        .count();
    let path = String::from_utf8_lossy(&line[at..at + length]).into_owned();
    let trimmed = path.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    trimmed.to_string()
}

fn phrases(lower: &[u8], found: &mut Vec<String>) {
    for phrase in PHRASES {
        let mut from = 0;
        while let Some(at) = find(&lower[from..], phrase.as_bytes()).map(|i| from + i) {
            from = at + phrase.len();
            if let Some(port) = port_after_phrase(lower, from) {
                found.push(format!("http://localhost:{port}"));
            }
        }
    }
}

/// The port in `port 3000`, `:3000`, `localhost:3000` or a bare `3000` after a phrase.
fn port_after_phrase(lower: &[u8], at: usize) -> Option<u16> {
    let mut at = skip_spaces(lower, at);
    if lower[at..].starts_with(b"port") {
        at = skip_spaces(lower, at + 4);
        if matches!(lower.get(at), Some(b':' | b'=')) {
            at = skip_spaces(lower, at + 1);
        }
        return digits_at(lower, at).map(|(port, _)| port);
    }
    if let Some((_, end)) = host_at(lower, at) {
        return port_at(lower, end).map(|(port, _)| port);
    }
    if lower[at..].starts_with(b"::") {
        at += 2;
    }
    if lower.get(at) == Some(&b':') {
        return digits_at(lower, at + 1).map(|(port, _)| port);
    }
    // A bare number after a phrase can as well count threads or cores; servers use the unprivileged range.
    digits_at(lower, at).map(|(port, _)| port).filter(|&port| port >= 1024)
}

fn skip_spaces(bytes: &[u8], at: usize) -> usize {
    at + bytes[at.min(bytes.len())..]
        .iter()
        .take_while(|&&b| b == b' ' || b == b'\t')
        .count()
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(haystack, needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls_in(line: &str) -> Vec<String> {
        detect(line.as_bytes())
    }

    #[test]
    fn announcements_of_popular_dev_servers_are_found() {
        assert_eq!(
            urls_in("  ➜  Local:   http://localhost:5173/"),
            ["http://localhost:5173/"]
        );
        assert_eq!(
            urls_in("   - Local:        http://localhost:3000"),
            ["http://localhost:3000"]
        );
        assert_eq!(
            urls_in("* Listening on http://127.0.0.1:3000"),
            ["http://127.0.0.1:3000"]
        );
        assert_eq!(
            urls_in("Starting development server at http://127.0.0.1:8000/"),
            ["http://127.0.0.1:8000/"]
        );
        assert_eq!(
            urls_in("Server running at https://[::1]:8443/app."),
            ["https://[::1]:8443/app"]
        );
        assert_eq!(
            urls_in("Web Server is available at http://localhost:1313/blog/ (bind address 127.0.0.1)"),
            ["http://localhost:1313/blog/"]
        );
    }

    #[test]
    fn addresses_for_all_interfaces_open_as_localhost() {
        assert_eq!(
            urls_in("Serving HTTP on 0.0.0.0 port 8000 (http://0.0.0.0:8000/) ..."),
            ["http://localhost:8000/"]
        );
        assert_eq!(urls_in("listening on http://[::]:4000"), ["http://localhost:4000"]);
    }

    #[test]
    fn listening_phrases_without_a_url_give_a_localhost_address() {
        assert_eq!(
            urls_in("Express server listening on port 3000"),
            ["http://localhost:3000"]
        );
        assert_eq!(urls_in("Listening on :8080"), ["http://localhost:8080"]);
        assert_eq!(urls_in("server started at localhost:9000"), ["http://localhost:9000"]);
        assert_eq!(urls_in("Running on port: 4200"), ["http://localhost:4200"]);
        assert_eq!(urls_in("listening on 5000"), ["http://localhost:5000"]);
    }

    #[test]
    fn other_hosts_and_numbers_are_not_servers() {
        assert!(urls_in("Network: http://192.168.1.20:5173/").is_empty());
        assert!(urls_in("see http://localhost.example.com:80/").is_empty());
        assert!(urls_in("running on 8 cores").is_empty());
        assert!(urls_in("postgres listening on port 99999").is_empty());
        assert!(urls_in("cargo build --release").is_empty());
        assert!(urls_in("http://localhost:abc").is_empty());
    }

    #[test]
    fn a_trailing_slash_does_not_make_a_new_server() {
        assert_eq!(identity("http://localhost:5173/"), identity("http://localhost:5173"));
    }
}
