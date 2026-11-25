//! The input field (E7, F209): a text view at the bottom of a session's pane where the command line is edited like
//! text on a Mac and sent as one piece; ↑ and ↓ step through the field's history, ⌃R searches it, ⌃C clears the
//! field and Escape leaves it for raw typing until the next prompt. It shows only while the shell waits for input.

mod chips;
mod glue;
mod look;
pub(crate) mod model;
mod suggest;
pub(crate) mod toggle;
pub(crate) mod vim;
mod vim_glue;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSEvent, NSEventModifierFlags, NSFont, NSLayoutConstraint, NSResponder, NSScrollView,
    NSTextView, NSView,
};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize};

use crate::kit::{guarded, ns};

/// The most lines the field grows to before it scrolls.
const MOST_LINES: f64 = 8.0;
/// The room around the text inside its view.
const TEXT_INSET: NSSize = NSSize::new(0.0, 3.0);
/// The room a text container keeps at each end of a line, AppKit's default.
const LINE_PADDING: f64 = 5.0;

/// What the field's text view keeps: the session view it belongs to, by id.
pub(crate) struct Ivars {
    view: u64,
}

define_class!(
    // SAFETY: NSTextView may be subclassed; `FieldText` adds plain ivars and does not implement Drop.
    #[unsafe(super(NSTextView, objc2_app_kit::NSText, NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermInputFieldText"]
    #[ivars = Ivars]
    pub(crate) struct FieldText;

    impl FieldText {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            if !guarded(false, || self.handle_key(event)) {
                // SAFETY: The overridden method with its own argument, as AppKit calls it.
                let () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // SAFETY: The overridden method, as AppKit calls it.
            let accepted: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if accepted {
                guarded((), || crate::host().changed(self.ivars().view, crate::Change::Focused));
            }
            accepted
        }

        #[unsafe(method(copy:))]
        fn copy_action(&self, sender: Option<&AnyObject>) {
            if guarded(false, || self.copy_terminal_selection()) {
                return;
            }
            // SAFETY: The overridden method with its own argument, as AppKit calls it.
            let () = unsafe { msg_send![super(self), copy: sender] };
        }

        #[unsafe(method(didChangeText))]
        fn did_change_text(&self) {
            // SAFETY: The overridden method, as AppKit calls it.
            let () = unsafe { msg_send![super(self), didChangeText] };
            guarded((), || {
                if let Some(view) = crate::find(self.ivars().view) {
                    view.field_text_changed();
                }
            });
        }
    }
);

impl FieldText {
    /// ⌘C with nothing selected in the field copies what is selected in the terminal above it; whether it did.
    fn copy_terminal_selection(&self) -> bool {
        if self.selectedRange().length > 0 {
            return false;
        }
        let Some(view) = crate::find(self.ivars().view).filter(|view| view.selected_text().is_some()) else {
            return false;
        };
        view.copy_selection();
        true
    }

    /// Handles the keys the field gives a meaning of its own; whether it did.
    fn handle_key(&self, event: &NSEvent) -> bool {
        let Some(view) = crate::find(self.ivars().view) else {
            return false;
        };
        if let Some(handled) = self.vim_key(&view, event) {
            return handled;
        }
        let flags = event.modifierFlags();
        let control = flags.contains(NSEventModifierFlags::Control);
        let shift = flags.contains(NSEventModifierFlags::Shift);
        let key = event
            .charactersIgnoringModifiers()
            .map(|s| s.to_string())
            .unwrap_or_default();
        match (event.keyCode(), key.as_str()) {
            (36 | 76, _) if !shift => {
                view.field_submit();
                true
            }
            (126, _) if self.on_first_line() => {
                view.field_history(true);
                true
            }
            (125, _) if self.on_last_line() => {
                view.field_history(false);
                true
            }
            (53, _) => {
                view.field_leave();
                true
            }
            (124, _) if self.caret() >= self.string().length() => view.field_accept(),
            (48, _) => {
                view.field_complete();
                true
            }
            (_, "c") if control => {
                self.set_text("");
                true
            }
            (_, "r") if control => {
                view.field_search();
                true
            }
            _ => false,
        }
    }

    fn text(&self) -> String {
        self.string().to_string()
    }

