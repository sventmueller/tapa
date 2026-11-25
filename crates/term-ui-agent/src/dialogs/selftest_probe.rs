//! What the self-test reads from and does with an agent tab's overlay, kept out of its production code.

use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSTextField, NSView};

use crate::tab::AgentTab;

/// The texts and button titles under `view`, in the order of its subviews.
fn texts_in(view: &NSView, into: &mut Vec<String>) {
    let object: &objc2::runtime::AnyObject = view;
    if let Some(field) = object.downcast_ref::<NSTextField>() {
        if !field.isHidden() {
            into.push(field.stringValue().to_string());
        }
    } else if let Some(button) = object.downcast_ref::<NSButton>() {
        let title = button.title().to_string();
        if !title.is_empty() {
            into.push(title);
        }
    }
    for sub in view.subviews().iter() {
        if !sub.isHidden() {
            texts_in(&sub, into);
        }
    }
}

/// The button titled `title` under `view`.
fn button_in(view: &NSView, title: &str) -> Option<Retained<NSButton>> {
    if view.isHidden() {
        return None;
    }
    let object: &objc2::runtime::AnyObject = view;
    if let Some(button) = object.downcast_ref::<NSButton>()
        && button.title().to_string() == title
    {
        return Some(objc2::Message::retain(button));
    }
    view.subviews().iter().find_map(|sub| button_in(&sub, title))
}

/// The controls under `view` in order, each as a token: `switch`, `popup`, `field`, or a button's title; the
/// frame's own buttons (close, the way to a waiting request) and hidden views left out.
fn controls_in(view: &NSView, into: &mut Vec<String>) {
    use objc2_app_kit::NSAccessibility;
    let object: &objc2::runtime::AnyObject = view;
    if object.downcast_ref::<objc2_app_kit::NSSwitch>().is_some() {
        into.push("switch".into());
    } else if object.downcast_ref::<objc2_app_kit::NSPopUpButton>().is_some() {
        into.push("popup".into());
    } else if let Some(button) = object.downcast_ref::<NSButton>() {
        let title = button.title().to_string();
        let label = button.accessibilityLabel().map(|l| l.to_string()).unwrap_or_default();
        if title.is_empty() && label != splat_i18n::t("Schließen") {
            into.push(label);
        } else if !title.is_empty() && title != splat_i18n::t("Zur Anfrage") {
            into.push(title);
        }
    } else if let Some(field) = object.downcast_ref::<NSTextField>()
        && field.isEditable()
    {
        into.push("field".into());
    }
    for sub in view.subviews().iter() {
        if !sub.isHidden() {
            controls_in(&sub, into);
        }
    }
}

impl AgentTab {
    /// The controls of the open overlay in order, without their values, to compare overlays of one function across
    /// agents; none while none is open.
    pub fn dialog_controls(&self) -> Option<Vec<String>> {
        let open = self.dialog.borrow().clone()?;
        let mut controls = Vec::new();
        controls_in(&open.frame.shade, &mut controls);
        Some(controls)
    }

    /// Whether the open overlay is a modal group VoiceOver names after its title.
    pub fn dialog_is_modal(&self) -> bool {
        use objc2_app_kit::NSAccessibility;
        let Some(open) = self.dialog.borrow().clone() else {
            return false;
        };
        let card = find_in::<NSView>(&open.frame.shade, &|view| view.isAccessibilityModal());
        card.is_some_and(|card| card.accessibilityLabel().is_some_and(|l| !l.to_string().is_empty()))
    }

    /// The texts of the open overlay, title first; none while none is open.
    pub fn dialog_texts(&self) -> Option<Vec<String>> {
        let open = self.dialog.borrow().clone()?;
        let mut texts = Vec::new();
        texts_in(&open.frame.shade, &mut texts);
        Some(texts)
    }

    /// Clicks the overlay's button titled `title`, as the mouse does; whether there is one.
    pub fn press_dialog_button(&self, title: &str) -> bool {
        let Some(open) = self.dialog.borrow().clone() else {
            return false;
        };
        let Some(button) = button_in(&open.frame.shade, title) else {
            return false;
        };
        // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
        unsafe { button.performClick(None) };
        true
    }

