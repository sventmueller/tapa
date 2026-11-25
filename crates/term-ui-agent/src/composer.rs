//! The composer: a floating card at the bottom with a growing text view, attachments, the `/` menu, the model,
//! mode and sandbox pills and the send button that stops a running turn.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSBezelStyle, NSBox, NSButton, NSCellImagePosition, NSColor, NSControlBorderShape,
    NSDragOperation, NSDraggingInfo, NSEvent, NSEventModifierFlags, NSLayoutConstraint, NSPasteboard, NSPasteboardType,
    NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSScrollView, NSStackView, NSTextField,
    NSTextInputClient, NSTextView, NSTintProminence, NSView,
};
use objc2_foundation::{NSArray, NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};

use super::style;
use super::target::AgentTarget;
use super::ui;
use crate::AgentKey;
use crate::common::constant;
use crate::common::util::{activate, guarded, ns, symbol};

/// The text view's height on one line, and at most before it scrolls.
const MIN_TEXT: f64 = 20.0;
const MAX_TEXT: f64 = 180.0;

pub(crate) struct TextIvars {
    key: AgentKey,
}

/// The key code of Tab.
const TAB_KEY: u16 = 48;

define_class!(
    // SAFETY: NSTextView has no subclassing requirements and `ComposerText` does not implement Drop.
    #[unsafe(super(NSTextView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentComposerText"]
    #[ivars = TextIvars]
    pub(crate) struct ComposerText;

    unsafe impl NSObjectProtocol for ComposerText {}

    impl ComposerText {
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled = guarded(false, || {
                let code = event.keyCode();
                let flags = event.modifierFlags();
                // Tab is text here; ⌃Tab and ⌃⇧Tab leave, as in any macOS text view, which binds only ⌃Tab.
                if code == TAB_KEY && flags.contains(NSEventModifierFlags::Control) {
                    if let Some(window) = self.window() {
                        if flags.contains(NSEventModifierFlags::Shift) {
                            window.selectPreviousKeyView(Some(self));
                        } else {
                            window.selectNextKeyView(Some(self));
                        }
                    }
                    return true;
                }
                let newline = flags.contains(NSEventModifierFlags::Shift) || flags.contains(NSEventModifierFlags::Option);
                if (code == 36 || code == 76) && !newline && !self.hasMarkedText() {
                    if let Some(tab) = super::get(self.ivars().key) {
                        tab.submit();
                    }
                    return true;
                }
                false
            });
            if !handled {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let accepted: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            guarded((), || {
                if accepted && let Some(tab) = super::get(self.ivars().key) {
                    tab.composer_focus(true);
                }
            });
            accepted
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            guarded((), || {
                if resigned && let Some(tab) = super::get(self.ivars().key) {
                    tab.composer_focus(false);
                }
            });
            resigned
        }

        #[unsafe(method(paste:))]
        fn paste(&self, sender: Option<&AnyObject>) {
            let pasteboard = NSPasteboard::generalPasteboard();
            let taken = guarded(false, || {
                super::get(self.ivars().key).is_some_and(|tab| tab.take_pasteboard(&pasteboard))
            });
            if !taken {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), paste: sender] };
            }
        }

        /// Images are readable too, or AppKit would disable Paste while the pasteboard holds only a picture.
        #[unsafe(method_id(readablePasteboardTypes))]
        fn readable_types(&self) -> Retained<NSArray<NSPasteboardType>> {
            NSArray::from_slice(&[
                constant!(NSPasteboardTypeString),
                constant!(NSPasteboardTypePNG),
                constant!(NSPasteboardTypeTIFF),
            ])
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, _sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let pasteboard = sender.draggingPasteboard();
            let taken = guarded(false, || {
                super::get(self.ivars().key).is_some_and(|tab| tab.take_pasteboard(&pasteboard))
            });
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            taken || unsafe { msg_send![super(self), performDragOperation: sender] }
        }
    }
);

pub(crate) struct Composer {
    pub(crate) card: Retained<NSBox>,
    pub(crate) text: Retained<ComposerText>,
    #[cfg_attr(
        not(feature = "selftest"),
        expect(dead_code, reason = "kept with the views it holds")
    )]
    scroll: Retained<NSScrollView>,
    height: Retained<NSLayoutConstraint>,
    placeholder: Retained<NSTextField>,
    pub(crate) chips: Retained<NSStackView>,
    pub(crate) model: Retained<NSButton>,
    pub(crate) mode: Retained<NSButton>,
    pub(crate) sandbox: Retained<NSButton>,
    pub(crate) send: Retained<NSButton>,
    /// Whether the send button shows a running turn and something to send, once it was set.
    send_look: std::cell::Cell<Option<(bool, bool)>>,
    /// The microphone, which dictates into the text view.
    #[cfg(feature = "dictation")]
    pub(crate) mic: term_ui_dictation::Mic,
}