    fn set_text(&self, text: &str) {
        self.setString(&ns(text));
        self.setSelectedRange(NSRange::new(self.string().length(), 0));
        self.didChangeText();
    }

    fn caret(&self) -> usize {
        self.selectedRange().location
    }

    fn on_first_line(&self) -> bool {
        let text = self.text();
        let caret = self.caret().min(text.encode_utf16().count());
        let before: String = String::from_utf16_lossy(&text.encode_utf16().take(caret).collect::<Vec<_>>());
        !before.contains('\n')
    }

    fn on_last_line(&self) -> bool {
        let text = self.text();
        let caret = self.caret().min(text.encode_utf16().count());
        let after: String = String::from_utf16_lossy(&text.encode_utf16().skip(caret).collect::<Vec<_>>());
        !after.contains('\n')
    }
}

/// The field of one session view.
pub(crate) struct InputField {
    root: Retained<look::Backdrop>,
    text: Retained<FieldText>,
    height: Retained<NSLayoutConstraint>,
    /// What the history suggests, in grey at the right; → takes it.
    hint: Retained<objc2_app_kit::NSTextField>,
    /// What the empty field asks for.
    placeholder: Retained<objc2_app_kit::NSTextField>,
    /// The folder and the branch above the text.
    chips: chips::Chips,
    /// The switch in the window's toolbar that turns the field on and off.
    toggle: std::cell::OnceCell<toggle::FieldToggle>,
}

impl InputField {
    /// A hidden field for the view `view`.
    pub(crate) fn new(mtm: MainThreadMarker, view: u64) -> InputField {
        let this = FieldText::alloc(mtm).set_ivars(Ivars { view });
        // SAFETY: TextKit 2's initializer of a text view, called once on the newly allocated object.
        let text: Retained<FieldText> = unsafe { msg_send![super(this), initUsingTextLayoutManager: true] };
        text.setRichText(false);
        text.setAllowsUndo(true);
        text.setAutomaticQuoteSubstitutionEnabled(false);
        text.setAutomaticDashSubstitutionEnabled(false);
        text.setAutomaticTextReplacementEnabled(false);
        text.setAutomaticSpellingCorrectionEnabled(false);
        text.setContinuousSpellCheckingEnabled(false);
        text.setSmartInsertDeleteEnabled(false);
        text.setTextContainerInset(TEXT_INSET);
        text.setDrawsBackground(false);
        text.setVerticallyResizable(true);
        text.setHorizontallyResizable(false);
        text.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
        text.setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(400.0, 28.0)));
        text.setAccessibilityLabel(Some(&ns(splat_i18n::t("Befehlszeile"))));
        let scroll = NSScrollView::new(mtm);
        scroll.setDocumentView(Some(&text));
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setBorderType(objc2_app_kit::NSBorderType::NoBorder);
        scroll.setDrawsBackground(false);
        let hint = crate::kit::label(mtm, "");
        hint.setTextColor(Some(&NSColor::tertiaryLabelColor()));
        hint.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingHead);
        hint.setAlignment(objc2_app_kit::NSTextAlignment::Right);
        crate::kit::hide_from_voiceover(&hint);
        let placeholder = crate::kit::label(mtm, splat_i18n::t("Befehl eingeben"));
        placeholder.setTextColor(Some(&NSColor::placeholderTextColor()));
        crate::kit::hide_from_voiceover(&placeholder);
        text.setAccessibilityPlaceholderValue(Some(&ns(splat_i18n::t("Befehl eingeben"))));
        let root = look::Backdrop::new(mtm);
        let chips = chips::Chips::new(mtm);
        lay_out(&root, chips.view(), &scroll, &placeholder, &hint);
        let height = root.heightAnchor().constraintEqualToConstant(30.0);
        height.setActive(true);
        root.setHidden(true);
        InputField {
            root,
            text,
            height,
            hint,
            placeholder,
            chips,
            toggle: std::cell::OnceCell::new(),
        }
    }

    pub(crate) fn view(&self) -> &NSView {
        &self.root
    }

    /// What the grey hint says now.
    pub(crate) fn hint(&self) -> String {
        self.hint.stringValue().to_string()
    }

    /// Shows `hint` in grey at the right, or nothing.
    pub(crate) fn set_hint(&self, hint: Option<&str>) {
        self.hint.setStringValue(&ns(hint.unwrap_or_default()));
    }

    /// Takes up the terminal's face and colours.
    pub(crate) fn style(&self, font: &NSFont, foreground: &NSColor, background: &NSColor) {
        self.text.setFont(Some(font));
        self.text.setTextColor(Some(foreground));
        self.text.setInsertionPointColor(Some(foreground));
        self.root.set_ground(background);
        self.hint.setFont(Some(font));
        self.placeholder.setFont(Some(font));
        self.fit();
    }

    /// Shows or hides the field; a shown field takes the keyboard when `focus`.
    pub(crate) fn set_visible(&self, visible: bool, focus: bool) {
        self.root.setHidden(!visible);
        if visible
            && focus
            && let Some(window) = self.root.window()
        {
            window.makeFirstResponder(Some(&self.text));
        }
    }

    pub(crate) fn is_visible(&self) -> bool {
        !self.root.isHidden()
    }

    /// Whether the field has the keyboard.
    pub(crate) fn has_focus(&self) -> bool {
        self.root
            .window()
            .and_then(|window| window.firstResponder())
            .is_some_and(|responder| {
                let responder: &AnyObject = &responder;
                let own: &AnyObject = &self.text;
                std::ptr::eq(responder, own)
            })
    }

    pub(crate) fn text(&self) -> String {
        self.text.text()
    }

    pub(crate) fn set_text(&self, text: &str) {
        self.text.set_text(text);
    }

    /// Grows the field with its lines, up to `MOST_LINES`.
    pub(crate) fn fit(&self) {
        let font = self.text.font();
        let line = font.map_or(16.0, |font| font.ascender() - font.descender() + font.leading());
        let text = self.text();
        self.placeholder.setHidden(!text.is_empty());
        let lines = (text.matches('\n').count() + 1) as f64;
        let around = look::TOP + chips::HEIGHT + look::GAP + 2.0 * TEXT_INSET.height + look::BOTTOM;
        self.height.setConstant(lines.min(MOST_LINES) * line.ceil() + around);
    }

    /// Shows the folder and the branch above the text.
    pub(crate) fn show_context(&self, folder: &str, branch: Option<&str>) {
        self.chips.show(folder, branch);
    }

    /// What the chips above the text say, folder first.
    pub(crate) fn context(&self) -> Vec<String> {
        self.chips.texts()
    }
}

