//! The mouse: selecting text, reporting to programs that asked for the mouse, the wheel, and ⌘-click on links.
//!
//! ⌥ held keeps the mouse with the terminal while a program has it, so text can still be selected; ⌥-drag selects a
//! rectangle (F113), and ⌘⌥-drag does while a program has the mouse. A double click selects what smart selection
//! recognises (F112), else a word.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;

use block2::RcBlock;

use objc2::DefinedClass;
use objc2::rc::{Retained, Weak};
use objc2_app_kit::{NSCursor, NSEvent, NSEventModifierFlags};
use objc2_foundation::{NSPoint, NSTimer};
use term_session::input::MouseTracking;
use term_session::{CellSide, Modifiers, MouseAction, MouseButton, MouseEvent, Point, Scroll, SelectionKind};

use super::view::TerminalView;
use crate::kit::{guarded, main_soon};
use crate::links;
use term_render::render::clamp_to;

/// How often a selection dragged beyond the top or bottom scrolls on.
const AUTOSCROLL: f64 = 0.05;

#[derive(Default)]
pub(crate) struct MouseState {
    drag: Cell<Option<Drag>>,
    /// The button the program was told is held.
    reported: Cell<Option<MouseButton>>,
    /// The cell of the last movement reported.
    last_cell: Cell<Option<Point>>,
    /// Wheel turns not yet a whole line.
    scroll: Cell<f64>,
    /// Scrolls while a selection is dragged beyond the top or bottom.
    autoscroll: RefCell<Option<Retained<NSTimer>>>,
    /// How far beyond the lines the dragging pointer is, above when negative, and its column.
    beyond: Cell<Option<(f64, usize, CellSide)>>,
    /// The cell under the pointer while ⌘ is held, and whether a link or file is there.
    hover: Cell<Option<(Point, bool)>>,
    /// The resolved files, missing paths and lookups in progress while ⌘ is held.
    files: RefCell<HashMap<PathBuf, FileLookup>>,
    /// Where the pointer was last seen, in the window's coordinates.
    pointer: Cell<Option<NSPoint>>,
    /// The pointing hand shows rather than the I-beam.
    hand: Cell<bool>,
}

enum FileLookup {
    Pending,
    Missing,
    Found(PathBuf),
}

impl MouseState {
    pub(crate) fn stop_autoscroll(&self) {
        if let Some(timer) = self.autoscroll.borrow_mut().take() {
            timer.invalidate();
        }
    }
}

/// A selection the pressed button is making.
#[derive(Debug, Clone, Copy)]
struct Drag {
    origin: Point,
    side: CellSide,
    kind: SelectionKind,
    /// The selection exists; a plain click starts one only once the mouse moves.
    started: bool,
    last: Option<(Point, CellSide)>,
}

/// Where an event happened on the grid.
#[derive(Debug, Clone, Copy)]
struct Spot {
    /// The nearest cell.
    point: Point,
    side: CellSide,
    /// How far beyond the lines the pointer is, in points: negative above them, positive below, else 0.
    beyond: f64,
}

fn modifiers(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        control: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        command: flags.contains(NSEventModifierFlags::Command),
    }
}

/// Whole lines out of `lines`, at most ten thousand either way.
fn whole_lines(lines: f64) -> i32 {
    let count = i32::try_from(clamp_to(lines.abs().trunc(), 10_000)).unwrap_or(0);
    if lines < 0.0 { -count } else { count }
}

static LINK_OPEN: term_base::lane::Lane =
    term_base::lane::Lane::with_start("term-link-open", 2, 32, term_base::priority::user_initiated);

impl TerminalView {
    fn spot(&self, event: &NSEvent) -> Option<Spot> {
        self.spot_at(self.convertPoint_fromView(event.locationInWindow(), None))
    }

    fn spot_at(&self, location: NSPoint) -> Option<Spot> {
        let size = self.ivars().state.size.get()?;
        let (columns, lines) = (usize::from(size.columns), usize::from(size.lines));
        let m = self.metrics();
        let (column, line) = m.position(location.x, location.y);
        let side = if column >= columns as f64 || (column >= 0.0 && column.fract() >= 0.5) {
            CellSide::Right
        } else {
            CellSide::Left
        };
        Some(Spot {
            point: Point {
                line: clamp_to(line.floor(), lines.saturating_sub(1)),
                column: clamp_to(column.floor(), columns.saturating_sub(1)),
            },
            side,
            beyond: if line < 0.0 {
                line * m.cell_height
            } else {
                (line - lines as f64).max(0.0) * m.cell_height
            },
        })
    }

    /// Whether the program takes the mouse for this event: it asked for it and ⌥ is not held.
    fn reporting(&self, event: &NSEvent) -> bool {
        !event.modifierFlags().contains(NSEventModifierFlags::Option) && self.program_takes_mouse()
    }

