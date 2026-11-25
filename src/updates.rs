//! Updates through Sparkle (F75), which only release bundles carry and only those whose Info.plist names a feed turn
//! on; Sparkle asks on the second launch whether it may check on its own, and until then the app makes no request.

use std::cell::OnceCell;

use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::NSMenuItem;
use objc2_foundation::NSBundle;
use splat_i18n::t;
use term_ui::kit::{item, ns, set_target};

thread_local! {
    static CONTROLLER: OnceCell<Option<Retained<AnyObject>>> = const { OnceCell::new() };
}

/// Loads Sparkle from the app's Frameworks folder and starts the updater, once; none in builds without it.
fn controller() -> Option<Retained<AnyObject>> {
    CONTROLLER.with(|cell| cell.get_or_init(load).clone())
}

fn load() -> Option<Retained<AnyObject>> {
    let main = NSBundle::mainBundle();
    main.objectForInfoDictionaryKey(&ns("SUFeedURL"))?;
    let url = main
        .privateFrameworksURL()?
        .URLByAppendingPathComponent(&ns("Sparkle.framework"))?;
    let bundle = NSBundle::bundleWithURL(&url)?;
    // SAFETY: Loading runs no initializers that need the app. The hardened runtime refuses the framework unless it
    // carries the app's Team ID, which the release signature gives it.
    if !unsafe { bundle.load() } {
        return None;
    }
    let class = AnyClass::get(c"SPUStandardUpdaterController")?;
    // SAFETY: Sparkle's documented initializer on a freshly allocated instance; both delegates are optional.
    unsafe {
        let allocated: Allocated<AnyObject> = msg_send![class, alloc];
        msg_send![
            allocated,
            initWithStartingUpdater: true,
            updaterDelegate: None::<&AnyObject>,
            userDriverDelegate: None::<&AnyObject>
        ]
    }
}

/// "Nach Updates suchen …" for the app menu, or none without Sparkle; Sparkle enables it while no check runs.
pub(crate) fn menu_item(mtm: MainThreadMarker) -> Option<Retained<NSMenuItem>> {
    let controller = controller()?;
    let check = item(mtm, t("Nach Updates suchen …"), Some(sel!(checkForUpdates:)), "");
    set_target(&check, &controller);
    Some(check)
}
