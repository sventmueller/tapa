//! What the address field's text asks for, and how two addresses compare.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use term_config::SearchEngine;

/// Where typed text leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Address {
    /// A page at this URL.
    Url(String),
    /// A local file; the browser opens it only inside the workspace's folder.
    File(PathBuf),
}

impl Address {
    /// The address a URL has, such as one a page led to.
    pub(crate) fn of_url(url: &str) -> Address {
        match file_path(url).filter(|_| scheme_of(url).as_deref() == Some("file")) {
            Some(path) => Address::File(path),
            None => Address::Url(url.to_string()),
        }
    }

    /// What names the tab that shows it, so opening it again finds the tab: a local server's origin, since a dev
    /// server is one tab whatever page it shows, else the page without its fragment.
    pub(crate) fn key(&self) -> String {
        match self {
            Address::Url(url) => key_of(url),
            Address::File(path) => format!("file://{}", path.display()),
        }
    }
}

/// The page or file `input` names, else a search for its words with `engine`; `None` for blank input.
pub(crate) fn resolve(input: &str, engine: SearchEngine) -> Option<Address> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    if let Some(path) = file_path(input) {
        return Some(Address::File(path));
    }
    if scheme_of(input).is_some_and(|scheme| matches!(scheme.as_str(), "about" | "data" | "blob"))
        || input.contains("://")
    {
        return Some(Address::Url(input.to_string()));
    }
    if !input.contains(char::is_whitespace)
        && let Some(host) = bare_host(input)
    {
        let scheme = if prefers_plain_http(&host) { "http" } else { "https" };
        return Some(Address::Url(format!("{scheme}://{input}")));
    }
    Some(Address::Url(search_url(engine, input)))
}

/// The URL that asks `engine` for `words`.
pub(crate) fn search_url(engine: SearchEngine, words: &str) -> String {
    let base = match engine {
        SearchEngine::DuckDuckGo => "https://duckduckgo.com/?q=",
        SearchEngine::Google => "https://www.google.com/search?q=",
        SearchEngine::Bing => "https://www.bing.com/search?q=",
        SearchEngine::Startpage => "https://www.startpage.com/sp/search?query=",
    };
    format!("{base}{}", form_encode(words))
}

/// `text` as a value of an HTML form: spaces become `+`, everything but letters, digits and `-_.~` its UTF-8 bytes in
/// `%XX`.
fn form_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b' ' => out.push('+'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(byte)),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The lower-case scheme of `url`, if it starts with one.
pub(crate) fn scheme_of(url: &str) -> Option<String> {
    let (scheme, _) = url.split_once(':')?;
    let mut chars = scheme.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    valid.then(|| scheme.to_ascii_lowercase())
}

/// Whether the browser shows pages of `scheme` itself; everything else, such as `mailto:` or another app's scheme,
/// it refuses rather than handing to the system.
pub(crate) fn is_web_scheme(scheme: &str) -> bool {
    matches!(scheme, "http" | "https" | "about" | "data" | "blob")
}

/// Whether "open in the default browser" may hand `url` to another app: a web page, or an HTML file, which never runs as a
/// program the way a script of the workspace opened by its default app would (E16).
pub(crate) fn opens_externally(url: &str) -> bool {
    match scheme_of(url).as_deref() {
        Some("http" | "https") => true,
        Some("file") => file_path(url)
            .and_then(|path| path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()))
            .is_some_and(|extension| matches!(extension.as_str(), "html" | "htm" | "xhtml")),
        _ => false,
    }
}

/// The path a `file:` URL or a typed absolute path names.
pub(crate) fn file_path(input: &str) -> Option<PathBuf> {
    if input.starts_with('/') {
        return Some(PathBuf::from(input));
    }
    if let Some(rest) = input.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    if scheme_of(input).as_deref() != Some("file") {
        return None;
    }
    let rest = &input["file:".len()..];
    let rest = rest.strip_prefix("//localhost").or_else(|| rest.strip_prefix("//"))?;
    let path = rest.split(['?', '#']).next().unwrap_or("");
    path.starts_with('/')
        .then(|| PathBuf::from(OsString::from_vec(percent_decode(path))))
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes.get(i + 1..i + 3).and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    out
}

