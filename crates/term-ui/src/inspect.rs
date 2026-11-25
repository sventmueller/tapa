//! What the self-test drives and reads in a terminal view: keys as AppKit delivers them, pastes from a private
//! pasteboard, and the screen as text.

use objc2::rc::Retained;
use objc2::{DefinedClass, msg_send};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSEventType, NSPasteboard, NSTextInputClient};
use objc2_foundation::{NSPoint, NSRange, NSRect};
use term_session::{Foreground, PasteAction, Point, Rgb};

use super::view::TerminalView;
use crate::kit::ns;

/// The key code of `c` on a US keyboard, and whether it needs Shift.
fn us_key(c: char) -> (u16, bool) {
    const LETTERS: [u16; 26] = [
        0x00, 0x0b, 0x08, 0x02, 0x0e, 0x03, 0x05, 0x04, 0x22, 0x26, 0x28, 0x25, 0x2e, 0x2d, 0x1f, 0x23, 0x0c, 0x0f,
        0x01, 0x11, 0x20, 0x09, 0x0d, 0x07, 0x10, 0x06,
    ];
    const DIGITS: [u16; 10] = [0x1d, 0x12, 0x13, 0x14, 0x15, 0x17, 0x16, 0x1a, 0x1c, 0x19];
    const OTHERS: [(char, char, u16); 11] = [
        ('-', '_', 0x1b),
        ('=', '+', 0x18),
        ('[', '{', 0x21),
        (']', '}', 0x1e),
        ('\\', '|', 0x2a),
        (';', ':', 0x29),
        ('\'', '"', 0x27),
        (',', '<', 0x2b),
        ('.', '>', 0x2f),
        ('/', '?', 0x2c),
        ('`', '~', 0x32),
    ];
    const SHIFTED_DIGITS: &str = ")!@#$%^&*(";
    let index = |base: char, from: char| usize::try_from(u32::from(base) - u32::from(from)).unwrap_or(0);
    match c {
        'a'..='z' => (LETTERS[index(c, 'a')], false),
        'A'..='Z' => (LETTERS[index(c, 'A')], true),
        '0'..='9' => (DIGITS[index(c, '0')], false),
        ' ' => (0x31, false),
        '\r' | '\n' => (0x24, false),
        _ => {
            if let Some(digit) = SHIFTED_DIGITS.chars().position(|d| d == c) {
                return (DIGITS[digit], true);
            }
            OTHERS
                .iter()
                .find_map(|&(plain, shifted, code)| {
                    (c == plain)
                        .then_some((code, false))
                        .or((c == shifted).then_some((code, true)))
                })
                .unwrap_or((0x00, false))
        }
    }
}