/// Puts the chips along the top of `root`, below its separator, and the text with its placeholder and hint below
/// them; the text's glyphs start where the chips do.
fn lay_out(root: &NSView, chips: &NSView, scroll: &NSView, placeholder: &NSView, hint: &NSView) {
    for view in [chips, scroll, placeholder, hint] {
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        root.addSubview(view);
    }
    let text_top = look::TOP + chips::HEIGHT + look::GAP;
    let first_line = text_top + TEXT_INSET.height;
    let glyphs = look::MARGIN_X;
    crate::kit::activate([
        chips
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&root.leadingAnchor(), look::MARGIN_X),
        chips
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&root.trailingAnchor(), -look::MARGIN_X),
        chips
            .topAnchor()
            .constraintEqualToAnchor_constant(&root.topAnchor(), look::TOP),
        scroll
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&root.leadingAnchor(), glyphs - LINE_PADDING),
        scroll
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&root.trailingAnchor(), -(glyphs - LINE_PADDING)),
        scroll
            .topAnchor()
            .constraintEqualToAnchor_constant(&root.topAnchor(), text_top),
        scroll
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&root.bottomAnchor(), -look::BOTTOM),
        hint.trailingAnchor()
            .constraintEqualToAnchor_constant(&root.trailingAnchor(), -glyphs),
        hint.topAnchor()
            .constraintEqualToAnchor_constant(&root.topAnchor(), first_line),
        hint.widthAnchor()
            .constraintLessThanOrEqualToAnchor_multiplier(&root.widthAnchor(), 0.6),
        placeholder
            .leadingAnchor()
            .constraintEqualToAnchor_constant(&root.leadingAnchor(), glyphs),
        placeholder
            .topAnchor()
            .constraintEqualToAnchor_constant(&root.topAnchor(), first_line),
    ]);
}
