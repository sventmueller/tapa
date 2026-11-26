//! The terminal view: a layer-backed view that draws a session's screen and hands it keys, mouse, pastes and drops.

use crate::kit::WindowId;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearanceCustomization, NSAppearanceNameAccessibilityHighContrastAqua,
    NSAppearanceNameAccessibilityHighContrastDarkAqua, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSBitmapImageRep, NSColor, NSDragOperation, NSDraggingInfo, NSEvent, NSLayoutConstraintOrientation, NSMenu,
    NSMenuItem, NSMenuItemValidation, NSPasteboard, NSPasteboardTypeFileURL, NSPasteboardTypeString, NSStackView,
    NSStackViewDistribution, NSTextInputClient, NSTrackingArea, NSTrackingAreaOptions, NSUserInterfaceLayoutDirection,
    NSUserInterfaceLayoutOrientation, NSView, NSViewLayerContentsRedrawPolicy,
};
use objc2_foundation::{
    NSArray, NSAttributedString, NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSTimer,
    NSUInteger,
};
use objc2_quartz_core::CADisplayLink;
use term_session::{Program, Session, Size, Snapshot};

use super::banner::Banner;
use super::input::KeyState;
use super::mouse::MouseState;
use crate::kit::{
    FlagGuard, Observation, guarded, item, live, main_after, ns, observe_window_visibility, pin_sides, separator,
    set_target,
};
use crate::model::{self, Command, State};
use term_render::constant;
use term_render::palette::{self, Theme};
use term_render::render::{self, Fonts};
use term_render::shape::Typography;
use term_render::thread::{Face, Renderer};

/// What a terminal view keeps.
pub struct Ivars {
    pub(super) id: u64,
    /// The id of the window the view's tab is in.
    window: WindowId,
    font_size: Cell<f64>,
    /// The session's redraw event is re-armed a display frame after the last one, not at once.
    pub(super) rearm_pending: Cell<bool>,
    /// What runs instead of the login shell, as a profile or `open -a` with a command asks.
    pub(super) program: RefCell<Option<Program>>,
    pub(super) session: RefCell<Option<Session>>,
    fonts: RefCell<Option<Rc<Fonts>>>,
    pub(super) theme: RefCell<Theme>,
    pub(super) screen: RefCell<Option<Arc<Snapshot>>>,
    /// The working directory the shell last reported, else the one it started in.
    pub(super) cwd: RefCell<PathBuf>,
    pub(super) title: RefCell<Option<String>>,
    pub(super) state: State,
    pub(super) keys: KeyState,
    pub(super) mouse: MouseState,
    banners: RefCell<Option<Banners>>,
    pub(super) metal: crate::metal::Metal,
    pub(super) blink: RefCell<Option<Retained<NSTimer>>>,
    /// Takes the snapshots of output that came within a frame of the last, paused while none waits.
    pub(super) frames: RefCell<Option<Retained<CADisplayLink>>>,
    observers: RefCell<Vec<Observation>>,
    /// The grid's size in the middle of the view while the window is resized (F218), and which showing it is.
    pub(super) size_label: RefCell<Option<Retained<objc2_app_kit::NSTextField>>>,
    pub(super) size_shown: Cell<u64>,
    /// The input field at the bottom (E7) and its state.
    pub(super) field: RefCell<Option<crate::field::InputField>>,
    pub(super) field_state: RefCell<crate::field::model::Field>,
    /// What the history suggests for the field's text now.
    pub(super) suggestion: RefCell<Option<String>>,
    /// The suggestion is the one line meant, from the history, the next command or the only candidate, so Tab takes
    /// it; one candidate of several is left to the menu.
    pub(super) suggestion_sure: Cell<bool>,
    /// The field's completion menu, while it is open.
    pub(super) menu: RefCell<Option<crate::field::menu::Menu>>,
    /// The suggestion drawn from the cursor on without the field.
    pub(super) ghost: RefCell<Option<crate::field::raw::Ghost>>,
    /// The colours of the command typed at the prompt without the field.
    pub(super) tint: RefCell<Option<term_render::tint::Tint>>,
    /// The line before the word whose menu the user closed, which does not open by itself again for that word.
    pub(super) menu_closed: RefCell<Option<String>>,
    /// What the shell integration reported about the shell.
    pub(super) shell: RefCell<crate::field::shell::Shell>,
    /// Copy mode (F111), while it is on.
    pub(super) copy: RefCell<Option<crate::copy::Copy>>,
}