/// Whether `path` lies inside the folder `root`, after resolving links on both.
pub(crate) fn is_inside(path: &Path, root: &Path) -> bool {
    match (path.canonicalize(), root.canonicalize()) {
        (Ok(path), Ok(root)) => path.starts_with(root),
        _ => false,
    }
}

/// The host of `input` read as an address without a scheme, such as `localhost:5173/app` or `docs.rs`; `None` when
/// it reads more like search words.
fn bare_host(input: &str) -> Option<String> {
    let authority = input.split(['/', '?', '#']).next().unwrap_or("");
    let (host, port) = split_port(authority)?;
    if host.is_empty() || port.is_some_and(|p| p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let host = host.to_ascii_lowercase();
    if host.starts_with('[') {
        return Some(host);
    }
    let labels: Vec<&str> = host.split('.').collect();
    let plain = labels
        .iter()
        .all(|label| !label.is_empty() && label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    if !plain {
        return None;
    }
    let top = labels.last().copied().unwrap_or("");
    let named = labels.len() > 1 && top.len() >= 2 && top.bytes().all(|b| b.is_ascii_alphabetic());
    (host == "localhost" || is_ipv4(&host) || named || port.is_some()).then_some(host)
}

/// `authority` split into host and port; an IPv6 literal keeps its brackets.
pub(crate) fn split_port(authority: &str) -> Option<(&str, Option<&str>)> {
    if authority.starts_with('[') {
        let end = authority.find(']')?;
        let rest = &authority[end + 1..];
        return match rest.strip_prefix(':') {
            Some(port) => Some((&authority[..=end], Some(port))),
            None if rest.is_empty() => Some((authority, None)),
            None => None,
        };
    }
    Some(match authority.rsplit_once(':') {
        Some((host, port)) => (host, Some(port)),
        None => (authority, None),
    })
}

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok())
}

