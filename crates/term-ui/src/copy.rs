//! Copy mode (F111): the keyboard moves a cursor of its own over the screen and the history with Vim's keys, selects
//! with `v`, `V` and ⌃V, searches with `/` and `?`, and `y` copies; the view scrolls along with the cursor. Keys go to
//! copy mode instead of the program while it is on.

mod model;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSColor, NSEvent, NSEventModifierFlags, NSTextField, NSView};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_session::{NamedKey, Point, Scroll, SelectionKind, Session};
use term_vim::{Mode, Note};

pub(crate) use model::{CopyKey, CopyMode, Step};

use super::view::TerminalView;
use crate::kit::ns;

/// Copy mode of one view: its state and what it shows over the terminal.
pub(crate) struct Copy {
    mode: CopyMode,
    cursor: Retained<NSView>,
    /// The status's capsule, in the view's bottom right corner.
    badge: Retained<NSView>,
    status: Retained<NSTextField>,
    /// The program wrote since copy mode last took the rows.
    stale: bool,
}

/// The copy-mode key of a key press; none for one with ⌘, which belongs to the menus.
fn key_of(event: &NSEvent) -> Option<CopyKey> {
    let flags = event.modifierFlags();
    if flags.contains(NSEventModifierFlags::Command) {
        return None;
    }
    if let Some(named) = crate::keys::named(event.keyCode()) {
        return Some(match named {
            NamedKey::Escape => CopyKey::Escape,
            NamedKey::Enter => CopyKey::Enter,
            NamedKey::Backspace => CopyKey::Backspace,
            NamedKey::Up => CopyKey::Up,
            NamedKey::Down => CopyKey::Down,
            NamedKey::Left => CopyKey::Left,
            NamedKey::Right => CopyKey::Right,
            NamedKey::Home => CopyKey::Home,
            NamedKey::End => CopyKey::End,
            NamedKey::PageUp => CopyKey::PageUp,
            NamedKey::PageDown => CopyKey::PageDown,
            _ => return None,
        });
    }
    if flags.contains(NSEventModifierFlags::Control) {
        let plain = event.charactersIgnoringModifiers()?.to_string();
        return plain.chars().next().map(|c| CopyKey::Ctrl(c.to_ascii_lowercase()));
    }
    event.characters()?.to_string().chars().next().map(CopyKey::Char)
}

const BADGE_HEIGHT: f64 = 22.0;
const BADGE_PADDING: f64 = 10.0;
const BADGE_MARGIN: f64 = 8.0;

/// The cursor's box over its cell.
fn cursor_view(mtm: MainThreadMarker) -> Retained<NSView> {
    let view = NSView::new(mtm);
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        let accent = NSColor::controlAccentColor();
        layer.setBorderWidth(1.5);
        layer.setBorderColor(Some(&accent.CGColor()));
        layer.setBackgroundColor(Some(&accent.colorWithAlphaComponent(0.35).CGColor()));
        layer.setCornerRadius(1.5);
    }
    view
}

/// The status in a capsule of the system's material, as macOS shows a mode it is in.
fn status_badge(mtm: MainThreadMarker) -> (Retained<NSView>, Retained<NSTextField>) {
    let label = crate::kit::label(mtm, "");
    label.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize_weight(
        objc2_app_kit::NSFont::smallSystemFontSize(),
        term_render::constant!(objc2_app_kit::NSFontWeightMedium),
    )));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let badge = objc2_app_kit::NSVisualEffectView::new(mtm);
    badge.setMaterial(objc2_app_kit::NSVisualEffectMaterial::Popover);
    badge.setBlendingMode(objc2_app_kit::NSVisualEffectBlendingMode::WithinWindow);
    badge.setState(objc2_app_kit::NSVisualEffectState::Active);
    badge.setWantsLayer(true);
    if let Some(layer) = badge.layer() {
        layer.setCornerRadius(BADGE_HEIGHT / 2.0);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
    }
    badge.addSubview(&label);
    (badge.into_super(), label)
}

/// What the status says: the search being typed, why a key failed, or the mode.
fn status_text(status: &model::Status) -> String {
    if let Some(prompt) = &status.prompt {
        return prompt.clone();
    }
    match &status.note {
        Some(Note::PatternNotFound(pattern)) => return tf("Nicht gefunden: {}", &[pattern]),
        Some(Note::InvalidPattern { .. }) => return t("Ungültiger Ausdruck").to_string(),
        Some(Note::NoPreviousPattern) => return t("Noch keine Suche").to_string(),
        _ => {}
    }
    match status.mode {
        Mode::Visual => t("Kopiermodus: Zeichen"),
        Mode::VisualLine => t("Kopiermodus: Zeilen"),
        Mode::VisualBlock => t("Kopiermodus: Block"),
        _ => t("Kopiermodus"),
    }
    .to_string()
}

impl TerminalView {
    /// Turns copy mode on, at the program's cursor or at the bottom of the history the view shows, or off again.
    pub fn toggle_copy_mode(&self) {
        if self.ivars().copy.borrow().is_some() {
            return self.leave_copy_mode();
        }
        let Some((text, view)) = self.with_session(|session| (session.rows_text(), session.rows_view())) else {
            return;
        };
        let scrolled = view.top + view.lines < view.rows;
        let start = if scrolled {
            (view.top + view.lines - 1, 0)
        } else {
            text.cursor
        };
        let mtm = self.mtm();
        let (badge, status) = status_badge(mtm);
        let copy = Copy {
            mode: CopyMode::new(text, start),
            cursor: cursor_view(mtm),
            badge,
            status,
            stale: false,
        };
        self.addSubview(&copy.cursor);
        self.addSubview(&copy.badge);
        *self.ivars().copy.borrow_mut() = Some(copy);
        self.field_hide_prompt();
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        self.repaint();
        self.copy_sync();
    }