impl Drop for Ivars {
    fn drop(&mut self) {
        if let Some(timer) = self.blink.get_mut().take() {
            timer.invalidate();
        }
        // The run loop keeps a link, and with it its target, until it is invalidated.
        if let Some(link) = self.frames.get_mut().take() {
            link.invalidate();
        }
        self.observers.get_mut().clear();
        self.mouse.stop_autoscroll();
        super::forget(self.id);
    }
}

pub(super) struct Banners {
    pub(super) notice: Banner,
    pub(super) find: crate::find::FindBar,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `TerminalView` does not implement Drop; its ivars do, and
    // objc2 drops them when the view is deallocated.
    /// A session's view, which draws its screen and hands the session keys, mouse, pastes and drops.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermSessionView"]
    #[ivars = Ivars]
    pub struct TerminalView;

    unsafe impl NSObjectProtocol for TerminalView {}

    unsafe impl NSTextInputClient for TerminalView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _range: NSRange) {
            guarded((), || self.insert(string));
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, selector: Sel) {
            guarded((), || self.command(selector));
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, _range: NSRange) {
            guarded((), || self.set_marked(string, selected));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            guarded((), || self.unmark());
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            guarded(NSRange::new(super::input::NOT_FOUND, 0), || self.marked_selection())
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            guarded(NSRange::new(super::input::NOT_FOUND, 0), || self.marked_range_now())
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            guarded(false, || self.has_marked())
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(&self, _range: NSRange, _actual: NSRangePointer) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSString>> {
            NSArray::new()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, _range: NSRange, _actual: NSRangePointer) -> NSRect {
            guarded(NSRect::ZERO, || self.cursor_on_screen())
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _point: NSPoint) -> NSUInteger {
            super::input::NOT_FOUND
        }
    }

    unsafe impl NSMenuItemValidation for TerminalView {
        #[unsafe(method(validateMenuItem:))]
        fn validate_menu_item(&self, item: &NSMenuItem) -> bool {
            guarded(false, || self.validate(item))
        }
    }

    impl TerminalView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, dirty: NSRect) {
            guarded((), || self.draw(dirty));
        }

        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            self.ivars().metal.supported && !self.ivars().state.picturing.get()
        }

        #[unsafe(method(cacheDisplayInRect:toBitmapImageRep:))]
        fn cache_display(&self, rect: NSRect, bitmap: &NSBitmapImageRep) {
            guarded((), || self.catch_up());
            let _picturing = FlagGuard::raise(&self.ivars().state.picturing);
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let () = unsafe { msg_send![super(self), cacheDisplayInRect: rect, toBitmapImageRep: bitmap] };
        }

        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            guarded((), || self.redraw_layer());
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let () = unsafe { msg_send![super(self), setFrameSize: size] };
            guarded((), || {
                self.layout_grid();
                self.redraw_layer();
            });
        }

        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            guarded((), || self.moved_to_window());
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn did_change_backing(&self) {
            guarded((), || {
                self.layout_grid();
                self.redraw_layer();
            });
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn did_change_appearance(&self) {
            guarded((), || self.update_theme());
        }

        #[unsafe(method(viewDidUnhide))]
        fn did_unhide(&self) {
            guarded((), || self.unhidden());
        }

        #[unsafe(method(viewDidHide))]
        fn did_hide(&self) {
            guarded((), || self.update_surface_visibility());
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        // Under a full-size content titlebar the window server would otherwise take a drag here to move the window.
        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(becomeFirstResponder))]
        fn become_first_responder(&self) -> bool {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let accepted: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if accepted {
                guarded((), || {
                    self.ivars().state.first_responder.set(true);
                    self.focus_changed();
                    super::host().changed(self.id(), crate::Change::Focused);
                    self.pass_keys_to_field();
                });
            }
            accepted
        }

        #[unsafe(method(resignFirstResponder))]
        fn resign_first_responder(&self) -> bool {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if resigned {
                guarded((), || {
                    self.ivars().state.first_responder.set(false);
                    self.discard_marked();
                    self.focus_changed();
                });
            }
            resigned
        }

        #[unsafe(method(keyDown:))]
        fn key_down_event(&self, event: &NSEvent) {
            if !guarded(true, || self.key_down(event)) {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(keyUp:))]
        fn key_up_event(&self, event: &NSEvent) {
            guarded((), || self.key_up(event));
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed_event(&self, event: &NSEvent) {
            guarded((), || self.flags_changed(event));
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down_event(&self, event: &NSEvent) {
            guarded((), || self.mouse_down(event));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged_event(&self, event: &NSEvent) {
            guarded((), || self.mouse_dragged(event));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up_event(&self, event: &NSEvent) {
            guarded((), || self.mouse_up(event));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down_event(&self, event: &NSEvent) {
            if !guarded(false, || self.other_button(event, true)) {
                // SAFETY: The superclass's implementation, which shows the context menu.
                let () = unsafe { msg_send![super(self), rightMouseDown: event] };
            }
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up_event(&self, event: &NSEvent) {
            guarded((), || {
                self.other_button(event, false);
            });
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged_event(&self, event: &NSEvent) {
            guarded((), || self.mouse_moved(event));
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down_event(&self, event: &NSEvent) {
            guarded((), || {
                self.other_button(event, true);
            });
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up_event(&self, event: &NSEvent) {
            guarded((), || {
                self.other_button(event, false);
            });
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged_event(&self, event: &NSEvent) {
            guarded((), || self.mouse_moved(event));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved_event(&self, event: &NSEvent) {
            guarded((), || self.mouse_moved(event));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel_event(&self, event: &NSEvent) {
            guarded((), || self.scroll_wheel(event));
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, event: &NSEvent) {
            guarded((), || self.update_pointer(event.modifierFlags(), Some(event.locationInWindow())));
        }

        #[unsafe(method_id(menuForEvent:))]
        fn menu_for_event(&self, _event: &NSEvent) -> Option<Retained<NSMenu>> {
            guarded(None, || Some(self.context_menu()))
        }

        #[unsafe(method(copy:))]
        fn copy_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.copy_selection());
        }

        #[unsafe(method(paste:))]
        fn paste_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                self.paste_from(&NSPasteboard::generalPasteboard());
            });
        }

        #[unsafe(method(selectAll:))]
        fn select_all_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.select_all());
        }

        #[unsafe(method(termClear:))]
        fn clear_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.clear());
        }

        #[unsafe(method(termNewHere:))]
        fn new_terminal_here_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                super::host().opened(self.window_id(), super::Opened::TerminalHere(self.cwd()));
            });
        }

        #[unsafe(method(termClosePane:))]
        fn close_pane_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                super::host().opened(self.window_id(), super::Opened::ClosePane(self.id()));
            });
        }

        #[unsafe(method(termDismissNotice:))]
        fn dismiss_notice_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.with_banners(|banners| banners.notice.hide()));
        }

        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            guarded(NSDragOperation::None, || {
                if super::clip::accepts(&sender.draggingPasteboard()) {
                    NSDragOperation::Copy
                } else {
                    NSDragOperation::None
                }
            })
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            guarded(false, || self.drop_from(&sender.draggingPasteboard()))
        }

        #[unsafe(method(isAccessibilityElement))]
        fn is_accessibility_element(&self) -> bool {
            true
        }

        #[unsafe(method_id(accessibilityRole))]
        fn accessibility_role(&self) -> Option<Retained<NSString>> {
            Some(constant!(objc2_app_kit::NSAccessibilityTextAreaRole).retain())
        }

        #[unsafe(method_id(accessibilityLabel))]
        fn accessibility_label(&self) -> Option<Retained<NSString>> {
            guarded(None, || Some(ns(&self.label())))
        }

        #[unsafe(method_id(accessibilityValue))]
        fn accessibility_value(&self) -> Option<Retained<AnyObject>> {
            guarded(None, || Some(ns(self.screen_text().text()).into()))
        }

        #[unsafe(method(accessibilityNumberOfCharacters))]
        fn accessibility_characters(&self) -> isize {
            guarded(0, || isize::try_from(self.screen_text().length()).unwrap_or(0))
        }

        #[unsafe(method_id(accessibilitySelectedText))]
        fn accessibility_selected_text(&self) -> Option<Retained<NSString>> {
            guarded(None, || Some(ns(&self.selected_text().unwrap_or_default())))
        }

        #[unsafe(method(accessibilitySelectedTextRange))]
        fn accessibility_selected_range(&self) -> NSRange {
            guarded(NSRange::new(0, 0), || NSRange::new(self.cursor_offset(), 0))
        }

        #[unsafe(method(accessibilityVisibleCharacterRange))]
        fn accessibility_visible_range(&self) -> NSRange {
            guarded(NSRange::new(0, 0), || NSRange::new(0, self.screen_text().length()))
        }

        #[unsafe(method(accessibilityInsertionPointLineNumber))]
        fn accessibility_insertion_line(&self) -> isize {
            guarded(0, || self.cursor_line())
        }

        #[unsafe(method(accessibilityLineForIndex:))]
        fn accessibility_line_for_index(&self, index: isize) -> isize {
            guarded(0, || {
                let line = self.screen_text().line_of(usize::try_from(index).unwrap_or(0));
                isize::try_from(line).unwrap_or(0)
            })
        }

        #[unsafe(method(accessibilityRangeForLine:))]
        fn accessibility_range_for_line(&self, line: isize) -> NSRange {
            guarded(NSRange::new(0, 0), || {
                let found = usize::try_from(line).ok().and_then(|line| self.screen_text().line_range(line));
                found.map_or(NSRange::new(0, 0), |(location, length)| NSRange::new(location, length))
            })
        }

        #[unsafe(method_id(accessibilityStringForRange:))]
        fn accessibility_string_for_range(&self, range: NSRange) -> Option<Retained<NSString>> {
            guarded(None, || {
                let text = self.screen_text().substring(range.location, range.length)?;
                Some(ns(&text))
            })
        }

        #[unsafe(method(accessibilityFrameForRange:))]
        fn accessibility_frame_for_range(&self, range: NSRange) -> NSRect {
            guarded(NSRect::ZERO, || self.frame_for_range(range))
        }
    }
);

