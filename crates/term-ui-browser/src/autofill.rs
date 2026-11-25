//! Fills a page's login form with a login the user took from macOS AutoFill. WebKit gives a web view outside Safari
//! no password AutoFill, so the app asks in a native panel, whose fields AutoFill serves, and writes into the form.

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_foundation::{NSDictionary, NSError, NSNumber};
use objc2_web_kit::{WKContentWorld, WKWebView};

use term_ui::kit::{guarded, ns};

/// Finds the form's user name and password fields and sets them as typing would, so the page's scripts see it.
const SCRIPT: &str = include_str!("fill_login.js");

/// Writes `user` and `password` into the login form of `web`'s page if it still shows `host`, and still encrypted
/// where `protocol`, the page's when the user was asked, is `https:`; `done` gets how many fields took a value, none
/// on a page without a login form.
pub(crate) fn fill(
    web: &WKWebView,
    protocol: &str,
    host: &str,
    user: &str,
    password: &str,
    done: impl Fn(usize) + 'static,
) {
    let keys = [&*ns("protocol"), &*ns("host"), &*ns("user"), &*ns("password")];
    let values: Vec<Retained<AnyObject>> = [protocol, host, user, password]
        .into_iter()
        .map(|value| ns(value).into_super().into_super())
        .collect();
    let arguments = NSDictionary::from_retained_objects(&keys, &values);
    let mtm: MainThreadMarker = web.mtm();
    let handler = RcBlock::new(move |result: *mut AnyObject, _error: *mut NSError| {
        guarded((), || {
            // SAFETY: WebKit hands over the script's result, or nil, valid for the duration of the call.
            let result = unsafe { result.as_ref() };
            let count = result
                .and_then(|value| value.downcast_ref::<NSNumber>())
                .map_or(0, |number| usize::try_from(number.as_isize()).unwrap_or(0));
            done(count);
        });
    });
    // SAFETY: The arguments are plain strings; the main frame is the one whose host the script checks, and the
    // client world keeps the page's own scripts from replacing what the script calls.
    unsafe {
        web.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &ns(SCRIPT),
            Some(&arguments),
            None,
            &WKContentWorld::defaultClientWorld(mtm),
            Some(&handler),
        );
    }
}