    /// Whether copy mode is on.
    pub fn in_copy_mode(&self) -> bool {
        self.ivars().copy.borrow().is_some()
    }

    /// Hands a key press to copy mode while it is on; whether it took it.
    pub(super) fn copy_mode_key(&self, event: &NSEvent) -> bool {
        if !self.in_copy_mode() {
            return false;
        }
        if let Some(key) = key_of(event) {
            self.copy_key(key);
        }
        true
    }

    /// Follows the program's output: copy mode takes the rows anew before its next key.
    pub(super) fn copy_output(&self) {
        match self.ivars().copy.borrow_mut().as_mut() {
            Some(copy) => copy.stale = true,
            None => return,
        }
        self.copy_follow();
    }

    fn copy_take_rows(&self) {
        let stale = self.ivars().copy.borrow().as_ref().is_some_and(|copy| copy.stale);
        if !stale {
            return;
        }
        let Some(text) = self.with_session(Session::rows_text) else {
            return;
        };
        if let Some(copy) = self.ivars().copy.borrow_mut().as_mut() {
            copy.mode.refresh(text);
            copy.stale = false;
        }
    }

    fn copy_key(&self, key: CopyKey) {
        self.copy_take_rows();
        let page = self.with_session(|session| session.rows_view().lines).unwrap_or(1);
        let step = match self.ivars().copy.borrow_mut().as_mut() {
            Some(copy) => copy.mode.key(key, page),
            None => return,
        };
        match step {
            Step::Moved => self.copy_sync(),
            Step::Copy => {
                self.copy_mode_selection();
                self.leave_copy_mode();
            }
            Step::Leave => self.leave_copy_mode(),
            Step::Ignored => {}
        }
    }

    /// Puts the selection on the pasteboard, or the cursor's row without one.
    fn copy_mode_selection(&self) {
        let mut copy = self.ivars().copy.borrow_mut();
        let Some(copy) = copy.as_mut() else { return };
        let marked = copy.mode.selection();
        let (row, _) = copy.mode.cursor();
        let text = self.with_session(|session| {
            if marked.is_none() {
                session.select_rows(SelectionKind::Line, (row, 0), (row, 0));
            }
            session.selected_text()
        });
        if let Some(text) = text.flatten().filter(|text| !text.is_empty()) {
            super::clip::store(&text);
        }
    }

    fn leave_copy_mode(&self) {
        let Some(copy) = self.ivars().copy.borrow_mut().take() else {
            return;
        };
        copy.cursor.removeFromSuperview();
        copy.badge.removeFromSuperview();
        self.repaint();
        self.with_session(|session| {
            session.clear_selection();
            session.scroll(Scroll::Bottom);
        });
        self.field_hide_prompt();
        self.refresh(true);
        self.pass_keys_to_field();
    }

    /// Shows copy mode's selection, scrolls its cursor into view and places the cursor and the status.
    fn copy_sync(&self) {
        let (cursor, selection) = {
            let mut copy = self.ivars().copy.borrow_mut();
            let Some(copy) = copy.as_mut() else { return };
            (copy.mode.cursor(), copy.mode.selection())
        };
        self.with_session(|session| {
            match selection {
                Some((kind, from, to)) => session.select_rows(kind, from, to),
                None => session.clear_selection(),
            }
            session.scroll_to_row(cursor.0);
        });
        self.refresh(true);
        self.copy_follow();
    }

    /// Places copy mode's cursor over its cell and the status in the view's corner, after the view scrolled or the
    /// program wrote.
    pub(super) fn copy_follow(&self) {
        let mut copy = self.ivars().copy.borrow_mut();
        let Some(copy) = copy.as_mut() else { return };
        let ((row, column), width) = (copy.mode.cursor(), copy.mode.cursor_width());
        let line = self.with_session(Session::rows_view).and_then(|view| view.line_of(row));
        match line {
            Some(line) => {
                let frame = self.metrics().cell_rect(Point { line, column }, width);
                copy.cursor.setFrame(frame);
                copy.cursor.setHidden(false);
            }
            None => copy.cursor.setHidden(true),
        }
        copy.status.setStringValue(&ns(&status_text(&copy.mode.status())));
        copy.status.sizeToFit();
        let bounds = self.bounds().size;
        let fitted = copy.status.frame().size;
        copy.status.setFrameOrigin(NSPoint::new(
            BADGE_PADDING,
            ((BADGE_HEIGHT - fitted.height) / 2.0).round(),
        ));
        let width = fitted.width + 2.0 * BADGE_PADDING;
        let y = if self.isFlipped() {
            bounds.height - BADGE_HEIGHT - BADGE_MARGIN
        } else {
            BADGE_MARGIN
        };
        copy.badge.setFrame(NSRect::new(
            NSPoint::new((bounds.width - width - BADGE_MARGIN).max(0.0), y.max(0.0)),
            NSSize::new(width, BADGE_HEIGHT),
        ));
    }

    /// Feeds `keys`, written as Vim writes them, to copy mode, and says where its cursor and selection are.
    #[cfg(feature = "selftest")]
    pub fn copy_keys(&self, keys: &str) -> String {
        for key in model::parse(keys) {
            if !self.in_copy_mode() {
                break;
            }
            self.copy_key(key);
        }
        let mut copy = self.ivars().copy.borrow_mut();
        match copy.as_mut() {
            Some(copy) => format!(
                "cursor {:?} selection {:?} status {:?}",
                copy.mode.cursor(),
                copy.mode.selection(),
                status_text(&copy.mode.status())
            ),
            None => "copy mode off".into(),
        }
    }
}
