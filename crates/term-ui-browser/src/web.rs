//! Calls into WebKit: the configuration every tab shares and the rules for what a page may open. objc2-web-kit marks
//! every method unsafe because the framework has not been reviewed for it yet, so most blocks here state only that a
//! method asks for nothing.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::NSEventModifierFlags;
use objc2_foundation::{NSBundle, NSNumber, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{
    WKDownload, WKDownloadDelegate, WKNavigationAction, WKNavigationActionPolicy, WKNavigationResponse,
    WKNavigationResponsePolicy, WKNavigationType, WKWebView, WKWebViewConfiguration, WKWebpagePreferences,
    WKWebsiteDataStore,
};

use super::address;
use term_ui::kit::{guarded, ns};
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;

thread_local! {
    static STORE: OnceCell<Retained<WKWebsiteDataStore>> = const { OnceCell::new() };
}

/// Where the pages of every tab keep cookies and storage: WebKit's default store of the app, so a login to a dev
/// server survives a restart as in Safari; the app adds nothing to it, and a self-test keeps nothing.
fn store(mtm: MainThreadMarker) -> Retained<WKWebsiteDataStore> {
    STORE.with(|store| {
        store
            .get_or_init(|| {
                #[cfg(feature = "selftest")]
                if std::env::var_os("TAPA_SELFTEST").is_some() {
                    // SAFETY: A store that lives in memory only, created on the main thread.
                    return unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
                }
                // SAFETY: The app's default store, created on the main thread.
                unsafe { WKWebsiteDataStore::defaultDataStore(mtm) }
            })
            .clone()
    })
}

/// Sets the cookie `name` with `value` for pages of 127.0.0.1 in the tabs' store, kept from their scripts, then calls
/// `done`; `None` when WebKit could not make the cookie.
pub fn set_local_cookie(
    mtm: MainThreadMarker,
    port: u16,
    name: &str,
    value: &str,
    done: impl FnOnce() + 'static,
) -> Option<()> {
    use objc2_foundation::{NSDictionary, NSHTTPCookie, NSURL};
    // The property keys of NSHTTPCookie have none for HttpOnly, the header parser takes it.
    let header = NSDictionary::from_retained_objects(
        &[&*ns("Set-Cookie")],
        &[ns(&format!("{name}={value}; Path=/; HttpOnly"))],
    );
    let url = NSURL::URLWithString(&ns(&format!("http://127.0.0.1:{port}/")))?;
    let cookie = NSHTTPCookie::cookiesWithResponseHeaderFields_forURL(&header, &url).firstObject()?;
    let done = std::cell::Cell::new(Some(done));
    let block = block2::RcBlock::new(move || {
        term_ui::kit::guarded((), || {
            if let Some(done) = done.take() {
                done();
            }
        });
    });
    // SAFETY: The store and the cookie live on the main thread; WebKit calls the block there once the cookie is set.
    unsafe {
        store(mtm)
            .httpCookieStore()
            .setCookie_completionHandler(&cookie, Some(&block))
    };
    Some(())
}

/// The configuration of a new tab. It adds no script message handler, so no page can call into the app.
pub(crate) fn configuration(mtm: MainThreadMarker) -> Retained<WKWebViewConfiguration> {
    // SAFETY: A plain configuration object, created on the main thread.
    let config = unsafe { WKWebViewConfiguration::new(mtm) };
    // SAFETY: The shared store and plain flags, set before any view uses the configuration.
    unsafe {
        config.setWebsiteDataStore(&store(mtm));
        if let Some(name) = safari_name() {
            config.setApplicationNameForUserAgent(Some(&ns(name)));
        }
        let preferences = config.preferences();
        #[cfg(feature = "selftest")]
        let unasked = POPUPS_UNASKED.load(std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(feature = "selftest"))]
        let unasked = false;
        preferences.setJavaScriptCanOpenWindowsAutomatically(unasked);
        preferences.setFraudulentWebsiteWarningEnabled(true);
        preferences.setElementFullscreenEnabled(true);
    }
    enable_inspect_element(&config);
    config
}

/// The tail Safari adds to WebKit's user agent, for the installed Safari. Sites such as Google serve a reduced page to
/// a WebKit browser whose user agent lacks it.
fn safari_name() -> Option<&'static str> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    NAME.get_or_init(|| {
        let url = objc2_app_kit::NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&ns("com.apple.Safari"))?;
        let bundle = NSBundle::bundleWithURL(&url)?;
        let version = bundle.objectForInfoDictionaryKey(&ns("CFBundleShortVersionString"))?;
        user_agent_tail(&version.downcast_ref::<NSString>()?.to_string())
    })
    .as_deref()
}