/// The composer's card around `column`, which VoiceOver reads as the input.
fn card(mtm: MainThreadMarker, column: &NSView) -> Retained<NSBox> {
    let card = ui::framed(
        mtm,
        column,
        &NSColor::separatorColor(),
        Some(&NSColor::controlBackgroundColor()),
        10.0,
        ui::insets(10.0, 12.0, 8.0, 10.0),
    );
    ui::group(&card, t("Eingabe"));
    card
}

/// A borderless button with a title and a chevron, as the pills of the composer read.
fn pill(mtm: MainThreadMarker, target: &AgentTarget, action: objc2::runtime::Sel, label: &str) -> Retained<NSButton> {
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(""), None, None, mtm) };
    crate::common::util::set_action(&button, target, action);
    button.setBordered(false);
    button.setFont(Some(&style::chat_small()));
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    button.setImage(symbol("chevron.up.chevron.down", label).as_deref());
    button.setImagePosition(NSCellImagePosition::ImageTrailing);
    crate::common::util::set_label(&button, Some(label));
    ui::hug(&button);
    button
}

impl Composer {
    pub(crate) fn new(mtm: MainThreadMarker, key: AgentKey, target: &AgentTarget) -> Composer {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(400.0, MIN_TEXT));
        let text = text_view(mtm, key, target, frame);
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setDrawsBackground(false);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDocumentView(Some(&text));
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        let height = scroll.heightAnchor().constraintEqualToConstant(MIN_TEXT);
        height.setActive(true);
        let placeholder = ui::label(
            mtm,
            t("⌘Esc zum Fokussieren oder Verlassen"),
            &style::chat_body(),
            &NSColor::placeholderTextColor(),
        );
        let chips = ui::row(mtm, 4.0, &[]);
        chips.setHidden(true);
        let parts = Footer::new(mtm, target);
        let editor = NSView::new(mtm);
        editor.setTranslatesAutoresizingMaskIntoConstraints(false);
        // Under the text view, which draws no background, so a click on the placeholder reaches the text.
        editor.addSubview(&placeholder);
        editor.addSubview(&scroll);
        placeholder.setTranslatesAutoresizingMaskIntoConstraints(false);
        #[cfg(feature = "dictation")]
        let mic = term_ui_dictation::mic(mtm, 13.0);
        #[cfg(feature = "dictation")]
        let trailing = {
            editor.addSubview(&mic.view);
            mic.view.setTranslatesAutoresizingMaskIntoConstraints(false);
            activate([
                mic.view
                    .trailingAnchor()
                    .constraintLessThanOrEqualToAnchor(&editor.trailingAnchor()),
                mic.view.topAnchor().constraintEqualToAnchor(&editor.topAnchor()),
            ]);
            (mic.view.leadingAnchor(), -6.0)
        };
        #[cfg(not(feature = "dictation"))]
        let trailing = (editor.trailingAnchor(), 0.0);
        activate([
            scroll.leadingAnchor().constraintEqualToAnchor(&editor.leadingAnchor()),
            scroll.topAnchor().constraintEqualToAnchor(&editor.topAnchor()),
            scroll.bottomAnchor().constraintEqualToAnchor(&editor.bottomAnchor()),
            scroll
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&trailing.0, trailing.1),
            placeholder
                .leadingAnchor()
                .constraintEqualToAnchor(&scroll.leadingAnchor()),
            placeholder
                .topAnchor()
                .constraintEqualToAnchor_constant(&scroll.topAnchor(), 2.0),
            placeholder
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor(&scroll.trailingAnchor()),
        ]);
        let spacer = ui::spacer(mtm);
        let footer = ui::row(
            mtm,
            8.0,
            &[
                &parts.attach,
                &parts.slash,
                &parts.model,
                &spacer,
                &parts.mode,
                &parts.sandbox,
                &parts.send,
            ],
        );
        let card = card(mtm, &ui::column(mtm, 8.0, &[&chips, &editor, &footer]));
        #[cfg(feature = "dictation")]
        mic.button
            .centerXAnchor()
            .constraintEqualToAnchor(&parts.send.centerXAnchor())
            .setActive(true);
        Composer {
            card,
            text,
            scroll,
            height,
            placeholder,
            chips,
            model: parts.model,
            mode: parts.mode,
            sandbox: parts.sandbox,
            send: parts.send,
            send_look: std::cell::Cell::new(None),
            #[cfg(feature = "dictation")]
            mic,
        }
    }

    /// Grows or shrinks the text view to its text, up to a limit, and shows the placeholder when it is empty.
    pub(crate) fn fit(&self) {
        let empty = self.text.string().length() == 0;
        self.placeholder.setHidden(!empty);
        // SAFETY: The composer is a TextKit 1 view by choice; its layout manager is only read here.
        let layout = unsafe { self.text.layoutManager() };
        // SAFETY: A text view keeps its text container alive as long as it lives.
        let container = unsafe { self.text.textContainer() };
        let used = match (layout, container) {
            (Some(layout), Some(container)) => {
                layout.ensureLayoutForTextContainer(&container);
                layout.usedRectForTextContainer(&container).size.height
            }
            _ => MIN_TEXT,
        };
        let height = (used + 4.0).clamp(style::chat(MIN_TEXT), MAX_TEXT);
        if (self.height.constant() - height).abs() > 0.5 {
            self.height.setConstant(height);
        }
    }

    /// Takes the text size the user chose for the agent tabs.
    pub(crate) fn text_size_changed(&self) {
        self.text.setFont(Some(&style::chat_body()));
        self.placeholder.setFont(Some(&style::chat_body()));
        self.model.setFont(Some(&style::chat_small()));
        self.mode.setFont(Some(&style::chat_small()));
        self.fit();
    }

    /// Shows `text` while the composer is empty.
    /// Names the agent the message goes to, for VoiceOver.
    pub(crate) fn set_agent(&self, agent: &str) {
        self.text
            .setAccessibilityLabel(Some(&ns(&tf("Nachricht an {}", &[agent]))));
    }

    pub(crate) fn set_placeholder(&self, text: &str) {
        self.placeholder.setStringValue(&ns(text));
    }

    /// What the user typed.
    pub(crate) fn text(&self) -> String {
        self.text.string().to_string()
    }

    pub(crate) fn clear(&self) {
        self.text.setString(&ns(""));
        self.fit();
    }

    pub(crate) fn set_text(&self, text: &str) {
        self.text.setString(&ns(text));
        self.fit();
    }

    /// The border of the card, in the system's accent colour while the composer has the keyboard focus.
    pub(crate) fn show_focus(&self, focused: bool) {
        let color = if focused {
            NSColor::controlAccentColor()
        } else {
            NSColor::separatorColor()
        };
        self.card.setBorderColor(&color);
    }

    /// The send button as a stop button while a turn runs, dimmed when there is nothing to send.
    pub(crate) fn show_running(&self, running: bool, can_send: bool) {
        if self.send_look.replace(Some((running, can_send))) == Some((running, can_send)) {
            return;
        }
        let glass = has_glass(&self.send);
        let (send, stop) = send_symbols(glass);
        let (name, label) = if running && !can_send {
            (stop, t("Stoppen"))
        } else {
            (send, t("Senden"))
        };
        self.send.setImage(symbol(name, label).as_deref());
        self.send.setToolTip(Some(&ns(label)));
        crate::common::util::set_label(&self.send, Some(label));
        let active = running || can_send;
        if !glass {
            let tint = if active {
                NSColor::controlAccentColor()
            } else {
                NSColor::tertiaryLabelColor()
            };
            self.send.setContentTintColor(Some(&tint));
        }
        self.send.setEnabled(active);
    }

    /// Whether the composer has the keyboard focus.
    pub(crate) fn has_focus(&self) -> bool {
        let Some(window) = self.text.window() else {
            return false;
        };
        let ours: &AnyObject = &self.text;
        window.firstResponder().is_some_and(|responder| {
            let responder: &AnyObject = &responder;
            std::ptr::eq(responder, ours)
        })
    }
}

