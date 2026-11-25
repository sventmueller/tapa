//! The navigation row above the page: back, forward, reload or stop, the address field, the loading indicator,
//! filling in a login and "open in the default browser". A horizontal stack lays them out from leading to trailing,
//! so the row mirrors in right-to-left languages; the address itself stays left to right like code (E3).

use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSControlSize, NSLayoutAttribute, NSLayoutConstraintOrientation, NSLineBreakMode,
    NSProgressIndicator, NSProgressIndicatorStyle, NSStackView, NSTextField, NSTextFieldBezelStyle,
    NSUserInterfaceLayoutOrientation, NSWritingDirection,
};
use objc2_foundation::NSEdgeInsets;
use splat_i18n::t;

use super::view::BrowserView;
use term_ui::kit::{ns, symbol, symbol_button};

pub(crate) struct Controls {
    pub(crate) row: Retained<NSStackView>,
    pub(crate) back: Retained<NSButton>,
    pub(crate) forward: Retained<NSButton>,
    pub(crate) reload: Retained<NSButton>,
    pub(crate) field: Retained<NSTextField>,
    pub(crate) spinner: Retained<NSProgressIndicator>,
    pub(crate) login: Retained<NSButton>,
    pub(crate) external: Retained<NSButton>,
}

impl Controls {
    pub(crate) fn new(mtm: MainThreadMarker, target: &BrowserView) -> Controls {
        let button = |name: &str, description: &str, action: Sel| {
            let button = symbol_button(
                mtm,
                name,
                description,
                term_ui::kit::style::symbol::MEDIUM,
                Some(target),
                action,
            );
            term_ui::kit::set_label(&button, Some(description));
            button.widthAnchor().constraintEqualToConstant(26.0).setActive(true);
            button
        };
        let back = button("chevron.backward", t("Zurück"), sel!(termBrowserBack:));
        let forward = button("chevron.forward", t("Vorwärts"), sel!(termBrowserForward:));
        let reload = button("arrow.clockwise", t("Neu laden"), sel!(termBrowserReloadOrStop:));
        let login = button("key", t("Anmeldung ausfüllen"), sel!(termBrowserFillLogin:));
        let external = button(
            "arrow.up.forward.app",
            t("In Standardbrowser öffnen"),
            sel!(termBrowserOpenExternally:),
        );
        let field = address_field(mtm, target);
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        spinner.setAccessibilityLabel(Some(&ns(t("Seite lädt"))));
        let row = NSStackView::new(mtm);
        row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        row.setAlignment(NSLayoutAttribute::CenterY);
        row.setSpacing(term_ui::kit::style::space::XXS);
        // The back button's symbol lines up with the tabs and the headings of the other tabs, its padding taken off.
        row.setEdgeInsets(NSEdgeInsets {
            top: 5.0,
            left: 0.0,
            bottom: 5.0,
            right: 8.0,
        });
        row.setAccessibilityLabel(Some(&ns(t("Navigation"))));
        for view in [
            &*back as &objc2_app_kit::NSView,
            &forward,
            &reload,
            &field,
            &spinner,
            &login,
            &external,
        ] {
            row.addArrangedSubview(view);
        }
        row.setCustomSpacing_afterView(term_ui::kit::style::space::M, &reload);
        row.setCustomSpacing_afterView(term_ui::kit::style::space::S, &field);
        row.setCustomSpacing_afterView(term_ui::kit::style::space::S, &spinner);
        Controls {
            row,
            back,
            forward,
            reload,
            field,
            spinner,
            login,
            external,
        }
    }

    /// Shows reload or stop on the button that does either, and the indicator while the page loads.
    pub(crate) fn set_loading(&self, loading: bool) {
        let (name, description) = if loading {
            ("xmark", t("Laden stoppen"))
        } else {
            ("arrow.clockwise", t("Neu laden"))
        };
        self.reload.setImage(symbol(name, description).as_deref());
        self.reload.setToolTip(Some(&ns(description)));
        term_ui::kit::set_label(&self.reload, Some(description));
        // SAFETY: The indicator is in the view hierarchy on the main thread; a nil sender is allowed.
        unsafe {
            if loading {
                self.spinner.startAnimation(None);
            } else {
                self.spinner.stopAnimation(None);
            }
        }
    }
}

fn address_field(mtm: MainThreadMarker, target: &BrowserView) -> Retained<NSTextField> {
    let field = NSTextField::textFieldWithString(&ns(""), mtm);
    field.setPlaceholderString(Some(&ns(t("Suchen oder Adresse eingeben"))));
    field.setBezelStyle(NSTextFieldBezelStyle::RoundedBezel);
    field.setUsesSingleLineMode(true);
    field.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    field.setBaseWritingDirection(NSWritingDirection::LeftToRight);
    term_ui::kit::set_label(&field, Some(t("Adresse")));
    // SAFETY: The field keeps its delegate weakly, and the view, which owns the field, clears it when it goes.
    unsafe { field.setDelegate(Some(ProtocolObject::from_ref(target))) };
    field.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Horizontal);
    field.setContentCompressionResistancePriority_forOrientation(250.0, NSLayoutConstraintOrientation::Horizontal);
    field
}