/// Safari's user agent tail for Safari `version`; the `Safari/605.1.15` token is frozen in every Safari since 2017.
fn user_agent_tail(version: &str) -> Option<String> {
    let valid = version
        .split('.')
        .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
    valid.then(|| format!("Version/{version} Safari/605.1.15"))
}

/// Adds "Inspect Element" to a page's context menu. `isInspectable` alone only lets Safari's Develop menu attach;
/// the menu item needs this preference, which WebKit offers through key-value coding only.
fn enable_inspect_element(config: &WKWebViewConfiguration) {
    guarded((), || {
        // SAFETY: The preferences object answers this key with a BOOL property; an unknown key raises an exception,
        // which objc2 turns into a panic that `guarded` stops.
        unsafe {
            let preferences = config.preferences();
            let on = NSNumber::new_bool(true);
            let _: () = objc2::msg_send![&*preferences, setValue: &*on, forKey: &*ns("developerExtrasEnabled")];
        }
    });
}

pub(crate) fn new_view(mtm: MainThreadMarker, config: &WKWebViewConfiguration) -> Retained<WKWebView> {
    // SAFETY: The configuration is complete and the frame empty, which constraints replace.
    let web = unsafe {
        WKWebView::initWithFrame_configuration(WKWebView::alloc(mtm), objc2_foundation::NSRect::ZERO, config)
    };
    // SAFETY: Plain flags of a new view; `setInspectable` exists from macOS 13.3, below the app's minimum.
    unsafe {
        web.setInspectable(true);
        web.setAllowsBackForwardNavigationGestures(true);
        web.setAllowsMagnification(true);
    }
    web
}

/// Loads `url` in `web`; `false` if it is no URL.
pub(crate) fn load(web: &WKWebView, url: &str) -> bool {
    let Some(url) = NSURL::URLWithString(&ns(url)) else {
        return false;
    };
    // SAFETY: A request for a URL that parsed.
    unsafe { web.loadRequest(&NSURLRequest::requestWithURL(&url)) };
    true
}

/// Loads the file at `path` of the workspace at `root`, letting the page read its own folder and nothing else;
/// `false` when the file lies outside the workspace.
pub(crate) fn load_file(web: &WKWebView, path: &Path, root: &Path) -> bool {
    let Some(folder) = read_access(path, root) else {
        return false;
    };
    let file = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    let folder = NSURL::fileURLWithPath_isDirectory(&ns(&folder.to_string_lossy()), true);
    // SAFETY: Both are file URLs, the file inside the folder, as `read_access` made sure.
    unsafe { web.loadFileURL_allowingReadAccessToURL(&file, &folder) };
    true
}

/// The folder a page at `path` may read: the one it sits in, and only while that lies in the workspace at `root`
/// (E22). The workspace's base path itself only for a page directly in it.
pub(crate) fn read_access(path: &Path, root: &Path) -> Option<PathBuf> {
    if !address::is_inside(path, root) {
        return None;
    }
    path.canonicalize().ok()?.parent().map(Path::to_path_buf)
}

/// Whether the page a navigation to a URL of `scheme` leads to runs its scripts, when the page that started it has a
/// URL of `source`: in a workspace the user does not trust, no page from a file runs any, nor one such a page opens
/// in place of itself (E22).
pub(crate) fn runs_scripts(scheme: &str, source: Option<&str>, trusted: bool) -> bool {
    if trusted {
        return true;
    }
    match scheme {
        "file" => false,
        "about" | "data" | "blob" => source != Some("file"),
        _ => true,
    }
}

/// Sets in `preferences` whether the page `action` leads to runs its scripts, by `runs_scripts`.
pub(crate) fn limit_scripts(action: &WKNavigationAction, preferences: &WKWebpagePreferences, trusted: bool) {
    // SAFETY: The action, its frames and the preferences are valid while WebKit asks about the navigation.
    unsafe {
        let scheme_of = |url: Option<Retained<NSURL>>| {
            url.and_then(|url| url.scheme())
                .map(|scheme| scheme.to_string().to_ascii_lowercase())
        };
        let scheme = scheme_of(action.request().URL()).unwrap_or_default();
        let source = scheme_of(action.sourceFrame().and_then(|frame| frame.request().URL()));
        preferences.setAllowsContentJavaScript(runs_scripts(&scheme, source.as_deref(), trusted));
    }
}