    /// Whether the program asked for the mouse.
    fn program_takes_mouse(&self) -> bool {
        self.with_session(|session| session.input_modes().mouse != MouseTracking::Off)
            .unwrap_or(false)
    }

    /// Selects what a recogniser of smart selection (F112) finds at `point`; whether one did.
    pub(super) fn select_smart(&self, point: Point) -> bool {
        let range = {
            let screen = self.ivars().screen.borrow();
            let Some(screen) = screen.as_ref() else { return false };
            crate::smart::range_at(&screen.rows, point)
        };
        let Some((first, last)) = range else { return false };
        self.with_session(|session| {
            session.select(SelectionKind::Simple, first, CellSide::Left);
            session.extend_selection(last, CellSide::Right);
        });
        true
    }

    fn report(&self, action: MouseAction, point: Point, event: &NSEvent) {
        let report = MouseEvent {
            action,
            line: point.line,
            column: point.column,
            modifiers: modifiers(event.modifierFlags()),
        };
        self.with_session(|session| session.mouse(&report));
    }

    pub(super) fn mouse_down(&self, event: &NSEvent) {
        let _interval = term_base::signpost::begin(term_base::signpost::name!("terminal mouse down"));
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        let Some(spot) = self.spot(event) else { return };
        let flags = event.modifierFlags();
        if flags.contains(NSEventModifierFlags::Command)
            && !flags.contains(NSEventModifierFlags::Option)
            && self.open_link(spot.point)
        {
            return;
        }
        let mouse = &self.ivars().mouse;
        if self.reporting(event) {
            self.report(MouseAction::Press(MouseButton::Left), spot.point, event);
            mouse.reported.set(Some(MouseButton::Left));
            mouse.last_cell.set(Some(spot.point));
            return;
        }
        let selecting = |kind| Drag {
            origin: spot.point,
            side: spot.side,
            kind,
            started: true,
            last: None,
        };
        let drag = match event.clickCount() {
            2 => selecting(SelectionKind::Word),
            3.. => selecting(SelectionKind::Line),
            _ if flags.contains(NSEventModifierFlags::Shift) && self.selected_text().is_some() => {
                self.with_session(|session| session.extend_selection(spot.point, spot.side));
                selecting(SelectionKind::Simple)
            }
            _ => {
                self.with_session(term_session::Session::clear_selection);
                let block = flags.contains(NSEventModifierFlags::Option)
                    && (flags.contains(NSEventModifierFlags::Command) || !self.program_takes_mouse());
                Drag {
                    started: false,
                    ..selecting(if block {
                        SelectionKind::Block
                    } else {
                        SelectionKind::Simple
                    })
                }
            }
        };
        if drag.started
            && event.clickCount() >= 2
            && !(drag.kind == SelectionKind::Word && self.select_smart(spot.point))
        {
            self.with_session(|session| session.select(drag.kind, spot.point, spot.side));
        }
        mouse.drag.set(Some(drag));
        self.refresh_interactive();
    }

    pub(super) fn mouse_dragged(&self, event: &NSEvent) {
        let _interval = term_base::signpost::begin(term_base::signpost::name!("terminal selection drag"));
        let Some(spot) = self.spot(event) else { return };
        let mouse = &self.ivars().mouse;
        if let Some(button) = mouse.reported.get() {
            if mouse.last_cell.replace(Some(spot.point)) != Some(spot.point) {
                self.report(MouseAction::Move(Some(button)), spot.point, event);
            }
            return;
        }
        let Some(mut drag) = mouse.drag.get() else { return };
        mouse.beyond.set(Some((spot.beyond, spot.point.column, spot.side)));
        if spot.beyond == 0.0 {
            mouse.stop_autoscroll();
        } else {
            self.start_autoscroll();
        }
        let end = (spot.point, spot.side);
        if drag.last == Some(end) {
            return;
        }
        self.with_session(|session| {
            if !drag.started {
                session.select(drag.kind, drag.origin, drag.side);
                drag.started = true;
            }
            session.extend_selection(spot.point, spot.side);
        });
        drag.last = Some(end);
        mouse.drag.set(Some(drag));
        self.refresh_interactive();
    }

