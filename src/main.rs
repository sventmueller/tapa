//! Tapa, a terminal for macOS on AppKit: its windows with workspaces, tabs and panes, the menus built from
//! the key bindings of the settings, and the start that hands the crates the product's identity (E3).

mod about;
mod agent;
mod app;
mod browser;
mod dictation;
mod editor_tab;
mod feedback;
mod files;
mod host;
mod hosted;
mod menu;
mod migrate;
mod notify;
mod open;
mod palette;
mod panels;
mod preview_tab;
mod quick;
mod quit;
mod recent;
mod restore;
#[cfg(feature = "selftest")]
mod selftest;
mod settings;
mod state;
mod switcher;
mod tab;
mod updates;
mod window;
mod workspace;

use objc2::MainThreadMarker;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use objc2_foundation::NSBundle;

fn main() {
    #[cfg(feature = "selftest")]
    if let Some(code) = selftest::stand_in::run() {
        std::process::exit(code);
    }
    splat_i18n::install(&localized_strings());
    let bundle_id = NSBundle::mainBundle()
        .bundleIdentifier()
        .map_or_else(|| term_product::BUNDLE_ID.to_string(), |id| id.to_string());
    term_base::set_identity(term_base::Identity {
        name: term_product::NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        bundle_id,
    });
    term_base::diag::install(Box::new(|what, detail| eprintln!("{what}: {detail}")));
    migrate::run();
    term_ui::install(&host::AppHost);
    let mtm = MainThreadMarker::new().expect("the app starts on the main thread");
    state::start(mtm);
    agent::install();
    browser::install();
    editor_tab::install();
    files::install();
    panels::install();
    preview_tab::start(mtm);
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = app::AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    dictation::install();
    menu::install(&app, &delegate, mtm);
    quick::install();
    app.run();
}

/// The interface texts of the language macOS picked from the app's resources.
fn localized_strings() -> String {
    NSBundle::mainBundle()
        .pathForResource_ofType(
            Some(&term_ui::kit::ns("Localizable")),
            Some(&term_ui::kit::ns("strings")),
        )
        .and_then(|path| std::fs::read_to_string(path.to_string()).ok())
        .unwrap_or_default()
}