    /// Presses Esc where the keyboard focus is, as the user does with the overlay open.
    pub fn press_escape(&self) {
        let Some(window) = self.root.window() else { return };
        let event = objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            objc2_app_kit::NSEventType::KeyDown,
            objc2_foundation::NSPoint::new(0.0, 0.0),
            objc2_app_kit::NSEventModifierFlags::empty(),
            0.0,
            window.windowNumber(),
            None,
            &crate::common::util::ns("\u{1b}"),
            &crate::common::util::ns("\u{1b}"),
            false,
            53,
        );
        if let (Some(event), Some(responder)) = (event, window.firstResponder()) {
            responder.keyDown(&event);
        }
    }
}

impl AgentTab {
    /// Closes the open overlay, as its close button does.
    pub fn dismiss_dialog(&self) {
        self.close_dialog();
    }
}

/// The control under `view` that `pick` finds.
fn find_in<T: objc2::Message + objc2::DowncastTarget>(view: &NSView, pick: &dyn Fn(&T) -> bool) -> Option<Retained<T>> {
    let object: &objc2::runtime::AnyObject = view;
    if let Some(found) = object.downcast_ref::<T>()
        && pick(found)
    {
        return Some(objc2::Message::retain(found));
    }
    view.subviews().iter().find_map(|sub| find_in(&sub, pick))
}

impl AgentTab {
    /// Flips the overlay's switch VoiceOver calls `label`, as a click does; whether there is one.
    pub fn press_dialog_switch(&self, label: &str) -> bool {
        use objc2_app_kit::NSAccessibility;
        let Some(open) = self.dialog.borrow().clone() else {
            return false;
        };
        let found = find_in::<objc2_app_kit::NSSwitch>(&open.frame.shade, &|switch| {
            switch.accessibilityLabel().is_some_and(|l| l.to_string() == label)
        });
        let Some(switch) = found else { return false };
        // SAFETY: A click without a sender, on the main thread, flips the switch and sends its action.
        unsafe { switch.performClick(None) };
        true
    }

    /// Types `text` into the overlay's field whose placeholder is `placeholder`; whether there is one.
    pub fn fill_dialog_field(&self, placeholder: &str, text: &str) -> bool {
        let Some(open) = self.dialog.borrow().clone() else {
            return false;
        };
        let found = find_in::<NSTextField>(&open.frame.shade, &|field| {
            field.placeholderString().is_some_and(|p| p.to_string() == placeholder)
        });
        let Some(field) = found else { return false };
        field.setStringValue(&crate::common::util::ns(text));
        true
    }

    /// Picks `title` in the overlay's pop-up VoiceOver calls `label`, as the user does; whether there is one.
    pub fn choose_in_dialog(&self, label: &str, title: &str) -> bool {
        use objc2_app_kit::NSAccessibility;
        let Some(open) = self.dialog.borrow().clone() else {
            return false;
        };
        let found = find_in::<objc2_app_kit::NSPopUpButton>(&open.frame.shade, &|popup| {
            popup.accessibilityLabel().is_some_and(|l| l.to_string() == label)
        });
        let Some(popup) = found else { return false };
        popup.selectItemWithTitle(&crate::common::util::ns(title));
        popup.titleOfSelectedItem().is_some_and(|t| t.to_string() == title)
    }

    /// Sends the Find item's action as ⌘F does, then searches the overlay for `text`; whether ⌘F reached the
    /// overlay and showed its search.
    pub fn search_dialog(&self, text: &str) -> bool {
        // The menu item sends its action along the key window's responder chain; the self-test's window need not be
        // key, so the chain starts at the focus of the tab's window.
        let responder = self.root.window().and_then(|w| w.firstResponder());
        // SAFETY: The selector names an action method taking a sender, sent without one as the menu may.
        let sent =
            responder.is_some_and(|r| unsafe { r.tryToPerform_with(objc2::sel!(performTextFinderAction:), None) });
        let Some(open) = self.dialog.borrow().clone() else {
            return false;
        };
        let found = find_in::<objc2_app_kit::NSSearchField>(&open.frame.shade, &|field| !field.isHidden());
        let Some(field) = found else { return false };
        field.setStringValue(&crate::common::util::ns(text));
        open.frame.filter();
        sent
    }
}