/// The growing text view the user types into.
fn text_view(mtm: MainThreadMarker, key: AgentKey, target: &AgentTarget, frame: NSRect) -> Retained<ComposerText> {
    let this = ComposerText::alloc(mtm).set_ivars(TextIvars { key });
    // SAFETY: NSTextView's initializer, called once on the newly allocated object.
    let text: Retained<ComposerText> = unsafe { msg_send![super(this), initWithFrame: frame] };
    text.setRichText(false);
    text.setFont(Some(&style::chat_body()));
    text.setTextColor(Some(&NSColor::labelColor()));
    text.setDrawsBackground(false);
    text.setAllowsUndo(true);
    text.setAutomaticQuoteSubstitutionEnabled(false);
    text.setAutomaticDashSubstitutionEnabled(false);
    text.setTextContainerInset(NSSize::new(0.0, 2.0));
    // SAFETY: A text view keeps its text container alive as long as it lives.
    if let Some(container) = unsafe { text.textContainer() } {
        container.setLineFragmentPadding(0.0);
    }
    text.setAccessibilityLabel(Some(&ns(t("Nachricht an Claude"))));
    text.setDelegate(Some(ProtocolObject::from_ref(target)));
    text.registerForDraggedTypes(&NSArray::from_slice(&[
        // SAFETY: AppKit sets its pasteboard type constants when it loads.
        unsafe { objc2_app_kit::NSPasteboardTypeFileURL },
        constant!(NSPasteboardTypePNG),
        constant!(NSPasteboardTypeTIFF),
        constant!(NSPasteboardTypeString),
    ]));
    text
}