impl TerminalView {
    /// Presses the key `code` that types `characters` with `modifiers`, through `keyDown:` as AppKit would.
    pub fn press(&self, code: u16, characters: &str, ignoring: &str, modifiers: NSEventModifierFlags) {
        let window = self.window().map_or(0, |window| window.windowNumber());
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::ZERO,
            modifiers,
            0.0,
            window,
            None,
            &ns(characters),
            &ns(ignoring),
            false,
            code,
        );
        if let Some(event) = event {
            // SAFETY: `keyDown:` takes the event, which lives for the call.
            let () = unsafe { msg_send![self, keyDown: &*event] };
        }
    }

    /// Presses or lets go of the modifier key `code` so that `flags` are held, through `flagsChanged:`.
    pub fn modifiers(&self, code: u16, flags: NSEventModifierFlags) {
        let window = self.window().map_or(0, |window| window.windowNumber());
        let event = NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::FlagsChanged,
            NSPoint::ZERO,
            flags,
            0.0,
            window,
            None,
            &ns(""),
            &ns(""),
            false,
            code,
        );
        if let Some(event) = event {
            // SAFETY: `flagsChanged:` takes the event, which lives for the call.
            let () = unsafe { msg_send![self, flagsChanged: &*event] };
        }
    }

    /// Whether the view shows the pointing hand over a link.
    pub fn hand(&self) -> bool {
        self.pointing_hand()
    }

    /// Types `text` key by key, as on a US keyboard.
    pub fn type_text(&self, text: &str) {
        for c in text.chars() {
            let (code, shift) = us_key(c);
            let modifiers = if shift {
                NSEventModifierFlags::Shift
            } else {
                NSEventModifierFlags::empty()
            };
            let typed = c.to_string();
            self.press(code, &typed, &typed, modifiers);
        }
    }

    /// The lines of the screen as the view shows them.
    pub fn visible_lines(&self) -> Vec<String> {
        self.with_session(term_session::Session::visible_text)
            .unwrap_or_default()
    }

    /// The longest the view's snapshots waited for the screen's lock and held it since this was asked last.
    pub fn lock_times(&self) -> term_session::LockTimes {
        self.with_session(term_session::Session::take_lock_times)
            .unwrap_or_default()
    }

    /// How many prompts the shell has shown.
    pub fn prompts(&self) -> usize {
        self.ivars().state.prompts.get()
    }

    /// Pastes what `pasteboard` holds as ⌘V would, and what it did with an image.
    pub fn paste_image_from(&self, pasteboard: &NSPasteboard) -> Option<PasteAction> {
        self.paste_from(pasteboard)
    }

    /// The notice's text while it shows.
    pub fn notice_banner(&self) -> Option<String> {
        let mut shown = None;
        self.with_banners(|banners| shown = banners.notice.shown());
        shown
    }

    /// Has the view take the focus as in a key window, which a self-test app that never activates does not get.
    pub fn take_focus(&self) {
        self.ivars().state.focused.set(true);
        self.with_session(|session| session.focus(true));
    }

    /// Whether a bell rang in the terminal.
    pub fn has_rung(&self) -> bool {
        self.ivars().state.has_rung()
    }

    /// The lines of the screen as the view last took it for drawing.
    pub fn drawn_lines(&self) -> Vec<String> {
        self.ivars().screen.borrow().as_ref().map_or_else(Vec::new, |screen| {
            screen
                .rows
                .iter()
                .map(|row| {
                    row.iter()
                        .map(|cell| cell.ch)
                        .collect::<String>()
                        .trim_end()
                        .to_string()
                })
                .collect()
        })
    }

    /// The text selected in the terminal.
    pub fn selection(&self) -> Option<String> {
        self.selected_text()
    }

    /// Whether a drag of `pasteboard` may end on the view, then what dropping it there does: whether it took it.
    pub fn drop_pasteboard(&self, pasteboard: &NSPasteboard) -> (bool, bool) {
        (super::clip::accepts(pasteboard), self.drop_from(pasteboard))
    }

    /// Whether the program in the terminal ended.
    pub fn has_exited(&self) -> bool {
        self.ivars().state.exited.get()
    }

    /// The shell's process number.
    pub fn shell(&self) -> Option<u32> {
        self.with_session(term_session::Session::pid)
    }

    /// What runs in front in the terminal.
    pub fn foreground(&self) -> Foreground {
        self.with_session(term_session::Session::foreground)
            .unwrap_or(Foreground::Unknown)
    }

    /// Composes `text` as an input method does, with the caret after `selected` characters of it.
    pub fn compose(&self, text: &str, selected: usize) {
        let none = NSRange::new(super::input::NOT_FOUND, 0);
        // SAFETY: An NSString is one of the two types the method takes.
        unsafe { self.setMarkedText_selectedRange_replacementRange(&ns(text), NSRange::new(selected, 0), none) };
    }

    /// Commits `text` as an input method does at the end of a composition.
    pub fn commit(&self, text: &str) {
        let none = NSRange::new(super::input::NOT_FOUND, 0);
        // SAFETY: An NSString is one of the two types the method takes.
        unsafe { self.insertText_replacementRange(&ns(text), none) };
    }

    /// The range of the text being composed, as input methods ask for it.
    pub fn composing(&self) -> Option<(usize, usize)> {
        let range = self.markedRange();
        self.hasMarkedText().then_some((range.location, range.length))
    }

    /// Where an input method would put its candidate window, in screen coordinates.
    pub fn candidate_rect(&self) -> NSRect {
        // SAFETY: A null pointer asks for no actual range.
        unsafe { self.firstRectForCharacterRange_actualRange(NSRange::new(0, 0), std::ptr::null_mut()) }
    }

    /// How many pixels of the `cells` cells after the cursor differ from the first of them, as the view draws them.
    pub fn ink_after_cursor(&self, cells: usize) -> usize {
        let Some(cursor) = self.ivars().screen.borrow().as_ref().map(|screen| screen.cursor.point) else {
            return 0;
        };
        let after = Point {
            line: cursor.line,
            column: cursor.column + 1,
        };
        let area = self.metrics().cell_rect(after, cells);
        let Some(bitmap) = self.bitmapImageRepForCachingDisplayInRect(area) else {
            return 0;
        };
        self.cacheDisplayInRect_toBitmapImageRep(area, &bitmap);
        let first = bitmap.colorAtX_y(0, 0);
        let mut differing = 0;
        for y in 0..bitmap.pixelsHigh() {
            for x in 0..bitmap.pixelsWide() {
                if bitmap.colorAtX_y(x, y) != first {
                    differing += 1;
                }
            }
        }
        differing
    }

    /// Columns and lines of the grid.
    pub fn grid(&self) -> Option<(u16, u16)> {
        self.ivars().state.size.get().map(|size| (size.columns, size.lines))
    }

    /// How many lines the view is scrolled back into the history.
    pub fn scrolled_back(&self) -> usize {
        self.ivars()
            .screen
            .borrow()
            .as_ref()
            .map_or(0, |screen| screen.display_offset)
    }

    /// Sends the left button's `kind` of event at `location` in the view's coordinates with `flags`, as AppKit would.
    pub fn mouse(&self, kind: NSEventType, location: NSPoint, flags: NSEventModifierFlags) {
        let Some(window) = self.window() else { return };
        let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
            kind,
            self.convertPoint_toView(location, None),
            flags,
            0.0,
            window.windowNumber(),
            None,
            0,
            1,
            1.0,
        );
        let Some(event) = event else { return };
        // SAFETY: Each of these takes the event, which lives for the call.
        let () = unsafe {
            match kind {
                NSEventType::LeftMouseDown => msg_send![self, mouseDown: &*event],
                NSEventType::LeftMouseDragged => msg_send![self, mouseDragged: &*event],
                NSEventType::MouseMoved => msg_send![self, mouseMoved: &*event],
                _ => msg_send![self, mouseUp: &*event],
            }
        };
    }

    /// The middle of `point`'s cell in the view's coordinates.
    pub fn cell_center(&self, point: Point) -> NSPoint {
        let cell = self.metrics().cell_rect(point, 1);
        NSPoint::new(
            cell.origin.x + cell.size.width / 2.0,
            cell.origin.y + cell.size.height / 2.0,
        )
    }

    /// The background the screen is drawn on now.
    pub fn background(&self) -> Option<Rgb> {
        self.ivars().screen.borrow().as_ref().map(|screen| screen.background)
    }
}

/// The view of the terminal opened last, by its number, and the number.
pub fn latest() -> Option<(u64, Retained<TerminalView>)> {
    let id = super::latest_id()?;
    super::find(id).map(|view| (id, view))
}

/// The terminal view with the number `id`, if it is open.
pub fn view(id: u64) -> Option<Retained<TerminalView>> {
    super::find(id)
}
