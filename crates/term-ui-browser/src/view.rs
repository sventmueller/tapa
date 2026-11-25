//! The view of a browser tab: the navigation row, the page below it, and the native page that stands in for it.

use std::cell::{Cell, OnceCell, RefCell};
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;

use block2::DynBlock;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, Bool, NSObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSBox, NSBoxType, NSControl, NSControlTextEditingDelegate, NSEvent, NSMenu, NSMenuItem, NSMenuItemValidation,
    NSTextFieldDelegate, NSTextView, NSView, NSWorkspace,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSError, NSKeyValueObservingOptions, NSObjectNSKeyValueObserverRegistration,
    NSObjectProtocol, NSRect, NSString, NSURL, NSURLAuthenticationChallenge, NSURLCredential,
    NSURLSessionAuthChallengeDisposition,
};
use objc2_web_kit::{
    WKDownload, WKFrameInfo, WKMediaCaptureType, WKNavigation, WKNavigationAction, WKNavigationActionPolicy,
    WKNavigationDelegate, WKNavigationResponse, WKNavigationResponsePolicy, WKOpenPanelParameters,
    WKPermissionDecision, WKSecurityOrigin, WKUIDelegate, WKWebView, WKWebViewConfiguration, WKWebpagePreferences,
    WKWindowFeatures,
};
use splat_i18n::t;

use super::address::{self, Address};
use super::controls::Controls;
use super::failure::{Failure, FailurePage};
use super::notice::Notice;
use super::{autofill, dialogs, downloads, tabs, trust, web};
use term_render::constant;
use term_ui::kit::{activate, guarded, item, ns, pin_sides, set_target};
use term_ui::workspace_host::{TabId, Tabs, Trust, Workspace, WorkspaceId};
#[cfg(feature = "selftest")]
mod selftest_probe;

/// The longest tab title, in characters.
pub(crate) const TITLE_LIMIT: usize = 32;

/// The properties of the web view the controls follow.
const OBSERVED: [&str; 5] = ["title", "URL", "loading", "canGoBack", "canGoForward"];

/// Tells this view's observations apart from those of its superclass.
static OBSERVATION: u8 = 0;

fn observation() -> *mut c_void {
    std::ptr::addr_of!(OBSERVATION).cast_mut().cast()
}

struct Parts {
    web: Retained<WKWebView>,
    controls: Controls,
    failure: FailurePage,
    notice: Notice,
    /// Items that are never shown and only give the tab its shortcuts.
    shortcuts: Retained<NSMenu>,
}

/// What loads an address into a tab that waited to show.
pub type Loader = Box<dyn FnOnce(&str)>;

/// What a tab restored out of view does once it first shows.
pub(crate) enum Wake {
    Go(Address),
    /// Runs with what loads a page into the tab, for a page whose server starts only then.
    Run(Box<dyn FnOnce(Loader)>),
}

pub(crate) struct Ivars {
    /// The id of the workspace the tab belongs to.
    workspace: WorkspaceId,
    key: String,
    tab: Cell<Option<TabId>>,
    /// Built when the tab first shows, or when it is asked to do something.
    parts: OnceCell<Parts>,
    waiting: RefCell<Option<Wake>>,
    /// What was last asked for, which the field shows until the page has an address, and "Try Again" asks again.
    asked: RefCell<Option<Address>>,
    failure: RefCell<Option<Failure>>,
    /// The tab title last set, so an unchanged one is not set again.
    title: RefCell<String>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements; `Drop` only detaches the view from its web view and field.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermBrowserView"]
    #[ivars = Ivars]
    pub(crate) struct BrowserView;

    // SAFETY: NSObjectProtocol asks nothing of a subclass of NSObject.
    unsafe impl NSObjectProtocol for BrowserView {}

    // SAFETY: The method below has the selector's signature, as AppKit calls it on the main thread.
    unsafe impl NSControlTextEditingDelegate for BrowserView {
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command(&self, _control: &NSControl, _view: &NSTextView, command: Sel) -> bool {
            guarded(false, || self.field_command(command))
        }
    }

