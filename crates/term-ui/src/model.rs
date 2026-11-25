//! What a terminal view decides without AppKit (E98): when a frame is due, how a bell rings, when VoiceOver hears of
//! new output, how long a notice stays, which commands its menu offers and when they work.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use splat_i18n::{t, tf};
use term_session::{Highlight, Size, Snapshot};

/// Output this long after the last snapshot is taken at once; output sooner waits for the display's next frame.
const FRAME: Duration = Duration::from_millis(8);
pub(crate) const BELL_FLASH: Duration = Duration::from_millis(140);
/// Bells closer than this ring once.
const BELL_GAP: Duration = Duration::from_millis(250);
/// VoiceOver hears of new output at most this often.
const ANNOUNCE_GAP: Duration = Duration::from_millis(500);
/// While a size is dragged, the grid reflows its scrollback and the program hears SIGWINCH at most this often.
const RESIZE_GAP: Duration = Duration::from_millis(100);
/// The tab takes a new title at most this often, as a spinner in a program's title changes it on every frame.
pub(crate) const TITLE_GAP: Duration = Duration::from_millis(250);

#[derive(Default)]
pub(crate) struct State {
    pub(crate) first_responder: Cell<bool>,
    pub(crate) focused: Cell<bool>,
    pub(crate) exited: Cell<bool>,
    pub(crate) prompts: Cell<usize>,
    /// Output arrived while the view was hidden.
    pub(crate) stale: Cell<bool>,
    pub(crate) frame_pending: Cell<bool>,
    last_frame: Cell<Option<Instant>>,
    pub(crate) cursor_off: Cell<bool>,
    last_bell: Cell<Option<Instant>>,
    flash_until: Cell<Option<Instant>>,
    announced: Cell<Option<Instant>>,
    pub(crate) size: Cell<Option<Size>>,
    /// The view took another size while its window was out of sight, which the grid takes once the window shows.
    pub(crate) size_waits: Cell<bool>,
    /// Fitting the grid to a dragged size.
    pub(crate) resizes: Pace,
    /// Handing the title to the tab.
    pub(crate) titles: Pace,
    /// The title the tab was given last.
    pub(crate) tab_title: RefCell<Option<String>>,
    notices: Cell<u64>,
    /// A GPU frame was skipped while the window was hidden behind others.
    pub(crate) unpresented: Cell<bool>,
    /// The view is drawing a picture of itself, which only Core Graphics can.
    pub(crate) picturing: Cell<bool>,
}

/// Spaces out a change that costs: at once when the last one is `gap` ago, else once after the rest of the gap.
#[derive(Default)]
pub(crate) struct Pace {
    last: Cell<Option<Instant>>,
    /// A later run is due; its timer clears this before it asks again.
    pub(crate) pending: Cell<bool>,
}

impl Pace {
    /// When the change asked for at `now` happens; `Now` notes it as the last.
    pub(crate) fn ask(&self, now: Instant, gap: Duration) -> Frame {
        if self.pending.get() {
            return Frame::Pending;
        }
        let since = self.last.get().map_or(gap, |last| now - last);
        if since < gap {
            self.pending.set(true);
            return Frame::After(gap - since);
        }
        self.last.set(Some(now));
        Frame::Now
    }
}

/// What a bell does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Bell {
    /// It came too soon after the last one.
    Ignore,
    /// The system sound, for a view out of sight or a user who wants less motion.
    Beep,
    /// The view flashes until then.
    Flash(Instant),
}

/// When the snapshot for new output is taken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Frame {
    /// The view is hidden; it takes one once it shows again.
    Stale,
    /// One is due already.
    Pending,
    Now,
    After(Duration),
}

/// A command of the terminal's context menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Command {
    Copy,
    Paste,
    SelectAll,
    Clear,
    NewTerminalHere,
    ClosePane,
}

impl State {
    /// What a bell at `now` does in a view that is `visible` or not.
    pub(crate) fn bell(&self, now: Instant, visible: bool, reduce_motion: bool) -> Bell {
        if self.last_bell.get().is_some_and(|last| now - last < BELL_GAP) {
            return Bell::Ignore;
        }
        self.last_bell.set(Some(now));
        if !visible || reduce_motion {
            return Bell::Beep;
        }
        let until = now + BELL_FLASH;
        self.flash_until.set(Some(until));
        Bell::Flash(until)
    }