impl TerminalView {
    /// A session view in window `window` that starts in `cwd`, and what its pane shows: the view under its notices.
    pub fn new(mtm: MainThreadMarker, window: WindowId, cwd: PathBuf) -> (Retained<TerminalView>, Retained<NSView>) {
        let config = super::host().config();
        let ivars = Ivars {
            id: super::next_id(),
            window,
            font_size: Cell::new(config.face().size),
            rearm_pending: Cell::new(false),
            program: RefCell::new(None),
            session: RefCell::new(None),
            fonts: RefCell::new(None),
            theme: RefCell::new(palette::theme(false, false)),
            screen: RefCell::new(None),
            cwd: RefCell::new(cwd),
            title: RefCell::new(None),
            state: State::default(),
            keys: KeyState::default(),
            mouse: MouseState::default(),
            banners: RefCell::new(None),
            metal: crate::metal::Metal::new(),
            blink: RefCell::new(None),
            frames: RefCell::new(None),
            observers: RefCell::new(vec![]),
            size_label: RefCell::new(None),
            size_shown: Cell::new(0),
            field: RefCell::new(None),
            field_state: RefCell::new(crate::field::model::Field::new(config.input.field)),
            suggestion: RefCell::new(None),
            suggestion_sure: Cell::new(false),
            menu: RefCell::new(None),
            ghost: RefCell::new(None),
            tint: RefCell::new(None),
            menu_closed: RefCell::new(None),
            shell: RefCell::new(crate::field::shell::Shell::default()),
            copy: RefCell::new(None),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
        this.setWantsLayer(true);
        if this.ivars().metal.supported {
            this.setLayerContentsRedrawPolicy(NSViewLayerContentsRedrawPolicy::Never);
        }
        // Programs lay out their output for a left-to-right grid, whatever the interface language.
        this.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
        this.registerForDraggedTypes(&NSArray::from_slice(&[
            constant!(NSPasteboardTypeFileURL),
            constant!(NSPasteboardTypeString),
        ]));
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::CursorUpdate
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: A tracking area holds its owner weakly, and the view owns the area.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&this),
                None,
            )
        };
        this.addTrackingArea(&area);
        this.update_theme();
        super::register(&this);
        let root = this.assemble(mtm);
        (this, root)
    }

    /// The number the app's registry of terminal views keeps the view under.
    pub fn id(&self) -> u64 {
        self.ivars().id
    }

    /// The id of the window the view's tab is in.
    pub fn window_id(&self) -> WindowId {
        self.ivars().window
    }

    /// Whether the view runs a command of its own rather than the login shell, so that its end stays to be read.
    pub fn runs_command(&self) -> bool {
        self.ivars().program.borrow().is_some()
    }

    /// The times from a key to the frame that showed after it, in microseconds, since this was asked last; empty
    /// where the view draws with Core Graphics.
    pub fn take_latencies(&self) -> Vec<u32> {
        self.with_renderer(Renderer::take_latencies).unwrap_or_default()
    }

    /// Counts the next frame towards the latencies as if a key had just been pressed, for the bench.
    pub fn mark_key(&self) {
        self.with_renderer(Renderer::key_pressed);
    }

    /// Opens the find bar (F104), or puts the keyboard back into it.
    pub fn open_find(&self) {
        self.with_banners(|banners| banners.find.open());
    }

    /// Searches for what the find bar holds: `None` anew as its text or options changed, `Some(true)` the next match,
    /// `Some(false)` the previous one.
    pub(crate) fn find_step(&self, step: Option<bool>) {
        let mut query = None;
        self.with_banners(|banners| {
            if banners.find.is_open() {
                query = Some((banners.find.pattern(), banners.find.options()));
            }
        });
        let Some((pattern, options)) = query else { return };
        let forward = step.unwrap_or(false);
        let result = self
            .with_session(|session| session.find(&pattern, options, forward))
            .unwrap_or_else(|| Err(String::new()));
        self.with_banners(|banners| banners.find.show(&result));
        self.refresh(true);
    }

    /// Closes the find bar, ends the search and gives the keyboard back to the terminal.
    pub fn end_find(&self) {
        self.with_banners(|banners| banners.find.close());
        self.with_session(term_session::Session::end_find);
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        self.refresh(true);
    }

    /// Opens the find bar with `text` and shows its first match, as typing it would.
    pub fn search_for(&self, text: &str) {
        self.with_banners(|banners| {
            banners.find.open();
            banners.find.set_pattern(text);
        });
        self.find_step(None);
    }

    /// Steps to the next or previous match while the find bar is open, as ⌘G and ⇧⌘G do.
    pub fn find_again(&self, forward: bool) {
        self.find_step(Some(forward));
    }

    /// Scrolls to the prompt `step` prompts away (F80); whether the view moved.
    pub fn jump_to_prompt(&self, step: isize) -> bool {
        let moved = self
            .with_session(|session| session.jump_to_prompt(step))
            .unwrap_or(false);
        if moved {
            self.refresh(true);
        }
        moved
    }

    /// Runs `program` instead of the login shell; before `spawn`.
    pub fn set_program(&self, program: Program) {
        *self.ivars().program.borrow_mut() = Some(program);
    }

    /// The working directory the shell last reported, else the one it started in.
    pub fn cwd(&self) -> PathBuf {
        self.ivars().cwd.borrow().clone()
    }

    /// The banners above the view and the view in a column, the view taking the height they leave.
    fn assemble(&self, mtm: MainThreadMarker) -> Retained<NSView> {
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setDistribution(NSStackViewDistribution::Fill);
        stack.setSpacing(crate::kit::space::NONE);
        let target: &AnyObject = self;
        let notice = Banner::new(
            mtm,
            "exclamationmark.triangle",
            &NSColor::systemOrangeColor().colorWithAlphaComponent(0.16),
            target,
            None,
            sel!(termDismissNotice:),
        );
        let find = crate::find::FindBar::new(mtm, self.id());
        let field = crate::field::InputField::new(mtm, self.id());
        for view in [find.view(), notice.view(), self, field.view()] {
            stack.addArrangedSubview(view);
            pin_sides(view, &stack, 0.0);
        }
        *self.ivars().field.borrow_mut() = Some(field);
        self.attach_field_icon();
        self.setContentHuggingPriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
        self.setContentCompressionResistancePriority_forOrientation(1.0, NSLayoutConstraintOrientation::Vertical);
        *self.ivars().banners.borrow_mut() = Some(Banners { notice, find });
        stack.into_super()
    }

    pub(super) fn with_banners(&self, f: impl FnOnce(&Banners)) {
        if let Some(banners) = self.ivars().banners.borrow().as_ref() {
            f(banners);
        }
    }

    fn context_menu(&self) -> Retained<NSMenu> {
        let mtm = self.mtm();
        let menu = NSMenu::new(mtm);
        for entry in model::menu() {
            match entry {
                Some((title, command)) => {
                    let entry = item(mtm, title, Some(action(command)), "");
                    set_target(&entry, self);
                    menu.addItem(&entry);
                }
                None => menu.addItem(&separator(mtm)),
            }
        }
        let added = super::host().context_items(self.id());
        if !added.is_empty() {
            menu.addItem(&separator(mtm));
        }
        for added in added {
            let entry = item(mtm, &added.title, Some(added.action), "");
            entry.setTag(isize::try_from(self.id()).unwrap_or(-1));
            menu.addItem(&entry);
        }
        menu
    }

    fn validate(&self, item: &NSMenuItem) -> bool {
        let Some(command) = item.action().and_then(command) else {
            return true;
        };
        let started = self.ivars().session.borrow().is_some();
        let selection = command == Command::Copy && self.selected_text().is_some();
        model::enabled(command, started, selection, || {
            super::clip::accepts(&NSPasteboard::generalPasteboard())
        })
    }

    pub(super) fn fonts(&self) -> Rc<Fonts> {
        let scale = self.window().map_or(2.0, |window| window.backingScaleFactor());
        let mut fonts = self.ivars().fonts.borrow_mut();
        match fonts.as_ref() {
            Some(current) if (current.metrics.scale - scale).abs() < f64::EPSILON => current.clone(),
            _ => {
                let face = self.render_face();
                let made = Rc::new(Fonts::styled(&face.family, face.size, scale, &face.typography));
                self.with_renderer(|renderer| renderer.set_face(face));
                *fonts = Some(made.clone());
                made
            }
        }
    }

    pub(super) fn render_face(&self) -> Face {
        let font = super::host().config().face();
        Face {
            typography: Typography::of(&font),
            family: font.family,
            size: self.ivars().font_size.get(),
        }
    }

    /// Takes up the settings again after the file changed: face, size, colours.
    pub fn apply_config(&self) {
        self.ivars().fonts.borrow_mut().take();
        self.ivars().font_size.set(super::host().config().face().size);
        self.update_theme();
        self.layout_grid();
        self.refresh(true);
    }

    /// The size the text is drawn at, in points.
    pub fn font_size(&self) -> f64 {
        self.ivars().font_size.get()
    }

    /// Draws with fonts of `size` points from now on, which changes the cells and so the grid.
    pub fn set_font_size(&self, size: f64) {
        if (self.ivars().font_size.replace(size) - size).abs() < f64::EPSILON {
            return;
        }
        self.ivars().fonts.borrow_mut().take();
        self.layout_grid();
        self.refresh(true);
    }

    /// The cell metrics, for turning positions into cells.
    pub(super) fn metrics(&self) -> render::Metrics {
        self.fonts().metrics
    }

    /// The grid that fits the view; after the program ended, one line less, for the line that says so.
    pub(super) fn grid_size(&self) -> Option<Size> {
        let bounds = self.bounds().size;
        if bounds.width < 1.0 || bounds.height < 1.0 {
            return None;
        }
        let m = self.metrics();
        let (columns, lines) = m.fit(bounds.width, bounds.height);
        let lines = self.ivars().state.grid_lines(lines);
        Some(Size {
            columns,
            lines,
            cell_width: m.pixels(m.cell_width),
            cell_height: m.pixels(m.cell_height),
        })
    }

    /// Fits the grid to the view and tells the program of a new size; while a size is dragged, at most every 100 ms,
    /// as each fit reflows the scrollback, and the frames between draw the grid as it is.
    pub(super) fn layout_grid(&self) {
        let Some(size) = self.grid_size() else { return };
        let state = &self.ivars().state;
        if state.size.get() == Some(size) {
            state.size_waits.set(false);
            return;
        }
        // A window out of sight changes size as it leaves or joins a tab group, as on every switch of workspaces; its
        // program hears only of the size it shows at.
        if state.size.get().is_some()
            && self
                .window()
                .is_some_and(|window| !crate::kit::window_in_front(&window))
        {
            state.size_waits.set(true);
            return;
        }
        state.size_waits.set(false);
        match state.resize(std::time::Instant::now(), live::dragging(self)) {
            model::Frame::Now => {}
            model::Frame::After(wait) => {
                let id = self.id();
                main_after(wait, move || {
                    if let Some(view) = super::find(id) {
                        view.ivars().state.resizes.pending.set(false);
                        view.layout_grid();
                    }
                });
                return;
            }
            model::Frame::Pending | model::Frame::Stale => return,
        }
        let had = state.size.replace(Some(size));
        if !self
            .with_session(|session| session.request_resize(size))
            .unwrap_or(false)
        {
            self.with_session(|session| session.resize(size));
            self.refresh(true);
        }
        if had.is_some() && live::dragging(self) {
            self.show_grid_size(size);
        }
    }

    fn moved_to_window(&self) {
        self.ivars().observers.borrow_mut().clear();
        let Some(window) = self.window() else {
            if let Some(timer) = self.ivars().blink.borrow_mut().take() {
                timer.invalidate();
            }
            self.pause_frames();
            self.update_surface_visibility();
            return;
        };
        let view = Weak::from(self);
        *self.ivars().observers.borrow_mut() = observe_window_visibility(&window, move || {
            if let Some(view) = view.load() {
                view.focus_changed();
                view.occlusion_changed();
            }
        });
        self.update_theme();
        self.layout_grid();
        self.focus_changed();
        self.occlusion_changed();
    }

    fn unhidden(&self) {
        self.update_surface_visibility();
        if !self.ivars().metal.supported && self.ivars().state.stale.get() {
            self.refresh(true);
        }
        let Some(window) = self.window() else { return };
        let idle = window.firstResponder().is_none_or(|responder| {
            let window_itself: &AnyObject = &window;
            let responder: &AnyObject = &responder;
            std::ptr::eq(responder, window_itself)
                || responder
                    .downcast_ref::<NSView>()
                    .is_some_and(NSView::isHiddenOrHasHiddenAncestor)
        });
        if idle {
            window.makeFirstResponder(Some(self));
        }
    }

    fn update_theme(&self) {
        let names = NSArray::from_slice(&[
            constant!(NSAppearanceNameAqua),
            constant!(NSAppearanceNameDarkAqua),
            constant!(NSAppearanceNameAccessibilityHighContrastAqua),
            constant!(NSAppearanceNameAccessibilityHighContrastDarkAqua),
        ]);
        let best = self.effectiveAppearance().bestMatchFromAppearancesWithNames(&names);
        let is = |name: &NSString| best.as_deref() == Some(name);
        let dark =
            is(constant!(NSAppearanceNameDarkAqua)) || is(constant!(NSAppearanceNameAccessibilityHighContrastDarkAqua));
        let high_contrast = is(constant!(NSAppearanceNameAccessibilityHighContrastAqua))
            || is(constant!(NSAppearanceNameAccessibilityHighContrastDarkAqua));
        let config = super::host().config();
        let name = if dark { &config.theme.dark } else { &config.theme.light };
        let chosen = super::host().themes().get(name, dark, high_contrast);
        let theme = Theme::from_config(&chosen, config.theme.minimum_contrast);
        if *self.ivars().theme.borrow() == theme {
            return;
        }
        self.with_session(|session| session.set_palette(theme.palette.clone()));
        self.with_renderer(|renderer| {
            renderer.set_theme(theme.clone());
        });
        *self.ivars().theme.borrow_mut() = theme;
        self.style_field();
        self.refresh(true);
    }

    pub(super) fn focus_changed(&self) {
        let state = &self.ivars().state;
        if state.size_waits.get() {
            self.layout_grid();
        }
        let focused = state.first_responder.get() && self.window().is_some_and(|w| w.isKeyWindow());
        if state.focused.replace(focused) == focused {
            return;
        }
        state.cursor_off.set(false);
        self.with_session(|session| session.focus(focused));
        self.refresh(false);
        crate::secure::refresh();
    }
}

/// The action of the menu item for `command`.
fn action(command: Command) -> Sel {
    match command {
        Command::Copy => sel!(copy:),
        Command::Paste => sel!(paste:),
        Command::SelectAll => sel!(selectAll:),
        Command::Clear => sel!(termClear:),
        Command::NewTerminalHere => sel!(termNewHere:),
        Command::ClosePane => sel!(termClosePane:),
    }
}

/// The command a menu item's `action` stands for.
fn command(action: Sel) -> Option<Command> {
    [
        Command::Copy,
        Command::Paste,
        Command::SelectAll,
        Command::Clear,
        Command::NewTerminalHere,
        Command::ClosePane,
    ]
    .into_iter()
    .find(|&command| self::action(command) == action)
}