    // SAFETY: The protocol's methods are all optional and none is implemented here.
    unsafe impl NSTextFieldDelegate for BrowserView {}

    // SAFETY: The method below has the selector's signature, as AppKit calls it on the main thread.
    unsafe impl NSMenuItemValidation for BrowserView {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            guarded(true, || self.validate(item.action()))
        }
    }

    // SAFETY: Each method below has its selector's signature and calls every completion handler exactly once.
    unsafe impl WKNavigationDelegate for BrowserView {
        #[unsafe(method(webView:decidePolicyForNavigationAction:preferences:decisionHandler:))]
        fn decide_action(
            &self,
            _web: &WKWebView,
            action: &WKNavigationAction,
            preferences: &WKWebpagePreferences,
            handler: &DynBlock<dyn Fn(WKNavigationActionPolicy, NonNull<WKWebpagePreferences>)>,
        ) {
            let decision = guarded(web::Decision::Cancel, || {
                let decision = web::decide_action(action, self.project_root().as_deref(), self.page_folder().as_deref());
                match &decision {
                    web::Decision::NewTab(url) => tabs::open_tab_beside(self, url),
                    web::Decision::LoadFile(path) => tabs::go_later(self, Address::File(path.clone())),
                    _ => web::limit_scripts(action, preferences, self.is_trusted()),
                }
                decision
            });
            handler.call((web::action_policy(&decision), NonNull::from(preferences)));
        }

        #[unsafe(method(webView:decidePolicyForNavigationResponse:decisionHandler:))]
        fn decide_response(
            &self,
            _web: &WKWebView,
            response: &WKNavigationResponse,
            handler: &DynBlock<dyn Fn(WKNavigationResponsePolicy)>,
        ) {
            let policy = guarded(WKNavigationResponsePolicy::Allow, || web::decide_response(response));
            handler.call((policy,));
        }

        #[unsafe(method(webView:navigationAction:didBecomeDownload:))]
        fn action_became_download(&self, _web: &WKWebView, _action: &WKNavigationAction, download: &WKDownload) {
            guarded((), || web::adopt_download(download, &downloads::delegate(self.mtm())));
        }

        #[unsafe(method(webView:navigationResponse:didBecomeDownload:))]
        fn response_became_download(
            &self,
            _web: &WKWebView,
            _response: &WKNavigationResponse,
            download: &WKDownload,
        ) {
            guarded((), || web::adopt_download(download, &downloads::delegate(self.mtm())));
        }

        #[unsafe(method(webView:didCommitNavigation:))]
        fn did_commit(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>) {
            guarded((), || self.hide_failure());
        }

        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn did_fail_provisional(&self, _web: &WKWebView, _navigation: Option<&WKNavigation>, error: &NSError) {
            guarded((), || self.failed(error));
        }

        #[unsafe(method(webViewWebContentProcessDidTerminate:))]
        fn content_ended(&self, _web: &WKWebView) {
            guarded((), || self.show_failure(Failure::Crashed, self.current_url()));
        }

        #[unsafe(method(webView:didReceiveAuthenticationChallenge:completionHandler:))]
        fn authentication_challenge(
            &self,
            _web: &WKWebView,
            challenge: &NSURLAuthenticationChallenge,
            done: &DynBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>,
        ) {
            let done = done.copy();
            let fallback = done.clone();
            let answered = guarded(false, || {
                self.answer_challenge(challenge, done);
                true
            });
            if !answered {
                fallback.call((NSURLSessionAuthChallengeDisposition::RejectProtectionSpace, std::ptr::null_mut()));
            }
        }
    }

    // SAFETY: Each method below has its selector's signature and calls every completion handler exactly once.
    unsafe impl WKUIDelegate for BrowserView {
        #[unsafe(method_id(webView:createWebViewWithConfiguration:forNavigationAction:windowFeatures:))]
        fn create_web_view(
            &self,
            _web: &WKWebView,
            configuration: &WKWebViewConfiguration,
            _action: &WKNavigationAction,
            _features: &WKWindowFeatures,
        ) -> Option<Retained<WKWebView>> {
            guarded(None, || tabs::open_popup(self, configuration))
        }

        #[unsafe(method(webViewDidClose:))]
        fn did_close(&self, _web: &WKWebView) {
            guarded((), || tabs::close_later(self));
        }

        #[unsafe(method(webView:runJavaScriptAlertPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn run_alert(&self, _web: &WKWebView, message: &NSString, frame: &WKFrameInfo, done: &DynBlock<dyn Fn()>) {
            let asked = guarded(false, || {
                dialogs::alert(self.window().as_deref(), message, frame, done);
                true
            });
            // WebKit raises an exception when a panel's completion handler is never called.
            if !asked {
                done.call(());
            }
        }

        #[unsafe(method(webView:runJavaScriptConfirmPanelWithMessage:initiatedByFrame:completionHandler:))]
        fn run_confirm(
            &self,
            _web: &WKWebView,
            message: &NSString,
            frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(Bool)>,
        ) {
            let asked = guarded(false, || {
                dialogs::confirm(self.window().as_deref(), message, frame, done);
                true
            });
            if !asked {
                done.call((Bool::NO,));
            }
        }

        #[unsafe(method(webView:runJavaScriptTextInputPanelWithPrompt:defaultText:initiatedByFrame:completionHandler:))]
        fn run_prompt(
            &self,
            _web: &WKWebView,
            question: &NSString,
            default: Option<&NSString>,
            frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSString)>,
        ) {
            let asked = guarded(false, || {
                dialogs::prompt(self.window().as_deref(), question, default, frame, done);
                true
            });
            if !asked {
                done.call((std::ptr::null_mut(),));
            }
        }

        #[unsafe(method(webView:runOpenPanelWithParameters:initiatedByFrame:completionHandler:))]
        fn run_open_panel(
            &self,
            _web: &WKWebView,
            parameters: &WKOpenPanelParameters,
            _frame: &WKFrameInfo,
            done: &DynBlock<dyn Fn(*mut NSArray<NSURL>)>,
        ) {
            let asked = guarded(false, || {
                dialogs::choose_files(self.window().as_deref(), parameters, done);
                true
            });
            if !asked {
                done.call((std::ptr::null_mut(),));
            }
        }

        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn media_capture(
            &self,
            _web: &WKWebView,
            _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo,
            _kind: WKMediaCaptureType,
            decide: &DynBlock<dyn Fn(WKPermissionDecision)>,
        ) {
            // The app declares no camera or microphone use, so macOS would end it on the first access.
            decide.call((WKPermissionDecision::Deny,));
        }
    }

    impl BrowserView {
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            let shows = !self.isHiddenOrHasHiddenAncestor();
            let shortcuts = || {
                self.ivars()
                    .parts
                    .get()
                    .is_some_and(|parts| parts.shortcuts.performKeyEquivalent(event))
            };
            shows
                && (guarded(false, shortcuts)
                    // SAFETY: NSView's own implementation, which offers the event to the subviews.
                    || unsafe { msg_send![super(self), performKeyEquivalent: event] })
        }

        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            key_path: Option<&NSString>,
            object: Option<&AnyObject>,
            change: Option<&NSDictionary<NSString, AnyObject>>,
            context: *mut c_void,
        ) {
            if context == observation() {
                guarded((), || self.refresh());
                return;
            }
            // SAFETY: NSView's own implementation, with the arguments key-value observing passed.
            unsafe {
                let _: () = msg_send![
                    super(self),
                    observeValueForKeyPath: key_path,
                    ofObject: object,
                    change: change,
                    context: context
                ];
            }
        }

        #[unsafe(method(termBrowserBack:))]
        fn back_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || web::go_back(&self.parts().web));
        }

        #[unsafe(method(termBrowserForward:))]
        fn forward_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || web::go_forward(&self.parts().web));
        }

        #[unsafe(method(termBrowserReloadOrStop:))]
        fn reload_or_stop_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if web::state(&self.parts().web).loading {
                    web::stop(&self.parts().web);
                } else {
                    self.reload_page();
                }
            });
        }

        #[unsafe(method(termBrowserReload:))]
        fn reload_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.reload_page());
        }

        #[unsafe(method(termBrowserRetry:))]
        fn retry_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.reload_page());
        }

        #[unsafe(method(termBrowserFocusAddress:))]
        fn focus_address_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.focus_address());
        }

        #[unsafe(method(termBrowserOpenExternally:))]
        fn open_externally_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.open_externally());
        }

        #[unsafe(method(termBrowserFillLogin:))]
        fn fill_login_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.fill_login());
        }

        #[unsafe(method(termBrowserProceed:))]
        fn proceed_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.proceed());
        }

        #[unsafe(method(termBrowserShowDownload:))]
        fn show_download_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.parts().notice.reveal());
        }

        #[unsafe(method(termBrowserCloseNotice:))]
        fn close_notice_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.parts().notice.close());
        }
    }
);