/// What the view shows: its URL, title, whether it loads and where it can go.
pub(crate) struct State {
    pub(crate) url: Option<String>,
    pub(crate) title: String,
    pub(crate) loading: bool,
    pub(crate) back: bool,
    pub(crate) forward: bool,
}

pub(crate) fn state(web: &WKWebView) -> State {
    // SAFETY: Plain properties of a view on the main thread.
    unsafe {
        State {
            url: web.URL().and_then(|u| u.absoluteString()).map(|s| s.to_string()),
            title: web.title().map(|s| s.to_string()).unwrap_or_default(),
            loading: web.isLoading(),
            back: web.canGoBack(),
            forward: web.canGoForward(),
        }
    }
}

pub(crate) fn go_back(web: &WKWebView) {
    // SAFETY: Asks for nothing; without a page to go back to it does nothing.
    unsafe { web.goBack() };
}

pub(crate) fn go_forward(web: &WKWebView) {
    // SAFETY: Asks for nothing; without a page to go forward to it does nothing.
    unsafe { web.goForward() };
}

pub(crate) fn reload(web: &WKWebView) {
    // SAFETY: Stopping takes no arguments and does nothing when the view loads nothing.
    unsafe { web.reload() };
}

pub(crate) fn stop(web: &WKWebView) {
    // SAFETY: Stopping takes no arguments and does nothing when the view loads nothing.
    unsafe { web.stopLoading() };
}

/// What to do with a navigation: let it load, cancel it, cancel it and open its URL in a new tab, cancel it and load
/// a file of the workspace with read access to its folder, or download it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    Allow,
    Cancel,
    NewTab(String),
    LoadFile(PathBuf),
    Download,
}

/// Decides a navigation of a page in a workspace whose folder is `root`, from a page that may read `granted`. Pages
/// may lead to web pages, to local files only inside the workspace (a web page reaches none, WebKit sees to that),
/// and to other apps' schemes never.
pub(crate) fn decide_action(action: &WKNavigationAction, root: Option<&Path>, granted: Option<&Path>) -> Decision {
    // SAFETY: The action and its request are valid while WebKit asks about them.
    let (url, download, link, command, main_frame) = unsafe {
        let url = action
            .request()
            .URL()
            .and_then(|u| u.absoluteString())
            .map(|s| s.to_string());
        let main_frame = action.targetFrame().is_some_and(|frame| frame.isMainFrame());
        (
            url,
            action.shouldPerformDownload(),
            action.navigationType() == WKNavigationType::LinkActivated,
            action.modifierFlags().contains(NSEventModifierFlags::Command),
            main_frame,
        )
    };
    decide(url, download, link && command, main_frame, Folders { root, granted })
}

/// The folders a navigation to a file is measured against: the workspace's and the one its page may read.
#[derive(Clone, Copy)]
struct Folders<'a> {
    root: Option<&'a Path>,
    granted: Option<&'a Path>,
}

/// `decide_action` for a navigation to `url`, which WebKit should `download`, from a link clicked with ⌘ when
/// `command_link`. A file outside the folder its page may read loads anew with access to its own folder, as a page
/// of its own, and never into a frame.
fn decide(url: Option<String>, download: bool, command_link: bool, main_frame: bool, folders: Folders) -> Decision {
    let Some(url) = url else { return Decision::Cancel };
    let scheme = address::scheme_of(&url).unwrap_or_default();
    if scheme == "file" {
        let Some((path, root)) = address::file_path(&url).zip(folders.root) else {
            return Decision::Cancel;
        };
        if !address::is_inside(&path, root) {
            return Decision::Cancel;
        }
        let readable = folders
            .granted
            .is_some_and(|granted| address::is_inside(&path, granted));
        if !readable {
            return if main_frame && !download {
                Decision::LoadFile(path)
            } else {
                Decision::Cancel
            };
        }
    }
    let allowed = scheme == "file" || address::is_web_scheme(&scheme);
    if !allowed {
        return Decision::Cancel;
    }
    if download {
        return Decision::Download;
    }
    if command_link && main_frame && matches!(scheme.as_str(), "http" | "https") {
        return Decision::NewTab(url);
    }
    Decision::Allow
}

