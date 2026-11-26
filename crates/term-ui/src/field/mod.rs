//! The input field (E7, F209): a text view at the bottom of a session's pane where the command line is edited like
//! text on a Mac and sent as one piece; ↑ and ↓ step through the field's history, ⌃R searches it, ⌃C clears the
//! field and Escape leaves it for raw typing until the next prompt. It shows only while the shell waits for input.
//! As in Warp with Tab set to take suggestions, a suggestion shows faint after the text: → or ⌃F take it, Tab where
//! it is the one line meant rather than one candidate of several, ⌃→ its next word. ⌃Space, or Tab otherwise, opens
//! the completion menu, and the command's parts have colours of their own.

mod chips;
pub(crate) mod colour;
mod glue;
mod look;
pub(crate) mod menu;
pub(crate) mod model;
pub(crate) mod raw;
pub(crate) mod shell;
mod suggest;
mod tint;
pub(crate) mod toggle;
pub(crate) mod vim;
mod vim_glue;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSEvent, NSEventModifierFlags, NSFont, NSLayoutConstraint, NSResponder, NSScrollView,
    NSTextInputClient, NSTextView, NSView,
};
use objc2_foundation::{NSPoint, NSRange, NSRect, NSSize};
use std::cell::RefCell;

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
        let at_end = self.caret() >= self.string().length();
        if let Some(handled) = view.menu_key(event.keyCode(), shift, at_end) {
            return handled;
        }
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
            (124, _) if control => view.field_accept_word(),
            (124, _) if at_end => view.field_accept(),
            (_, "f") if control => view.field_accept(),
            (_, "e") if control && at_end => view.field_accept(),
            (48, _) if !shift && view.field_tab_accept() => true,
            (48, _) => {
                view.field_complete();
                true
            }
            (49, _) if control => {
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
    /// Vim's mode, in grey at the right.
    hint: Retained<objc2_app_kit::NSTextField>,
    /// The rest of the suggested line, faint right after the text; → takes it.
    ghost: Retained<objc2_app_kit::NSTextField>,
    /// The colours of the text, the suggestion and the command's parts.
    colours: RefCell<colour::Colours>,
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
        let ghost = crate::kit::label(mtm, "");
        ghost.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByClipping);
        ghost.setHidden(true);
        crate::kit::hide_from_voiceover(&ghost);
        text.addSubview(&ghost);
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
            ghost,
            colours: RefCell::new(colour::Colours::of(&term_render::palette::theme(false, false))),
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

    /// The rest of the suggested line that shows after the text.
    pub(crate) fn ghost(&self) -> String {
        if self.ghost.isHidden() {
            return String::new();
        }
        self.ghost.stringValue().to_string()
    }

    /// Shows `ghost` faint right after the text, or nothing.
    pub(crate) fn set_ghost(&self, ghost: Option<&str>) {
        self.ghost.setStringValue(&ns(ghost.unwrap_or_default()));
        self.place_ghost();
        self.placeholder
            .setHidden(!self.text().is_empty() || !self.ghost.isHidden());
    }

    /// Puts the suggestion where the text ends, as far as the field has room; it stays away when the text does not end
    /// on its last visible line.
    fn place_ghost(&self) {
        let wanted = !self.ghost.stringValue().is_empty();
        let end = self.text.string().length();
        let Some(caret) = wanted.then(|| self.rect_of_utf16(end, &self.text)).flatten() else {
            self.ghost.setHidden(true);
            return;
        };
        let room = self.text.bounds().size.width - caret.origin.x - LINE_PADDING;
        let size = self.ghost.intrinsicContentSize();
        if room < 1.0 {
            self.ghost.setHidden(true);
            return;
        }
        // A label draws its text a little in from its frame, as its cell pads it.
        let padding = 2.0;
        self.ghost.setFrame(NSRect::new(
            NSPoint::new(
                caret.origin.x - padding,
                caret.origin.y + ((caret.size.height - size.height) / 2.0).round(),
            ),
            NSSize::new(room + padding, size.height),
        ));
        self.ghost.setHidden(false);
    }

    /// The rectangle of the text from UTF-16 offset `at` on, as input methods ask for it, in `view`'s coordinates.
    fn rect_of_utf16(&self, at: usize, view: &NSView) -> Option<NSRect> {
        let window = self.text.window()?;
        // SAFETY: An empty range within the text; no actual range is asked for.
        let screen = unsafe {
            self.text
                .firstRectForCharacterRange_actualRange(NSRange::new(at, 0), std::ptr::null_mut())
        };
        if screen.size.height <= 0.0 {
            return None;
        }
        let in_window = window.convertRectFromScreen(screen);
        Some(view.convertRect_fromView(in_window, None))
    }

    /// Where the text from byte `byte` on shows, in `view`'s coordinates.
    pub(crate) fn rect_of(&self, byte: usize, view: &NSView) -> Option<NSRect> {
        let text = self.text();
        let at = text.get(..byte).map_or(0, |head| head.encode_utf16().count());
        self.rect_of_utf16(at, view)
    }

    /// The face of the text.
    pub(crate) fn font(&self) -> Retained<NSFont> {
        self.text
            .font()
            .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(13.0, 0.0))
    }

    /// Takes up the terminal's face and colours.
    pub(crate) fn style(&self, font: &NSFont, theme: &term_render::palette::Theme) {
        let colours = colour::Colours::of(theme);
        let background = colour::ns_colour(theme.palette.background);
        self.text.setFont(Some(font));
        self.text.setTextColor(Some(&colours.text));
        self.text.setInsertionPointColor(Some(&colours.text));
        self.root.set_ground(&background);
        self.hint.setFont(Some(font));
        self.ghost.setFont(Some(font));
        self.ghost.setTextColor(Some(&colours.ghost));
        self.placeholder.setFont(Some(font));
        *self.colours.borrow_mut() = colours;
        self.fit();
        self.place_ghost();
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
        self.placeholder.setHidden(!text.is_empty() || !self.ghost.isHidden());
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