/// Whether a host without a scheme is reached over plain HTTP, as servers on this Mac and in the local network are.
fn prefers_plain_http(host: &str) -> bool {
    is_loopback(host)
        || is_ipv4(host)
        || host.starts_with('[')
        || !host.contains('.')
        || [".local", ".test", ".internal", ".lan"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// Whether `host` is this Mac.
pub(crate) fn is_loopback(host: &str) -> bool {
    host == "localhost"
        || host.ends_with(".localhost")
        || host == "[::1]"
        || host == "0.0.0.0"
        || host.starts_with("127.") && is_ipv4(host)
}

/// The origin of `url` as `scheme://host[:port]`, lower case and without a default port; `None` for URLs without
/// a host, such as `about:` and `file:`.
pub(crate) fn origin(url: &str) -> Option<String> {
    let scheme = scheme_of(url)?;
    let rest = url[scheme.len() + 1..].strip_prefix("//")?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
    let (host, port) = split_port(authority)?;
    if host.is_empty() {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let default = match scheme.as_str() {
        "http" | "ws" => Some("80"),
        "https" | "wss" => Some("443"),
        _ => None,
    };
    Some(match port.filter(|p| !p.is_empty() && Some(*p) != default) {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// Whether `url` names a server rather than a page of it, such as `http://localhost:5173/`.
pub(crate) fn is_origin_only(url: &str) -> bool {
    let Some(rest) = url.split_once("://").map(|(_, rest)| rest) else {
        return false;
    };
    let path = rest.find(['/', '?', '#']).map_or("", |at| &rest[at..]);
    origin(url).is_some() && (path.is_empty() || path == "/")
}

/// The host of `url`, if it has one.
pub(crate) fn host_of(url: &str) -> Option<String> {
    let origin = origin(url)?;
    let authority = origin.split_once("://")?.1;
    Some(split_port(authority)?.0.to_string())
}

/// Whether `url` is served from this Mac.
pub(crate) fn is_local(url: &str) -> bool {
    host_of(url).is_some_and(|host| is_loopback(&host))
}

/// The tab key of the page at `url`; see [`Address::key`].
pub(crate) fn key_of(url: &str) -> String {
    match origin(url) {
        Some(origin) if is_local(url) => origin,
        _ => url.split('#').next().unwrap_or(url).to_string(),
    }
}

/// A tab title for a page titled `title` at `url`: the title, shortened to `limit` characters, else the page's host.
pub(crate) fn tab_title(title: &str, url: Option<&str>, limit: usize) -> Option<String> {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return url.and_then(host_of);
    }
    if title.chars().count() <= limit {
        return Some(title);
    }
    let short: String = title.chars().take(limit.saturating_sub(1)).collect();
    Some(format!("{}…", short.trim_end()))
}

/// A tab title for `address` before its page loaded: the host of its URL, else its file's name.
pub(crate) fn title_before_load(address: &Address, limit: usize) -> Option<String> {
    match address {
        Address::Url(url) => tab_title("", Some(url), limit),
        Address::File(path) => path.file_name().map(|name| name.to_string_lossy().into_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(input: &str) -> Option<String> {
        match resolve(input, SearchEngine::DuckDuckGo)? {
            Address::Url(url) => Some(url),
            Address::File(path) => Some(format!("file {}", path.display())),
        }
    }

    #[test]
    fn hosts_without_a_scheme_get_the_one_their_kind_of_server_speaks() {
        assert_eq!(url("localhost:5173").as_deref(), Some("http://localhost:5173"));
        assert_eq!(
            url("127.0.0.1:8080/app?x=1").as_deref(),
            Some("http://127.0.0.1:8080/app?x=1")
        );
        assert_eq!(url("[::1]:3000").as_deref(), Some("http://[::1]:3000"));
        assert_eq!(url("devbox:8000").as_deref(), Some("http://devbox:8000"));
        assert_eq!(url("printer.local").as_deref(), Some("http://printer.local"));
        assert_eq!(url("docs.rs/serde").as_deref(), Some("https://docs.rs/serde"));
        assert_eq!(url("  https://example.com  ").as_deref(), Some("https://example.com"));
        assert_eq!(url("about:blank").as_deref(), Some("about:blank"));
    }

    #[test]
    fn words_are_searched_with_the_chosen_engine() {
        assert_eq!(
            url("rust borrow checker").as_deref(),
            Some("https://duckduckgo.com/?q=rust+borrow+checker")
        );
        assert_eq!(url("1.5").as_deref(), Some("https://duckduckgo.com/?q=1.5"));
        assert_eq!(
            url("javascript:alert(1)").as_deref(),
            Some("https://duckduckgo.com/?q=javascript%3Aalert%281%29")
        );
        assert_eq!(
            search_url(SearchEngine::Startpage, "Grüße & mehr"),
            "https://www.startpage.com/sp/search?query=Gr%C3%BC%C3%9Fe+%26+mehr"
        );
        assert_eq!(resolve("   ", SearchEngine::Google), None);
        assert_eq!(
            search_url(SearchEngine::Google, "a b"),
            "https://www.google.com/search?q=a+b"
        );
        assert_eq!(
            search_url(SearchEngine::Bing, "a b"),
            "https://www.bing.com/search?q=a+b"
        );
    }

    #[test]
    fn file_urls_and_absolute_paths_name_files() {
        assert_eq!(url("/tmp/a b.html").as_deref(), Some("file /tmp/a b.html"));
        assert_eq!(url("file:///tmp/a%20b.html#top").as_deref(), Some("file /tmp/a b.html"));
        assert_eq!(url("file://localhost/tmp/x.html").as_deref(), Some("file /tmp/x.html"));
        assert_eq!(file_path("https://example.com/x"), None);
        let home = std::env::var_os("HOME").map(PathBuf::from);
        assert_eq!(
            file_path("~/site/index.html"),
            home.map(|home| home.join("site/index.html"))
        );
    }

    #[test]
    fn only_web_pages_and_html_files_open_in_another_app() {
        assert!(opens_externally("https://example.org/"));
        assert!(opens_externally("http://localhost:5173/"));
        assert!(opens_externally("file:///work/site/index.html"));
        assert!(!opens_externally("file:///work/run.command"));
        assert!(!opens_externally("file:///work/tool.app"));
        assert!(!opens_externally("mailto:a@example.org"));
        assert!(!opens_externally("data:text/html,hi"));
    }

    #[test]
    fn a_file_counts_as_inside_only_where_its_links_resolve() {
        let root = term_base::testing::scratch("browser-inside");
        std::fs::create_dir_all(root.join("site")).unwrap();
        std::fs::write(root.join("site/index.html"), "<p>x</p>").unwrap();
        assert!(is_inside(&root.join("site/index.html"), &root));
        assert!(!is_inside(&root.join("site/../../etc/hosts"), &root));
        assert!(!is_inside(Path::new("/etc/hosts"), &root));
        assert!(!is_inside(&root.join("missing.html"), &root));
    }

    #[test]
    fn origins_drop_paths_case_and_default_ports() {
        assert_eq!(
            origin("HTTP://LocalHost:5173/a/b#c").as_deref(),
            Some("http://localhost:5173")
        );
        assert_eq!(
            origin("https://user:pw@example.com:443/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(origin("http://[::1]:80").as_deref(), Some("http://[::1]"));
        assert_eq!(origin("about:blank"), None);
        assert_eq!(origin("file:///tmp/x.html"), None);
    }

    #[test]
    fn a_local_server_is_one_tab_and_other_pages_one_tab_each() {
        assert_eq!(key_of("http://localhost:5173/settings"), "http://localhost:5173");
        assert_eq!(key_of("http://127.0.0.1:5173/#/a"), "http://127.0.0.1:5173");
        assert_eq!(key_of("https://docs.rs/serde#top"), "https://docs.rs/serde");
        assert!(is_local("http://app.localhost:3000"));
        assert!(!is_local("http://192.168.1.10:3000"));
        assert!(is_origin_only("http://localhost:5173"));
        assert!(is_origin_only("http://localhost:5173/"));
        assert!(!is_origin_only("http://localhost:5173/settings"));
        assert!(!is_origin_only("http://localhost:5173/?tab=2"));
        assert_eq!(
            Address::of_url("file:///p/a%20b.html"),
            Address::File(PathBuf::from("/p/a b.html"))
        );
        assert_eq!(Address::of_url("/p"), Address::Url("/p".into()));
        let file = Address::File(PathBuf::from("/p/index.html"));
        assert_eq!(file.key(), "file:///p/index.html");
    }

    #[test]
    fn tab_titles_are_short_and_fall_back_to_the_host() {
        assert_eq!(tab_title("  Vite \n App ", None, 30).as_deref(), Some("Vite App"));
        assert_eq!(
            tab_title("A very long title of a documentation page", None, 12).as_deref(),
            Some("A very long…")
        );
        assert_eq!(
            tab_title("", Some("http://localhost:5173/x"), 30).as_deref(),
            Some("localhost")
        );
        assert_eq!(tab_title("", None, 30), None);
    }

    #[test]
    fn a_tab_that_has_not_loaded_is_titled_by_its_host_or_file() {
        let url = Address::Url("https://docs.rs/objc2/latest".into());
        assert_eq!(title_before_load(&url, 30).as_deref(), Some("docs.rs"));
        let file = Address::File(PathBuf::from("/p/site/index.html"));
        assert_eq!(title_before_load(&file, 30).as_deref(), Some("index.html"));
    }
}
