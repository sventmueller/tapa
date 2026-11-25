//! The microphone button of a text field, with a spinner beside it while a recording is being transcribed, and the
//! short line that says what dictation does.

use std::time::Duration;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSAccessibility, NSAnimatablePropertyContainer, NSButton, NSColor, NSControlSize, NSImageSymbolConfiguration,
    NSProgressIndicator, NSProgressIndicatorStyle, NSStackView, NSTextField, NSUserInterfaceLayoutOrientation,
    NSWorkspace,
};
use splat_i18n::t;

use super::desk::Phase;
use crate::util::{constant, ns, set_action, symbol};

/// How long one half of the button's pulse takes while it listens.
pub(super) const PULSE: Duration = Duration::from_millis(650);

/// What the controls of a field show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Shown {
    pub(crate) phase: Phase,
    pub(crate) waiting: bool,
    pub(crate) transcribing: bool,
    /// The line beside the controls: what happens, or how the last dictation ended.
    pub(crate) line: String,
    pub(crate) error: bool,
    /// The shortcut, as the menu writes it, for the tooltip.
    pub(crate) shortcut: String,
}

impl Shown {
    pub(crate) fn recording(&self) -> bool {
        self.phase != Phase::Idle || self.waiting
    }
}

/// A field's microphone button and spinner, side by side.
#[derive(Clone)]
pub struct Mic {
    /// The stack that holds both.
    pub view: Retained<NSStackView>,
    /// The microphone button.
    pub button: Retained<NSButton>,
    pub(crate) spinner: Retained<NSProgressIndicator>,
}

impl Mic {
    /// The controls at `points`, sending `action` to `target`; the tag names the field once it is registered.
    pub(crate) fn new(mtm: MainThreadMarker, points: f64, target: &AnyObject, action: Sel) -> Mic {
        let image = symbol("mic", t("Diktat starten")).unwrap_or_default();
        // SAFETY: Without a target the button sends nothing until `set_action` gives it one, right below.
        let button = unsafe { NSButton::buttonWithImage_target_action(&image, None, None, mtm) };
        set_action(&button, target, action);
        button.setBordered(false);
        button.setSymbolConfiguration(Some(&NSImageSymbolConfiguration::configurationWithPointSize_weight(
            points,
            constant!(objc2_app_kit::NSFontWeightRegular),
        )));
        button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
        let spinner = NSProgressIndicator::new(mtm);
        spinner.setStyle(NSProgressIndicatorStyle::Spinning);
        spinner.setControlSize(NSControlSize::Small);
        spinner.setDisplayedWhenStopped(false);
        spinner.setHidden(true);
        let view = NSStackView::new(mtm);
        view.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        view.setSpacing(term_ui::kit::space::XS);
        view.addArrangedSubview(&spinner);
        view.addArrangedSubview(&button);
        let mic = Mic { view, button, spinner };
        mic.show(&Shown {
            phase: Phase::Idle,
            waiting: false,
            transcribing: false,
            line: String::new(),
            error: false,
            shortcut: String::new(),
        });
        mic
    }

    pub(crate) fn set_field(&self, tag: isize) {
        self.button.setTag(tag);
    }

    pub(crate) fn show(&self, shown: &Shown) {
        let (name, label, tint) = if shown.recording() {
            ("mic.fill", t("Diktat beenden"), NSColor::systemRedColor())
        } else {
            ("mic", t("Diktat starten"), NSColor::secondaryLabelColor())
        };
        self.button.setImage(symbol(name, label).as_deref());
        self.button.setContentTintColor(Some(&tint));
        let tip = if shown.shortcut.is_empty() {
            label.to_string()
        } else {
            format!("{label} ({})", shown.shortcut)
        };
        self.button.setToolTip(Some(&ns(&tip)));
        self.button.setAccessibilityLabel(Some(&ns(label)));
        let value = ns(&shown.line);
        // SAFETY: A string is a valid accessibility value of a button.
        unsafe { self.button.setAccessibilityValue(Some(&value)) };
        if shown.phase != Phase::Listening {
            self.button.setAlphaValue(1.0);
        }
        self.spinner.setHidden(!shown.transcribing);
        if shown.transcribing {
            // SAFETY: Starting the animation takes no sender.
            unsafe { self.spinner.startAnimation(None) };
        } else {
            // SAFETY: Stopping the animation takes no sender, and a nil one is allowed.
            unsafe { self.spinner.stopAnimation(None) };
        }
    }

    /// One step of the pulse while the field listens; `bright` says which way.
    pub(crate) fn pulse(&self, bright: bool) {
        let alpha = if bright { 1.0 } else { 0.35 };
        self.button.animator().setAlphaValue(alpha);
    }
}

/// The line that says what dictation does, in the field's status bar.
pub(super) fn line(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let line = NSTextField::labelWithString(&ns(""), mtm);
    line.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(
        term_ui::kit::style::SMALL,
    )));
    line.setTextColor(Some(&NSColor::secondaryLabelColor()));
    line.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    line.setContentCompressionResistancePriority_forOrientation(
        120.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    line.setHidden(true);
    line
}

pub(super) fn show_line(line: &NSTextField, shown: &Shown) {
    line.setStringValue(&ns(&shown.line));
    line.setToolTip(Some(&ns(&shown.line)));
    line.setHidden(shown.line.is_empty());
    let color = if shown.error {
        NSColor::systemRedColor()
    } else {
        NSColor::secondaryLabelColor()
    };
    line.setTextColor(Some(&color));
}

/// Whether the user asked for less motion, which leaves out the pulse.
pub(super) fn reduce_motion() -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

#[cfg(feature = "selftest")]
mod selftest_probe;
