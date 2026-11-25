//! System notifications (F63) for what programs announce with OSC 9 or OSC 777 and agents report while their tab is
//! not in front; a click on one brings the session or the agent's tab forward.

use std::cell::{Cell, OnceCell};

use block2::{DynBlock, RcBlock};
use objc2::rc::Retained;
use objc2::runtime::{Bool, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_foundation::{NSBundle, NSError, NSObject, NSObjectProtocol};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationPresentationOptions,
    UNNotificationRequest, UNNotificationResponse, UNNotificationSound, UNUserNotificationCenter,
    UNUserNotificationCenterDelegate,
};
use term_base::guarded;
use term_session::event::Notification;
use term_ui::kit::ns;

/// What starts the identifier of a session's notification; one per session, so a new one replaces the last.
const PREFIX: &str = "session-";
/// What starts the identifier of an agent tab's notification, followed by its workspace and its key.
const AGENT_PREFIX: &str = "agent-";

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Delegate` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[name = "TermNotificationDelegate"]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl UNUserNotificationCenterDelegate for Delegate {
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            let shown = guarded(UNNotificationPresentationOptions::empty(), || {
                UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound
            });
            completion.call((shown,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &DynBlock<dyn Fn()>,
        ) {
            guarded((), || {
                let identifier = response.notification().request().identifier().to_string();
                if let Some(view) = view_of(&identifier) {
                    term_base::main_soon(move || crate::switcher::bring_forward(view));
                } else if let Some((workspace, key)) = agent_of(&identifier) {
                    term_base::main_soon(move || crate::agent::bring_forward(workspace, &key));
                }
            });
            completion.call(());
        }
    }
);

thread_local! {
    static CENTER: OnceCell<Option<(Retained<UNUserNotificationCenter>, Retained<Delegate>)>> =
        const { OnceCell::new() };
    static ASKED: Cell<bool> = const { Cell::new(false) };
}

/// The notification center with its delegate set, unless the app runs outside a bundle, where it has none.
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    CENTER.with(|cell| {
        cell.get_or_init(|| {
            // `currentNotificationCenter` raises an exception in a process without a bundle identifier.
            NSBundle::mainBundle().bundleIdentifier()?;
            let center = UNUserNotificationCenter::currentNotificationCenter();
            let delegate: Retained<Delegate> = {
                let this = Delegate::alloc().set_ivars(());
                // SAFETY: NSObject's designated initializer, called once on the newly allocated object.
                unsafe { msg_send![super(this), init] }
            };
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            Some((center, delegate))
        })
        .as_ref()
        .map(|(center, _)| center.clone())
    })
}

/// Shows `notification` from session `view` as a system notification, titled `heading` where it has no title.
pub(crate) fn post(view: u64, notification: &Notification, heading: &str) {
    let (title, body) = parts(notification, heading);
    show(&format!("{PREFIX}{view}"), &title, &body);
}

/// Shows what the agent tab `key` of workspace `workspace` tells the user who looks elsewhere.
pub(crate) fn post_agent(workspace: u64, key: &str, title: &str, body: &str) {
    show(&format!("{AGENT_PREFIX}{workspace}-{key}"), title, body);
}

/// Shows a notification under `identifier`, which replaces the one shown under it before.
fn show(identifier: &str, title: &str, body: &str) {
    let Some(center) = center() else { return };
    if !ASKED.replace(true) {
        let answered = RcBlock::new(|_granted: Bool, _error: *mut NSError| {});
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
            &answered,
        );
    }
    let content = UNMutableNotificationContent::new();
    content.setTitle(&ns(title));
    content.setBody(&ns(body));
    content.setSound(Some(&UNNotificationSound::defaultSound()));
    content.setThreadIdentifier(&ns(identifier));
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(&ns(identifier), &content, None);
    center.addNotificationRequest_withCompletionHandler(&request, None);
}

/// The notification's title and text: its own title, else `heading`; with no text, the title alone as the text.
fn parts(notification: &Notification, heading: &str) -> (String, String) {
    let title = notification.title.clone().unwrap_or_else(|| heading.to_string());
    if notification.body.is_empty() {
        return (heading.to_string(), title);
    }
    (title, notification.body.clone())
}

/// What the sidebar shows of `notification`.
pub(crate) fn summary(notification: &Notification) -> String {
    match &notification.title {
        Some(title) if !notification.body.is_empty() => format!("{title}: {}", notification.body),
        Some(title) => title.clone(),
        None => notification.body.clone(),
    }
}

/// The session a notification's identifier names.
fn view_of(identifier: &str) -> Option<u64> {
    identifier.strip_prefix(PREFIX)?.parse().ok()
}

/// The workspace and the agent tab's key an agent's notification identifier names.
fn agent_of(identifier: &str) -> Option<(u64, String)> {
    let (workspace, key) = identifier.strip_prefix(AGENT_PREFIX)?.split_once('-')?;
    Some((workspace.parse().ok()?, key.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn notification(title: Option<&str>, body: &str) -> Notification {
        Notification {
            title: title.map(str::to_string),
            body: body.into(),
        }
    }

    #[test]
    fn a_notification_without_title_takes_the_tabs_heading() {
        let parts = |title, body| parts(&notification(title, body), "~/Repos/tapa");
        assert_eq!(parts(None, "Build done"), ("~/Repos/tapa".into(), "Build done".into()));
        assert_eq!(parts(Some("cargo"), "3 failed"), ("cargo".into(), "3 failed".into()));
        assert_eq!(parts(Some("Done"), ""), ("~/Repos/tapa".into(), "Done".into()));
    }

    #[test]
    fn the_sidebar_shows_title_and_text_in_one_line() {
        assert_eq!(summary(&notification(Some("cargo"), "3 failed")), "cargo: 3 failed");
        assert_eq!(summary(&notification(Some("Done"), "")), "Done");
        assert_eq!(summary(&notification(None, "Build done")), "Build done");
    }

    #[test]
    fn the_identifier_names_the_session() {
        assert_eq!(view_of(&format!("{PREFIX}42")), Some(42));
        assert_eq!(view_of("other-42"), None);
        assert_eq!(agent_of("agent-7-agent:3"), Some((7, "agent:3".into())));
        assert_eq!(agent_of(&format!("{PREFIX}42")), None);
    }
}