/// Downloads what a page cannot show, and what its server sends as an attachment.
pub(crate) fn decide_response(response: &WKNavigationResponse) -> WKNavigationResponsePolicy {
    // SAFETY: The response is valid while WebKit asks about it.
    let (main_frame, showable, response) = unsafe {
        (
            response.isForMainFrame(),
            response.canShowMIMEType(),
            response.response(),
        )
    };
    let attachment = response
        .downcast_ref::<objc2_foundation::NSHTTPURLResponse>()
        .and_then(|http| http.valueForHTTPHeaderField(&ns("Content-Disposition")))
        .is_some_and(|value| {
            value
                .to_string()
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("attachment")
        });
    if main_frame && (!showable || attachment) {
        WKNavigationResponsePolicy::Download
    } else {
        WKNavigationResponsePolicy::Allow
    }
}

pub(crate) fn action_policy(decision: &Decision) -> WKNavigationActionPolicy {
    match decision {
        Decision::Allow => WKNavigationActionPolicy::Allow,
        Decision::Download => WKNavigationActionPolicy::Download,
        Decision::Cancel | Decision::NewTab(_) | Decision::LoadFile(_) => WKNavigationActionPolicy::Cancel,
    }
}

/// Hands `download` to `delegate`, which asks where to save it.
pub(crate) fn adopt_download(download: &WKDownload, delegate: &ProtocolObject<dyn WKDownloadDelegate>) {
    // SAFETY: The download keeps its delegate weakly; the delegate lives as long as the app.
    unsafe { download.setDelegate(Some(delegate)) };
}