impl Drop for BrowserView {
    fn drop(&mut self) {
        let Some(parts) = self.ivars().parts.get() else { return };
        let observer: &NSObject = self;
        for key in OBSERVED {
            // SAFETY: `build` registered this view for exactly these key paths with this context.
            unsafe {
                parts
                    .web
                    .removeObserver_forKeyPath_context(observer, &ns(key), observation())
            };
        }
        // SAFETY: Clearing the delegates asks for nothing.
        unsafe {
            parts.web.setNavigationDelegate(None);
            parts.web.setUIDelegate(None);
        }
        // SAFETY: Clearing the delegate asks for nothing.
        unsafe { parts.controls.field.setDelegate(None) };
    }
}

impl BrowserView {
    /// A tab's view in `workspace`, named `key` in the tab bar. A pop-up must use the configuration
    /// WebKit hands over; other tabs get a new one.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        workspace: WorkspaceId,
        configuration: Option<&WKWebViewConfiguration>,
        key: String,
    ) -> Retained<BrowserView> {
        let this = Self::unbuilt(mtm, workspace, key, None);
        let own;
        let configuration = match configuration {
            Some(configuration) => configuration,
            None => {
                own = web::configuration(mtm);
                &own
            }
        };
        this.build(mtm, configuration);
        this
    }

    /// A tab's view in `workspace` that shows `asked` and builds its page only once it first shows, then does `wake`.
    pub(crate) fn waiting(
        mtm: MainThreadMarker,
        workspace: WorkspaceId,
        key: String,
        asked: Address,
        wake: Wake,
    ) -> Retained<BrowserView> {
        let this = Self::unbuilt(mtm, workspace, key, Some(asked));
        *this.ivars().waiting.borrow_mut() = Some(wake);
        let view = Weak::from(&*this);
        term_ui::kit::panel::when_first_shown(&this, move || {
            if let Some(view) = view.load() {
                view.wake();
            }
        });
        this
    }

    fn unbuilt(
        mtm: MainThreadMarker,
        workspace: WorkspaceId,
        key: String,
        asked: Option<Address>,
    ) -> Retained<BrowserView> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            workspace,
            key,
            tab: Cell::new(None),
            parts: OnceCell::new(),
            waiting: RefCell::new(None),
            asked: RefCell::new(asked),
            failure: RefCell::new(None),
            title: RefCell::new(String::new()),
        });
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] }
    }

    /// Builds the page of a tab that waited to show, and does what it waited for.
    fn wake(&self) {
        if self.ivars().parts.get().is_some() {
            return;
        }
        let mtm = self.mtm();
        self.build(mtm, &web::configuration(mtm));
        let waiting = self.ivars().waiting.take();
        match waiting {
            Some(Wake::Go(address)) => self.go(address),
            Some(Wake::Run(run)) => {
                let view = Weak::from(self);
                run(Box::new(move |url| {
                    if let Some(view) = view.load() {
                        view.go(Address::of_url(url));
                    }
                }));
            }
            None => self.refresh(),
        }
    }

    fn build(&self, mtm: MainThreadMarker, configuration: &WKWebViewConfiguration) {
        let web = web::new_view(mtm, configuration);
        // SAFETY: WebKit holds both delegates weakly, and `Drop` clears them before this view goes.
        unsafe {
            web.setNavigationDelegate(Some(ProtocolObject::from_ref(self)));
            web.setUIDelegate(Some(ProtocolObject::from_ref(self)));
        }
        let controls = Controls::new(mtm, self);
        let failure = FailurePage::new(mtm, self);
        let notice = Notice::new(mtm, self);
        self.lay_out(&controls.row, notice.view(), &[&web, failure.view()]);
        let observer: &NSObject = self;
        for key in OBSERVED {
            // SAFETY: `Drop` removes the observation before this view goes; the context marks it as this view's.
            unsafe {
                web.addObserver_forKeyPath_options_context(
                    observer,
                    &ns(key),
                    NSKeyValueObservingOptions::empty(),
                    observation(),
                );
            }
        }
        let shortcuts = self.shortcuts(mtm);
        let _ = self.ivars().parts.set(Parts {
            web,
            controls,
            failure,
            notice,
            shortcuts,
        });
    }

    /// Stacks the navigation row, a separator and the notice bar, and gives the rest to the pages, which overlap.
    fn lay_out(&self, row: &NSView, notice: &NSView, pages: &[&NSView]) {
        let line = NSBox::new(self.mtm());
        line.setBoxType(NSBoxType::Separator);
        let mut above = None;
        for view in [row, &line, notice] {
            self.addSubview(view);
            view.setTranslatesAutoresizingMaskIntoConstraints(false);
            let top = match above {
                Some(above) => view.topAnchor().constraintEqualToAnchor(&NSView::bottomAnchor(above)),
                None => view.topAnchor().constraintEqualToAnchor(&self.topAnchor()),
            };
            top.setActive(true);
            pin_sides(view, self, 0.0);
            above = Some(view);
        }
        for page in pages {
            self.addSubview(page);
            page.setTranslatesAutoresizingMaskIntoConstraints(false);
            activate([
                page.topAnchor().constraintEqualToAnchor(&notice.bottomAnchor()),
                page.leadingAnchor().constraintEqualToAnchor(&self.leadingAnchor()),
                page.trailingAnchor().constraintEqualToAnchor(&self.trailingAnchor()),
                page.bottomAnchor().constraintEqualToAnchor(&self.bottomAnchor()),
            ]);
        }
    }

    /// The tab's shortcuts as in Safari, as items of a menu that `performKeyEquivalent:` asks, so they match keys
    /// as the menu bar does, but only while the tab shows.
    fn shortcuts(&self, mtm: MainThreadMarker) -> Retained<NSMenu> {
        let menu = NSMenu::new(mtm);
        for (key, title, action) in [
            ("cmd+l", t("Adresse"), sel!(termBrowserFocusAddress:)),
            ("cmd+r", t("Neu laden"), sel!(termBrowserReload:)),
            ("cmd+[", t("Zurück"), sel!(termBrowserBack:)),
            ("cmd+]", t("Vorwärts"), sel!(termBrowserForward:)),
        ] {
            let entry = item(mtm, title, Some(action), key);
            set_target(&entry, self);
            menu.addItem(&entry);
        }
        menu
    }

    fn parts(&self) -> &Parts {
        if self.ivars().parts.get().is_none() {
            self.wake();
        }
        self.ivars().parts.get().expect("`wake` builds the parts")
    }

    pub(crate) fn key(&self) -> &str {
        &self.ivars().key
    }

    pub(crate) fn tab(&self) -> Option<TabId> {
        self.ivars().tab.get()
    }

    pub(crate) fn set_tab(&self, id: TabId) {
        self.ivars().tab.set(Some(id));
    }

    pub(crate) fn workspace_id(&self) -> WorkspaceId {
        self.ivars().workspace
    }

    pub(crate) fn web_view(&self) -> Retained<WKWebView> {
        self.parts().web.clone()
    }

    /// Whether `web` is the page of this tab, without building one.
    pub(crate) fn shows_web(&self, web: &WKWebView) -> bool {
        self.ivars()
            .parts
            .get()
            .is_some_and(|parts| std::ptr::eq(&*parts.web, web))
    }

    fn project_root(&self) -> Option<PathBuf> {
        Workspace::new(self.workspace_id()).project_root()
    }

    fn is_trusted(&self) -> bool {
        Workspace::new(self.workspace_id()).is_trusted()
    }

    /// The folder the file page that shows may read, which `web::load_file` granted it.
    fn page_folder(&self) -> Option<PathBuf> {
        let parts = self.ivars().parts.get()?;
        let path = address::file_path(&web::state(&parts.web).url?)?;
        web::read_access(&path, &self.project_root()?)
    }

    /// The address of the page, or of what was asked for while there is none or it failed.
    pub(crate) fn current_url(&self) -> Option<String> {
        if self.ivars().failure.borrow().is_none()
            && let Some(parts) = self.ivars().parts.get()
            && let Some(url) = web::state(&parts.web).url
        {
            return Some(url);
        }
        self.asked_url()
    }

    fn asked_url(&self) -> Option<String> {
        match self.ivars().asked.borrow().as_ref()? {
            Address::Url(url) => Some(url.clone()),
            Address::File(path) => NSURL::fileURLWithPath(&ns(&path.to_string_lossy()))
                .absoluteString()
                .map(|s| s.to_string()),
        }
    }

    /// Shows `address`: a web page, a file of the workspace's folder, or the reason it does not.
    pub(crate) fn go(&self, address: Address) {
        self.ivars().waiting.take();
        let parts = self.parts();
        *self.ivars().asked.borrow_mut() = Some(address.clone());
        match &address {
            Address::Url(url) => {
                let scheme = address::scheme_of(url).unwrap_or_default();
                if !address::is_web_scheme(&scheme) || !web::load(&parts.web, url) {
                    self.show_failure(Failure::Unsupported, Some(url.clone()));
                }
            }
            Address::File(path) => {
                let loaded = self
                    .project_root()
                    .is_some_and(|root| web::load_file(&parts.web, path, &root));
                if !loaded {
                    self.show_failure(Failure::OutsideProject, self.asked_url());
                }
            }
        }
        self.refresh();
    }

    /// Takes the tab to `address` again. On the same local server the page it shows reloads when `address` names
    /// the server or that page, and another page of it loads; another site's page that already shows stays.
    pub(crate) fn revisit(&self, address: Address) {
        let current = self.current_url();
        let same = current
            .as_ref()
            .is_some_and(|url| address::key_of(url) == address.key());
        match &address {
            Address::Url(url) if same && address::is_local(url) => {
                if address::is_origin_only(url) || current.as_deref() == Some(url.as_str()) {
                    self.reload_page();
                } else {
                    self.go(address);
                }
            }
            _ if same => {}
            _ => self.go(address),
        }
    }

    /// Reloads the page, or asks again for what failed.
    pub(crate) fn reload_page(&self) {
        // A tab that waits to show loads its page fresh once it does.
        if self.ivars().parts.get().is_none() {
            return;
        }
        if self.ivars().failure.borrow().is_none() {
            web::reload(&self.parts().web);
            return;
        }
        let asked = self.ivars().asked.borrow().clone();
        if let Some(asked) = asked {
            self.go(asked);
        }
    }

    /// Shows the page that invites an address, with the field focused.
    pub(crate) fn start_empty(&self) {
        self.show_failure(Failure::Start, None);
        self.focus_address();
    }

    pub(crate) fn focus_address(&self) {
        let field = &self.parts().controls.field;
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(field));
        }
        // SAFETY: Selecting the field's text asks for nothing; a nil sender is allowed.
        unsafe { field.selectText(None) };
    }

    fn field_command(&self, command: Sel) -> bool {
        let field = &self.parts().controls.field;
        if command == sel!(insertNewline:) {
            let engine = super::host::host().search_engine();
            if let Some(address) = address::resolve(&field.stringValue().to_string(), engine) {
                self.go(address);
                if let Some(window) = self.window() {
                    window.makeFirstResponder(Some(&self.parts().web));
                }
            }
            return true;
        }
        if command == sel!(cancelOperation:) {
            field.setStringValue(&ns(&self.current_url().unwrap_or_default()));
            if let Some(editor) = field.currentEditor() {
                // SAFETY: The field's editor selects its text; a nil sender is allowed.
                unsafe { editor.selectAll(None) };
            }
            return true;
        }
        false
    }

    /// Runs the browser command `action` from the menu bar.
    pub(super) fn perform(&self, action: Sel) {
        let web = &self.parts().web;
        if action == sel!(termBrowserBack:) {
            web::go_back(web);
        } else if action == sel!(termBrowserForward:) {
            web::go_forward(web);
        } else if action == sel!(termBrowserReload:) {
            self.reload_page();
        } else if action == sel!(termBrowserFocusAddress:) {
            self.focus_address();
        }
    }

    pub(super) fn validate(&self, action: Option<Sel>) -> bool {
        let (back, forward) = self.ivars().parts.get().map_or((false, false), |parts| {
            let state = web::state(&parts.web);
            (state.back, state.forward)
        });
        match action {
            Some(action) if action == sel!(termBrowserBack:) => back,
            Some(action) if action == sel!(termBrowserForward:) => forward,
            _ => true,
        }
    }

    fn open_externally(&self) {
        let Some(url) = self.current_url().and_then(|url| NSURL::URLWithString(&ns(&url))) else {
            return;
        };
        if address::opens_externally(&url.absoluteString().map(|s| s.to_string()).unwrap_or_default()) {
            NSWorkspace::sharedWorkspace().openURL(&url);
        }
    }

    /// Asks for a login, which macOS AutoFill can offer, and writes it into the page's form; the page must still
    /// show the same host when it arrives.
    pub(super) fn fill_login(&self) {
        let (Some(url), Some(window)) = (self.current_url(), self.window()) else {
            return;
        };
        let scheme = address::scheme_of(&url);
        let Some(page) = address::origin(&url)
            .filter(|_| matches!(scheme.as_deref(), Some("http" | "https")))
            .and_then(|origin| origin.split_once("://").map(|(_, authority)| authority.to_string()))
        else {
            return;
        };
        let host = address::host_of(&url).unwrap_or_else(|| page.clone());
        let plain = scheme.as_deref() == Some("http") && !address::is_local(&url);
        let protocol = format!("{}:", scheme.as_deref().unwrap_or_default());
        let view = Weak::from(self);
        dialogs::fill_login(&window, &host, plain, move |user, password| {
            let Some(view) = view.load() else { return };
            let weak = Weak::from(&*view);
            autofill::fill(&view.parts().web, &protocol, &page, &user, &password, move |filled| {
                if filled == 0
                    && let Some(view) = weak.load()
                {
                    view.parts().notice.show_login(t(
                        "Auf dieser Seite gibt es kein Anmeldeformular, oder sie hat inzwischen gewechselt.",
                    ));
                }
            });
        });
    }

    /// Answers an authentication challenge of the page's server through `done`, now or once the user did.
    fn answer_challenge(
        &self,
        challenge: &NSURLAuthenticationChallenge,
        done: block2::RcBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>,
    ) {
        let method = challenge.protectionSpace().authenticationMethod();
        let is = |known: &NSString| &*method == known;
        if is(constant!(objc2_foundation::NSURLAuthenticationMethodServerTrust)) {
            let (disposition, credential) = trust::answer(challenge);
            let pointer = credential
                .as_ref()
                .map_or(std::ptr::null_mut(), |c| Retained::as_ptr(c).cast_mut());
            done.call((disposition, pointer));
        } else if is(constant!(objc2_foundation::NSURLAuthenticationMethodHTTPBasic))
            || is(constant!(objc2_foundation::NSURLAuthenticationMethodHTTPDigest))
        {
            dialogs::login(self.window().as_deref(), challenge, done);
        } else {
            done.call((
                NSURLSessionAuthChallengeDisposition::RejectProtectionSpace,
                std::ptr::null_mut(),
            ));
        }
    }

    /// Accepts the certificate of the development server whose page failed, and asks for the page again.
    pub(super) fn proceed(&self) {
        if self.current_url().is_some_and(|url| trust::accept(&url)) {
            self.reload_page();
        }
    }

    /// Tells in the tab that `file` finished downloading.
    pub(super) fn downloaded(&self, file: &NSURL) {
        self.parts().notice.show_download(file);
    }

    fn failed(&self, error: &NSError) {
        let Some(failure) = Failure::of_error(error) else {
            return;
        };
        let key = constant!(objc2_foundation::NSURLErrorFailingURLErrorKey);
        let url = web::error_detail(error, key).or_else(|| self.asked_url());
        self.show_failure(failure, url);
    }

    /// Shows the native page for `failure` at `url`, which "Try Again" then asks for.
    fn show_failure(&self, failure: Failure, url: Option<String>) {
        let parts = self.parts();
        let proceed = failure == Failure::Insecure && url.as_deref().is_some_and(trust::can_accept);
        parts.failure.show(&failure, url.as_deref(), proceed);
        parts.web.setHidden(true);
        if let Some(url) = url {
            *self.ivars().asked.borrow_mut() = Some(Address::of_url(&url));
        }
        *self.ivars().failure.borrow_mut() = Some(failure);
        self.refresh();
    }

    fn hide_failure(&self) {
        let parts = self.parts();
        parts.failure.hide();
        parts.web.setHidden(false);
        *self.ivars().failure.borrow_mut() = None;
        self.refresh();
    }

    /// Brings the controls and the tab title in line with the page.
    fn refresh(&self) {
        let Some(parts) = self.ivars().parts.get() else { return };
        let state = web::state(&parts.web);
        let controls = &parts.controls;
        let url = self.current_url();
        if controls.field.currentEditor().is_none() {
            controls.field.setStringValue(&ns(url.as_deref().unwrap_or_default()));
        }
        controls.back.setEnabled(state.back);
        controls.forward.setEnabled(state.forward);
        controls.reload.setEnabled(state.loading || url.is_some());
        controls.set_loading(state.loading);
        let external = url.as_deref().is_some_and(address::opens_externally);
        controls.external.setEnabled(external);
        let web_page = url
            .as_deref()
            .and_then(address::scheme_of)
            .is_some_and(|scheme| matches!(scheme.as_str(), "http" | "https"));
        controls
            .login
            .setEnabled(web_page && self.ivars().failure.borrow().is_none());
        self.update_title(&state);
    }

    fn update_title(&self, state: &web::State) {
        let failed = self.ivars().failure.borrow().is_some();
        let title = if failed { "" } else { state.title.as_str() };
        let url = self.current_url();
        let title = address::tab_title(title, url.as_deref(), TITLE_LIMIT).unwrap_or_else(|| t("Browser").to_string());
        if *self.ivars().title.borrow() == title {
            return;
        }
        let Some(tab) = self.tab() else { return };
        if self.window().is_none() {
            return;
        }
        let workspace = Workspace::new(self.workspace_id());
        if workspace.is_open() {
            workspace.set_tab_title(tab, &title);
            *self.ivars().title.borrow_mut() = title;
        }
    }
}