    pub(crate) fn flashing(&self, now: Instant) -> bool {
        self.flash_until.get().is_some_and(|until| now < until)
    }

    /// When the snapshot for output that arrived at `now` is taken, in a view that is `hidden` or not.
    pub(crate) fn frame(&self, now: Instant, hidden: bool) -> Frame {
        if hidden {
            self.stale.set(true);
            return Frame::Stale;
        }
        if self.frame_pending.get() {
            return Frame::Pending;
        }
        let since = self.last_frame.get().map_or(FRAME, |last| now - last);
        if since >= FRAME {
            return Frame::Now;
        }
        self.frame_pending.set(true);
        Frame::After(FRAME - since)
    }

    /// When a new size of the view at `now` reaches the grid, while a size is `dragging` or not.
    pub(crate) fn resize(&self, now: Instant, dragging: bool) -> Frame {
        self.resizes
            .ask(now, if dragging { RESIZE_GAP } else { Duration::ZERO })
    }

    /// Notes that a snapshot was taken at `now`, which shows what a pending frame would have.
    pub(crate) fn framed(&self, now: Instant) {
        self.last_frame.set(Some(now));
        self.stale.set(false);
        self.frame_pending.set(false);
    }

    /// Whether VoiceOver hears of the output at `now`: while the view has the focus, at most every half second.
    pub(crate) fn announce(&self, now: Instant) -> bool {
        if !self.focused.get() || self.announced.get().is_some_and(|last| now - last < ANNOUNCE_GAP) {
            return false;
        }
        self.announced.set(Some(now));
        true
    }

    /// The number of a notice that shows now.
    pub(crate) fn next_notice(&self) -> u64 {
        let shown = self.notices.get() + 1;
        self.notices.set(shown);
        shown
    }

    /// Whether notice `shown` is still the last one, so its time running out hides it.
    pub(crate) fn is_last_notice(&self, shown: u64) -> bool {
        self.notices.get() == shown
    }

    /// Whether the cursor blinks: one the program wants blinking and visible, in a view with the focus, before the
    /// program ended.
    pub(crate) fn blinking(&self, cursor_blinks: bool) -> bool {
        self.focused.get() && !self.exited.get() && cursor_blinks
    }

    /// The lines of the grid when `fit` fit the view: after the program ended, one less, for the line that says so.
    pub(crate) fn grid_lines(&self, fit: u16) -> u16 {
        if self.exited.get() {
            fit.saturating_sub(1).max(1)
        } else {
            fit
        }
    }
}

/// The line that says the program ended: where the cursor stands if that line is empty, else the next one, which may
/// be the line kept free below the grid.
pub(crate) fn final_line(screen: &Snapshot) -> usize {
    let cursor = screen.cursor.point;
    let blank = screen
        .rows
        .get(cursor.line)
        .is_some_and(|row| row.iter().all(|cell| cell.ch == ' ' || cell.ch == '\0'));
    if blank { cursor.line } else { cursor.line + 1 }
}

/// The lines that look different as the selection goes from `before` to `now`: those its ends passed over, or every
/// line of a block, whose columns span all of its lines.
pub(crate) fn selection_lines(before: Option<Highlight>, now: Option<Highlight>) -> Vec<usize> {
    let span = |h: Highlight| h.start.line..=h.end.line;
    match (before, now) {
        (a, b) if a == b => Vec::new(),
        (Some(a), Some(b)) if !a.block && !b.block => {
            let between = |x: usize, y: usize| x.min(y)..=x.max(y);
            between(a.start.line, b.start.line)
                .chain(between(a.end.line, b.end.line))
                .collect()
        }
        (a, b) => a
            .into_iter()
            .flat_map(span)
            .chain(b.into_iter().flat_map(span))
            .collect(),
    }
}

/// The most characters of a program's title that the tab and VoiceOver get, far more than either shows; a title of
/// megabytes would otherwise be laid out whole by AppKit on every change.
const LONGEST_TITLE: usize = 1024;

/// The title a program set, if it holds more than spaces, up to its `LONGEST_TITLE`-th character.
pub(crate) fn shown_title(title: Option<String>) -> Option<String> {
    title.filter(|t| !t.trim().is_empty()).map(|mut title| {
        if let Some((cut, _)) = title.char_indices().nth(LONGEST_TITLE) {
            title.truncate(cut);
        }
        title
    })
}