/// The value of `key` in an error's details, as text.
pub(crate) fn error_detail(error: &objc2_foundation::NSError, key: &objc2_foundation::NSString) -> Option<String> {
    let value: Retained<AnyObject> = error.userInfo().objectForKey(key)?;
    if let Some(text) = value.downcast_ref::<objc2_foundation::NSString>() {
        return Some(text.to_string());
    }
    let url = value.downcast_ref::<NSURL>()?;
    url.absoluteString().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        Decision, Folders, WKNavigationActionPolicy, action_policy, decide, error_detail, read_access, runs_scripts,
        user_agent_tail,
    };
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSDictionary, NSError, NSString, NSURL};
    /// A folder with an `index.html`, as a workspace's base path.
    fn project(name: &str) -> term_base::testing::Scratch {
        let root = term_base::testing::scratch(&format!("ui-browser-{name}"));
        std::fs::write(root.join("index.html"), "<p>hi</p>").unwrap();
        root
    }

    #[test]
    fn pages_lead_to_web_pages_and_files_of_the_folder_only() {
        let root = project("lead");
        let inside = format!("file://{}", root.join("index.html").display());
        let folders = Folders {
            root: Some(&root),
            granted: Some(&root),
        };
        let to = |url: &str| decide(Some(url.to_string()), false, false, true, folders);
        assert_eq!(to("https://example.org/"), Decision::Allow);
        assert_eq!(to(&inside), Decision::Allow);
        assert_eq!(to("file:///etc/hosts"), Decision::Cancel);
        assert_eq!(to("mailto:someone@example.org"), Decision::Cancel);
        assert_eq!(to("x-apple.systempreferences:"), Decision::Cancel);
        let nowhere = Folders {
            root: None,
            granted: None,
        };
        assert_eq!(decide(Some(inside), false, false, true, nowhere), Decision::Cancel);
        assert_eq!(decide(None, false, false, true, folders), Decision::Cancel);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn downloads_go_to_webkit_and_command_clicks_on_web_links_to_a_new_tab() {
        let root = project("command");
        let url = || Some("https://example.org/file.zip".to_string());
        let none = Folders {
            root: None,
            granted: None,
        };
        assert_eq!(decide(url(), true, false, true, none), Decision::Download);
        assert_eq!(
            decide(url(), false, true, true, none),
            Decision::NewTab("https://example.org/file.zip".into())
        );
        assert_eq!(decide(url(), false, true, false, none), Decision::Allow);
        let file = format!("file://{}", root.join("index.html").display());
        let folders = Folders {
            root: Some(&root),
            granted: Some(&root),
        };
        assert_eq!(decide(Some(file), false, true, true, folders), Decision::Allow);
        assert_eq!(
            decide(Some("ftp://example.org/".into()), true, false, true, none),
            Decision::Cancel
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_page_reads_its_own_folder_only_and_the_base_only_when_it_sits_there() {
        let root = project("folder");
        std::fs::create_dir_all(root.join("site/deep")).unwrap();
        std::fs::write(root.join("site/index.html"), "<p>site</p>").unwrap();
        std::fs::write(root.join("site/deep/page.html"), "<p>deep</p>").unwrap();
        let base = root.canonicalize().unwrap();
        assert_eq!(
            read_access(&root.join("site/index.html"), &root),
            Some(base.join("site"))
        );
        assert_eq!(
            read_access(&root.join("site/deep/page.html"), &root),
            Some(base.join("site/deep"))
        );
        assert_eq!(read_access(&root.join("index.html"), &root), Some(base.clone()));
        assert_eq!(read_access(std::path::Path::new("/etc/hosts"), &root), None);
        assert_eq!(read_access(&root.join("site/missing.html"), &root), None);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_file_beyond_the_pages_folder_loads_as_a_page_of_its_own_never_into_a_frame() {
        let root = project("beyond");
        std::fs::create_dir_all(root.join("site")).unwrap();
        std::fs::write(root.join("site/index.html"), "<p>site</p>").unwrap();
        let site = root.join("site");
        let folders = Folders {
            root: Some(&root),
            granted: Some(&site),
        };
        let readme = root.join("index.html");
        let url = || Some(format!("file://{}", readme.display()));
        assert_eq!(
            decide(url(), false, false, true, folders),
            Decision::LoadFile(readme.clone())
        );
        assert_eq!(decide(url(), false, false, false, folders), Decision::Cancel);
        let beside = Some(format!("file://{}", site.join("index.html").display()));
        assert_eq!(decide(beside, false, false, false, folders), Decision::Allow);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn file_pages_of_an_untrusted_workspace_run_no_scripts_and_web_pages_do() {
        assert!(!runs_scripts("file", None, false));
        assert!(!runs_scripts("file", Some("http"), false));
        assert!(!runs_scripts("data", Some("file"), false));
        assert!(!runs_scripts("about", Some("file"), false));
        assert!(runs_scripts("http", Some("file"), false));
        assert!(runs_scripts("http", None, false), "localhost pages run as usual");
        assert!(runs_scripts("https", None, false));
        assert!(runs_scripts("about", None, false));
        assert!(runs_scripts("file", None, true), "a trusted workspace's files run");
    }

    #[test]
    fn an_errors_detail_reads_as_text_whether_it_is_a_string_or_an_address() {
        let key = NSString::from_str("NSErrorFailingURLStringKey");
        let other = NSString::from_str("NSErrorFailingURLKey");
        let missing = NSString::from_str("Missing");
        let url = NSURL::URLWithString(&NSString::from_str("https://example.org/x")).unwrap();
        let values: [Retained<AnyObject>; 2] = [NSString::from_str("https://example.org/y").into(), url.into()];
        let info = NSDictionary::from_retained_objects(&[&*key, &*other], &values);
        // SAFETY: A plain error with a dictionary of strings to objects.
        let error = unsafe { NSError::errorWithDomain_code_userInfo(&NSString::from_str("Test"), 1, Some(&info)) };
        assert_eq!(error_detail(&error, &key).as_deref(), Some("https://example.org/y"));
        assert_eq!(error_detail(&error, &other).as_deref(), Some("https://example.org/x"));
        assert_eq!(error_detail(&error, &missing), None);
    }

    #[test]
    fn a_new_tab_cancels_the_navigation_in_the_page() {
        assert_eq!(action_policy(&Decision::Allow), WKNavigationActionPolicy::Allow);
        assert_eq!(action_policy(&Decision::Download), WKNavigationActionPolicy::Download);
        assert_eq!(action_policy(&Decision::Cancel), WKNavigationActionPolicy::Cancel);
        assert_eq!(
            action_policy(&Decision::NewTab("https://example.org/".into())),
            WKNavigationActionPolicy::Cancel
        );
    }

    #[test]
    fn user_agent_tail_takes_a_dotted_version_only() {
        assert_eq!(user_agent_tail("27.2").as_deref(), Some("Version/27.2 Safari/605.1.15"));
        assert_eq!(
            user_agent_tail("27.2.1").as_deref(),
            Some("Version/27.2.1 Safari/605.1.15")
        );
        assert_eq!(user_agent_tail(""), None);
        assert_eq!(user_agent_tail("27..2"), None);
        assert_eq!(user_agent_tail("27.2 beta"), None);
    }
}