/// The controls of the composer's footer.
struct Footer {
    attach: Retained<NSButton>,
    slash: Retained<NSButton>,
    model: Retained<NSButton>,
    mode: Retained<NSButton>,
    sandbox: Retained<NSButton>,
    send: Retained<NSButton>,
}

impl Footer {
    fn new(mtm: MainThreadMarker, target: &AgentTarget) -> Footer {
        let attach = ui::symbol_button(
            mtm,
            "plus",
            t("Dateien oder Bilder anhängen"),
            13.0,
            target,
            sel!(ideAgentAttach:),
            0,
        );
        let slash = ui::symbol_button(mtm, "slash.circle", t("Befehle"), 13.0, target, sel!(ideAgentSlash:), 0);
        let model = pill(mtm, target, sel!(ideAgentModelMenu:), t("Modell und Denktiefe"));
        let mode = pill(mtm, target, sel!(ideAgentModeMenu:), t("Freigabemodus"));
        let sandbox = ui::symbol_button(mtm, "shield", t("Sandbox"), 12.0, target, sel!(ideAgentSandbox:), 0);
        let send = send_button(mtm, target);
        Footer {
            attach,
            slash,
            model,
            mode,
            sandbox,
            send,
        }
    }
}

/// Whether AppKit draws `button` as Liquid Glass, which it can from macOS 26 on.
fn has_glass(button: &NSButton) -> bool {
    button.respondsToSelector(sel!(setTintProminence:))
}

/// The symbols of the send button for sending and stopping: bare on a glass button, which draws its own circle,
/// else filled circles.
fn send_symbols(glass: bool) -> (&'static str, &'static str) {
    if glass {
        ("arrow.up", "stop.fill")
    } else {
        ("arrow.up.circle.fill", "stop.circle.fill")
    }
}

/// The round send button: a glass button tinted in the accent colour, or a filled circle symbol before macOS 26.
fn send_button(mtm: MainThreadMarker, target: &AgentTarget) -> Retained<NSButton> {
    let send = ui::symbol_button(mtm, "arrow.up", t("Senden"), 12.0, target, sel!(ideAgentSend:), 0);
    let glass = has_glass(&send);
    send.setImage(symbol(send_symbols(glass).0, t("Senden")).as_deref());
    let (points, weight) = if glass {
        send.setBordered(true);
        send.setBezelStyle(NSBezelStyle::Glass);
        send.setBorderShape(NSControlBorderShape::Circle);
        send.setTintProminence(NSTintProminence::Primary);
        send.setContentTintColor(None);
        (12.0, constant!(objc2_app_kit::NSFontWeightBold))
    } else {
        send.setContentTintColor(Some(&NSColor::controlAccentColor()));
        (22.0, constant!(objc2_app_kit::NSFontWeightRegular))
    };
    send.setSymbolConfiguration(Some(
        &objc2_app_kit::NSImageSymbolConfiguration::configurationWithPointSize_weight(points, weight),
    ));
    ui::fixed_width(&send, 26.0);
    ui::fixed_height(&send, 26.0);
    send
}

/// The pasteboard's types that hold a picture.
pub(crate) fn image_types() -> [&'static NSPasteboardType; 2] {
    [constant!(NSPasteboardTypePNG), constant!(NSPasteboardTypeTIFF)]
}

#[cfg(feature = "selftest")]
mod selftest_probe;