/// What VoiceOver calls the view.
pub(crate) fn label(title: Option<&str>) -> String {
    match title {
        Some(title) => tf("Terminal: {}", &[title]),
        None => t("Terminal").to_string(),
    }
}

/// The context menu, `None` for a separator.
pub(crate) fn menu() -> [Option<(&'static str, Command)>; 8] {
    [
        Some((t("Kopieren"), Command::Copy)),
        Some((t("Einsetzen"), Command::Paste)),
        Some((t("Alles auswählen"), Command::SelectAll)),
        None,
        Some((t("Terminal leeren"), Command::Clear)),
        None,
        Some((t("Neues Terminal hier"), Command::NewTerminalHere)),
        Some((t("Bereich schließen"), Command::ClosePane)),
    ]
}

/// Whether `command` works: copying needs a selection, pasting something the shell takes, the rest a started shell.
pub(crate) fn enabled(command: Command, started: bool, selection: bool, can_paste: impl FnOnce() -> bool) -> bool {
    match command {
        Command::Copy => selection,
        Command::Paste => started && can_paste(),
        Command::SelectAll | Command::Clear => started,
        Command::NewTerminalHere | Command::ClosePane => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_end_of_the_program_is_told_on_the_cursor_line_when_it_is_empty_else_below() {
        let cell = |ch| term_session::Cell {
            ch,
            zero_width: vec![],
            fg: term_session::Rgb::new(0, 0, 0),
            bg: term_session::Rgb::new(255, 255, 255),
            underline_color: None,
            flags: term_session::CellFlags::default(),
            hyperlink: None,
        };
        let screen = |row: Vec<term_session::Cell>| Snapshot {
            columns: row.len(),
            lines: 2,
            rows: vec![vec![cell('$'); row.len()], row],
            cursor: term_session::Cursor {
                point: term_session::Point { line: 1, column: 0 },
                shape: term_session::CursorShape::Block,
                visible: true,
                blinking: false,
                wide: false,
            },
            selection: None,
            display_offset: 0,
            history: 0,
            damage: term_session::Damage::Full,
            background: term_session::Rgb::new(255, 255, 255),
            modes: term_session::InputModes::default(),
        };
        assert_eq!(final_line(&screen(vec![cell(' '), cell('\0')])), 1);
        assert_eq!(final_line(&screen(vec![cell('o'), cell('k')])), 2);
        assert_eq!(shown_title(Some("  ".into())), None);
        assert_eq!(shown_title(Some("vim".into())).as_deref(), Some("vim"));
    }

    #[test]
    fn a_dragged_size_reaches_the_grid_at_most_every_gap_and_any_other_at_once() {
        let state = State::default();
        let now = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(state.resize(now, true), Frame::Now);
        assert_eq!(state.resize(now + ms(30), true), Frame::After(ms(70)));
        assert_eq!(state.resize(now + ms(60), true), Frame::Pending);
        state.resizes.pending.set(false);
        assert_eq!(state.resize(now + ms(100), true), Frame::Now);
        assert_eq!(
            state.resize(now + ms(110), false),
            Frame::Now,
            "not dragged, it fits at once"
        );
    }

    #[test]
    fn a_title_reaches_the_tab_at_once_and_then_at_most_every_gap() {
        let state = State::default();
        let now = Instant::now();
        let ms = Duration::from_millis;
        assert_eq!(state.titles.ask(now, TITLE_GAP), Frame::Now);
        assert_eq!(state.titles.ask(now + ms(50), TITLE_GAP), Frame::After(ms(200)));
        assert_eq!(state.titles.ask(now + ms(100), TITLE_GAP), Frame::Pending);
        state.titles.pending.set(false);
        assert_eq!(state.titles.ask(now + ms(250), TITLE_GAP), Frame::Now);
    }

    #[test]
    fn bells_close_together_ring_once_and_out_of_sight_they_beep() {
        let state = State::default();
        let now = Instant::now();
        assert_eq!(state.bell(now, true, false), Bell::Flash(now + BELL_FLASH));
        assert!(state.flashing(now));
        assert!(!state.flashing(now + BELL_FLASH));
        assert_eq!(state.bell(now + Duration::from_millis(100), true, false), Bell::Ignore);
        assert_eq!(state.bell(now + Duration::from_millis(300), false, false), Bell::Beep);
        assert_eq!(state.bell(now + Duration::from_millis(600), true, true), Bell::Beep);
    }

    #[test]
    fn output_draws_at_most_once_a_frame_and_not_while_hidden() {
        let state = State::default();
        let now = Instant::now();
        assert_eq!(state.frame(now, true), Frame::Stale);
        assert!(state.stale.get());
        assert_eq!(state.frame(now, false), Frame::Now);
        state.framed(now);
        assert!(!state.stale.get());
        let soon = now + Duration::from_millis(3);
        assert_eq!(state.frame(soon, false), Frame::After(FRAME - Duration::from_millis(3)));
        assert_eq!(state.frame(soon, false), Frame::Pending);
        state.framed(soon);
        assert!(
            !state.frame_pending.get(),
            "a snapshot taken meanwhile serves the pending frame"
        );
        assert_eq!(state.frame(soon + FRAME, false), Frame::Now);
    }

    #[test]
    fn voiceover_hears_of_output_only_with_the_focus_and_not_too_often() {
        let state = State::default();
        let now = Instant::now();
        assert!(!state.announce(now));
        state.focused.set(true);
        assert!(state.announce(now));
        assert!(!state.announce(now + Duration::from_millis(200)));
        assert!(state.announce(now + Duration::from_millis(600)));
    }

    #[test]
    fn only_the_last_notice_hides_when_its_time_runs_out() {
        let state = State::default();
        let first = state.next_notice();
        let second = state.next_notice();
        assert!(!state.is_last_notice(first));
        assert!(state.is_last_notice(second));
    }

    #[test]
    fn an_ended_program_keeps_a_line_free_and_stops_the_blinking() {
        let state = State::default();
        state.focused.set(true);
        assert!(state.blinking(true));
        assert_eq!(state.grid_lines(24), 24);
        state.exited.set(true);
        assert!(!state.blinking(true));
        assert_eq!(state.grid_lines(24), 23);
        assert_eq!(state.grid_lines(1), 1);
    }

    #[test]
    fn the_menu_works_as_far_as_the_shell_does() {
        assert!(!enabled(Command::Copy, true, false, || true));
        assert!(!enabled(Command::Paste, false, false, || true));
        assert!(enabled(Command::Paste, true, false, || true));
        assert!(!enabled(Command::Clear, false, false, || true));
        assert!(enabled(Command::NewTerminalHere, false, false, || false));
        assert_eq!(menu().iter().filter(|e| e.is_none()).count(), 2);
    }

    #[test]
    fn a_selection_redraws_the_lines_its_ends_passed_and_a_block_all_of_its_lines() {
        let at = |line, column| term_session::Point { line, column };
        let simple = |from: (usize, usize), to: (usize, usize)| Highlight {
            start: at(from.0, from.1),
            end: at(to.0, to.1),
            block: false,
        };
        let grown = selection_lines(Some(simple((2, 3), (5, 1))), Some(simple((2, 3), (7, 4))));
        assert_eq!(grown, [2, 5, 6, 7]);
        assert_eq!(
            selection_lines(Some(simple((2, 3), (5, 1))), Some(simple((2, 3), (5, 1)))),
            [0usize; 0]
        );
        assert_eq!(selection_lines(None, Some(simple((4, 0), (5, 2)))), [4, 5]);
        let block = Highlight {
            block: true,
            ..simple((1, 2), (3, 4))
        };
        assert_eq!(
            selection_lines(Some(block), Some(Highlight { end: at(3, 6), ..block })),
            [1, 2, 3, 1, 2, 3]
        );
    }

    #[test]
    fn a_title_of_megabytes_is_cut_after_its_first_characters() {
        let long = "ä".repeat(2_000_000);
        let shown = shown_title(Some(long)).unwrap();
        assert_eq!(shown.chars().count(), LONGEST_TITLE);
        assert!(shown.chars().all(|c| c == 'ä'));
        let fits = "x".repeat(LONGEST_TITLE);
        assert_eq!(shown_title(Some(fits.clone())), Some(fits));
    }

    #[test]
    fn a_blank_title_is_no_title() {
        assert_eq!(shown_title(Some("  ".into())), None);
        assert_eq!(label(Some("vim")), tf("Terminal: {}", &["vim"]));
        assert_eq!(label(None), t("Terminal"));
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