    fn start_autoscroll(&self) {
        let mut timer = self.ivars().mouse.autoscroll.borrow_mut();
        if timer.is_some() {
            return;
        }
        let view = Weak::from(self);
        let block = RcBlock::new(move |_: NonNull<NSTimer>| {
            guarded((), || {
                if let Some(view) = view.load() {
                    view.autoscroll();
                }
            });
        });
        // SAFETY: The block holds the view weakly; the main run loop fires the timer on the main thread.
        *timer = Some(unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(AUTOSCROLL, true, &block) });
    }

    /// Scrolls the history under a selection dragged beyond the lines, faster the farther the pointer is.
    fn autoscroll(&self) {
        let mouse = &self.ivars().mouse;
        let size = self.ivars().state.size.get();
        let (Some((beyond, column, side)), Some(size), Some(_)) = (mouse.beyond.get(), size, mouse.drag.get()) else {
            mouse.stop_autoscroll();
            return;
        };
        if beyond == 0.0 {
            mouse.stop_autoscroll();
            return;
        }
        let step = 1 + whole_lines((beyond / self.metrics().cell_height).abs()).min(9);
        let (lines, line) = if beyond < 0.0 {
            (step, 0)
        } else {
            (-step, usize::from(size.lines).saturating_sub(1))
        };
        self.with_session(|session| {
            session.scroll(Scroll::Lines(lines));
            session.extend_selection(Point { line, column }, side);
        });
        self.refresh_interactive();
    }

    pub(super) fn mouse_up(&self, event: &NSEvent) {
        let mouse = &self.ivars().mouse;
        let drag = mouse.drag.take();
        let selected = drag.is_some();
        mouse.stop_autoscroll();
        let plain = !event
            .modifierFlags()
            .intersects(NSEventModifierFlags::DeviceIndependentFlagsMask);
        if let Some(drag) = drag
            && !drag.started
            && plain
            && event.clickCount() == 1
        {
            self.click_in_prompt(drag.origin);
        }
        if selected && super::host().config().terminal.copy_on_select {
            self.copy_selection();
        }
        if let Some(button) = mouse.reported.take()
            && let Some(spot) = self.spot(event)
        {
            self.report(MouseAction::Release(button), spot.point, event);
        }
        self.pass_keys_to_field();
    }

    /// Whether a button is held in the view, for a selection or for the program.
    pub(crate) fn mouse_held(&self) -> bool {
        let mouse = &self.ivars().mouse;
        mouse.drag.get().is_some() || mouse.reported.get().is_some()
    }

    /// The right, middle and side buttons; whether the program took the event, else the right button opens the menu.
    pub(super) fn other_button(&self, event: &NSEvent, down: bool) -> bool {
        let button = match event.buttonNumber() {
            1 => MouseButton::Right,
            2 => MouseButton::Middle,
            3 => MouseButton::Back,
            4 => MouseButton::Forward,
            _ => return false,
        };
        let mouse = &self.ivars().mouse;
        let Some(spot) = self.spot(event) else { return false };
        if down {
            if !self.reporting(event) {
                return false;
            }
            self.report(MouseAction::Press(button), spot.point, event);
            mouse.reported.set(Some(button));
            return true;
        }
        if mouse.reported.get() != Some(button) {
            return false;
        }
        mouse.reported.set(None);
        self.report(MouseAction::Release(button), spot.point, event);
        true
    }

    /// Movement, reported to a program that follows the mouse whenever it enters another cell.
    pub(super) fn mouse_moved(&self, event: &NSEvent) {
        self.update_pointer(event.modifierFlags(), Some(event.locationInWindow()));
        if event.modifierFlags().contains(NSEventModifierFlags::Option) {
            return;
        }
        let Some(spot) = self.spot(event) else { return };
        let mouse = &self.ivars().mouse;
        if mouse.last_cell.replace(Some(spot.point)) == Some(spot.point) {
            return;
        }
        self.report(MouseAction::Move(mouse.reported.get()), spot.point, event);
    }

    /// The wheel and trackpad: the history, or the program when it takes the mouse or shows the alternate screen.
    pub(super) fn scroll_wheel(&self, event: &NSEvent) {
        let _interval = term_base::signpost::begin(term_base::signpost::name!("terminal scroll wheel"));
        let delta = event.scrollingDeltaY();
        let lines = if event.hasPreciseScrollingDeltas() {
            delta / self.metrics().cell_height
        } else {
            delta
        };
        let mouse = &self.ivars().mouse;
        let total = mouse.scroll.get() + lines;
        let whole = whole_lines(total);
        mouse.scroll.set(total - f64::from(whole));
        if whole == 0 {
            return;
        }
        let Some(spot) = self.spot(event) else { return };
        let flags = event.modifierFlags();
        self.with_session(|session| {
            if flags.contains(NSEventModifierFlags::Option) {
                session.scroll(Scroll::Lines(whole));
            } else {
                session.wheel(whole, spot.point, modifiers(flags));
            }
        });
        self.refresh_interactive();
    }

    /// The pointing hand while ⌘ is held over a link or a file ⌘-click opens, else the I-beam.
    /// `seen` is the pointer's place in the window when the event has one; a key event has none.
    pub(super) fn update_pointer(&self, flags: NSEventModifierFlags, seen: Option<NSPoint>) {
        let Some(window) = self.window() else { return };
        let mouse = &self.ivars().mouse;
        if seen.is_some() {
            mouse.pointer.set(seen);
        }
        let in_window = mouse
            .pointer
            .get()
            .unwrap_or_else(|| window.mouseLocationOutsideOfEventStream());
        let location = self.convertPoint_fromView(in_window, None);
        if !objc2_foundation::NSPointInRect(location, self.bounds()) {
            return;
        }
        let command = flags.contains(NSEventModifierFlags::Command) && !flags.contains(NSEventModifierFlags::Option);
        let link = match self.spot_at(location).filter(|_| command) {
            Some(spot) => match mouse.hover.get() {
                Some((point, link)) if point == spot.point => link,
                _ => {
                    let link = self.link_at(spot.point);
                    mouse.hover.set(Some((spot.point, link)));
                    link
                }
            },
            None => {
                mouse.hover.set(None);
                mouse.files.borrow_mut().clear();
                false
            }
        };
        mouse.hand.set(link);
        if link {
            NSCursor::pointingHandCursor().set();
        } else {
            NSCursor::IBeamCursor().set();
        }
    }

    /// Whether ⌘-click at `point` opens something.
    fn link_at(&self, point: Point) -> bool {
        let (url, word) = {
            let screen = self.ivars().screen.borrow();
            let Some(screen) = screen.as_ref() else { return false };
            (links::url_at(&screen.rows, point), links::path_at(&screen.rows, point))
        };
        if url.is_some() {
            return true;
        }
        let Some(file) = word else { return false };
        let named = self.named(&file.path);
        if let Some(found) = self.ivars().mouse.files.borrow().get(&named) {
            return matches!(found, FileLookup::Found(_));
        }
        self.look_up(named);
        false
    }

    /// Looks for the file at `named` on the disk lane; the pointer turns into a hand once it is found.
    fn look_up(&self, named: PathBuf) {
        self.ivars()
            .mouse
            .files
            .borrow_mut()
            .insert(named.clone(), FileLookup::Pending);
        let id = self.id();
        let path = named.clone();
        let result_path = named.clone();
        let queued = term_base::lane::DISK.ask(
            move || existing_file(&path),
            move |result| {
                let Some(found) = result else { return };
                main_soon(move || {
                    let Some(view) = super::find(id) else { return };
                    let mouse = &view.ivars().mouse;
                    {
                        let mut files = mouse.files.borrow_mut();
                        let Some(entry @ FileLookup::Pending) = files.get_mut(&result_path) else {
                            return;
                        };
                        *entry = found.map_or(FileLookup::Missing, FileLookup::Found);
                    }
                    mouse.hover.set(None);
                    view.update_pointer(NSEvent::modifierFlags_class(), None);
                });
            },
        );
        if queued.is_err() {
            self.ivars().mouse.files.borrow_mut().remove(&named);
            term_base::diag::report("looking for a file under the pointer", "the disk lane is full");
        }
    }

    /// Opens the link or file at `point`; whether there was one.
    fn open_link(&self, point: Point) -> bool {
        let (url, word) = {
            let screen = self.ivars().screen.borrow();
            let Some(screen) = screen.as_ref() else { return false };
            (links::url_at(&screen.rows, point), links::path_at(&screen.rows, point))
        };
        if let Some(url) = url {
            self.open_url(&url);
            return true;
        }
        let Some(file) = word else { return false };
        let (line, column) = (file.line.unwrap_or(1), file.column.unwrap_or(1));
        let named = self.named(&file.path);
        let window = self.window_id();
        let cached = {
            let files = self.ivars().mouse.files.borrow();
            match files.get(&named) {
                Some(FileLookup::Found(path)) => Some(path.clone()),
                Some(FileLookup::Missing) => return false,
                _ => None,
            }
        };
        if let Some(path) = cached {
            super::host().opened(window, super::Opened::File { path, line, column });
            return true;
        }
        let id = self.id();
        LINK_OPEN
            .ask(
                move || existing_file(&named),
                move |path| {
                    if let Some(path) = path.flatten() {
                        main_soon(move || {
                            if super::find(id).is_some_and(|view| view.window_id() == window) {
                                super::host().opened(window, super::Opened::File { path, line, column });
                            }
                        });
                    }
                },
            )
            .is_ok()
    }

    /// The path `word` names, relative to the shell's working directory.
    fn named(&self, word: &str) -> PathBuf {
        match word.strip_prefix("~/") {
            Some(rest) => term_base::home().join(rest),
            None => self.ivars().cwd.borrow().join(word),
        }
    }
}

/// The file at `path` as editor tabs know it by, if there is one.
fn existing_file(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().filter(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wheel_turns_become_whole_lines_toward_zero() {
        assert_eq!(whole_lines(2.7), 2);
        assert_eq!(whole_lines(-2.7), -2);
        assert_eq!(whole_lines(0.4), 0);
        assert_eq!(whole_lines(1e9), 10_000);
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
