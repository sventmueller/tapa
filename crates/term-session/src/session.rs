//! A terminal session: a program in a PTY, read on a thread of its own, its screen kept for the view.

use std::collections::HashMap;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use polling::Poller;
use term_base::LockIgnoringPoison;
use term_base::{ProcessId, Spawner};

use crate::event::Event;
use crate::fair::FairMutex;
use crate::foreground::{self, Foreground};
use crate::input::{
    InputModes, Key, KeyCode, Modifiers, MouseEvent, Wheel, encode_focus, encode_key, encode_mouse, encode_paste,
    encode_wheel,
};
use crate::paste::{self, PasteAction, QuoteStyle};
use crate::pty;
use crate::snapshot::{self, Palette, Point, Snapshot};
use crate::{integration, io as reader, sys};

mod blocks;
mod copy;
mod format;
mod marks;
mod restored;
pub use blocks::{Block, BlockPart, BlockState, BlocksView, ShownBlock};
pub use copy::{RowsText, RowsView};
pub use format::{Look, StyledRun, StyledText};
pub use marks::{ScrollMark, ScrollMarks};

/// The most history lines a session keeps, whatever the settings say.
pub(crate) const MAX_SCROLLBACK: usize = 100_000;

/// What runs in the terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Program {
    /// The user's shell as a login shell, with shell integration for zsh, bash and fish.
    LoginShell(PathBuf),
    /// A program with its arguments, run as it is.
    Command {
        /// The executable.
        program: PathBuf,
        /// Its arguments.
        args: Vec<String>,
    },
    /// `ssh` with its arguments, taking the shell integration to the other host where both integrations are on (F89).
    Ssh(Vec<String>),
}

/// The size of the terminal in cells and of one cell in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Size {
    pub columns: u16,
    pub lines: u16,
    /// Width of a cell in pixels.
    pub cell_width: u16,
    /// Height of a cell in pixels.
    pub cell_height: u16,
}

impl Default for Size {
    fn default() -> Size {
        Size {
            columns: 80,
            lines: 24,
            cell_width: 8,
            cell_height: 16,
        }
    }
}

impl Size {
    fn grid(self) -> term_vt::Size {
        term_vt::Size {
            columns: usize::from(self.columns),
            lines: usize::from(self.lines),
        }
    }

    /// The size the PTY is told, whose pixel sizes must stay within 16 bits.
    fn winsize(self) -> libc::winsize {
        let columns = self.columns.max(2);
        let lines = self.lines.max(1);
        libc::winsize {
            ws_row: lines,
            ws_col: columns,
            ws_xpixel: columns.saturating_mul(self.cell_width.min(u16::MAX / columns)),
            ws_ypixel: lines.saturating_mul(self.cell_height.min(u16::MAX / lines)),
        }
    }

    pub(crate) fn pixels(self) -> (u32, u32) {
        (
            u32::from(self.columns) * u32::from(self.cell_width),
            u32::from(self.lines) * u32::from(self.cell_height),
        )
    }
}

/// How a session starts.
#[derive(Debug, Clone)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Options {
    /// What runs in it.
    pub program: Program,
    /// The working directory it starts in.
    pub cwd: PathBuf,
    pub size: Size,
    /// How many lines of history it keeps, at most `MAX_SCROLLBACK`.
    pub scrollback: usize,
    /// Where the shell integration scripts are written; `None` turns shell integration off.
    pub shell_integration: Option<PathBuf>,
    /// The shell integration's `ssh` takes it to the shell on the other host (F89).
    pub ssh_integration: bool,
    /// The colours.
    pub palette: Palette,
    /// Programs may read the pasteboard (OSC 52); off by default because any program, also one on a remote host, could
    /// read whatever was copied.
    pub allow_clipboard_read: bool,
    /// Programs may switch on the kitty keyboard protocol.
    pub kitty_keyboard: bool,
    /// How long closing waits at each step before it escalates: hang-up, SIGTERM, SIGKILL.
    pub close_grace: Duration,
    /// The text of an earlier session in this place, shown above the program's first output (F71).
    pub restored: Vec<String>,
}

impl Options {
    /// The user's login shell in `cwd`, with shell integration and the defaults.
    pub fn new(cwd: impl Into<PathBuf>) -> Options {
        Options {
            program: Program::LoginShell(term_base::shell_env::user_shell()),
            cwd: cwd.into(),
            size: Size::default(),
            scrollback: 10_000,
            shell_integration: Some(integration::default_dir()),
            ssh_integration: true,
            palette: Palette::default(),
            allow_clipboard_read: false,
            kitty_keyboard: true,
            close_grace: Duration::from_secs(2),
            restored: Vec::new(),
        }
    }
}

/// What a search found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Found {
    /// How many matches there are, at most 10 000.
    pub count: usize,
    /// Which one shows, from 0.
    pub current: Option<usize>,
}

/// How to scroll through the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Scroll {
    /// By lines; positive towards older output.
    Lines(i32),
    /// One screen towards older output.
    PageUp,
    /// One screen towards newer output.
    PageDown,
    /// To the oldest line.
    Top,
    /// Back to the live screen.
    Bottom,
}

/// How a selection grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SelectionKind {
    /// Cell by cell.
    Simple,
    /// A rectangle.
    Block,
    /// Whole words, as after a double click.
    Word,
    /// Whole lines, as after a triple click.
    Line,
}

/// Which half of a cell the pointer is in, which decides whether the cell belongs to a selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(missing_docs, reason = "the names say it")]
pub enum CellSide {
    Left,
    Right,
}

type Listener = Box<dyn Fn(Event) + Send + Sync>;
pub(crate) type ClipboardReply = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// What the session and its reader thread share.
pub(crate) struct Shared {
    /// The screen, behind the lock the reader thread and the owner share.
    pub(crate) own: FairMutex<crate::own::Own>,
    pub(crate) palette: Mutex<Palette>,
    pub(crate) size: Mutex<Size>,
    pub(crate) clipboard_reply: Mutex<Option<ClipboardReply>>,
    redraw_pending: AtomicBool,
    full_redraw: AtomicBool,
    pub(crate) running: AtomicBool,
    closed: AtomicBool,
    listener: Listener,
    pub(crate) publisher: crate::frames::Publisher,
}

impl Shared {
    /// A snapshot of the screen as it is now, with the colours in force.
    pub(crate) fn build_snapshot(&self) -> Snapshot {
        let palette = self.palette.lock_ignoring_poison().clone();
        let mut snapshot = self.own.lock().snapshot(&palette);
        if self.full_redraw.swap(false, Ordering::AcqRel) {
            snapshot.damage = snapshot::Damage::Full;
        }
        snapshot
    }

    /// Hands `event` to the owner unless the session was closed.
    pub(crate) fn emit(&self, event: Event) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.listener)(event)));
    }

    /// Output or a change of the screen's state: a new frame for the renderer, and a redraw for the owner unless one
    /// is already pending.
    pub(crate) fn wake(&self) {
        self.publisher.changed(self);
        if !self.redraw_pending.swap(true, Ordering::AcqRel) {
            self.emit(Event::Redraw);
        }
    }
}

/// A program running in a terminal. Dropping the session closes it.
pub struct Session {
    shared: Arc<Shared>,
    input: Sender<Vec<u8>>,
    poller: Arc<Poller>,
    master: File,
    pid: u32,
    process: ProcessId,
    spawner: Arc<dyn Spawner>,
    close_grace: Duration,
    closed: AtomicBool,
    lock_times: LockClock,
}

/// The longest `Session::snapshot` waited for the screen's lock and then held it, since they were last taken.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LockTimes {
    /// Until the reader thread let the lock go.
    pub wait: Duration,
    /// While the snapshot was taken.
    pub hold: Duration,
}

/// `LockTimes` as nanoseconds.
#[derive(Default)]
struct LockClock {
    wait: AtomicU64,
    hold: AtomicU64,
}

impl LockClock {
    fn note(&self, wait: Duration, hold: Duration) {
        let nanos = |d: Duration| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX);
        self.wait.fetch_max(nanos(wait), Ordering::Relaxed);
        self.hold.fetch_max(nanos(hold), Ordering::Relaxed);
    }

    fn take(&self) -> LockTimes {
        LockTimes {
            wait: Duration::from_nanos(self.wait.swap(0, Ordering::Relaxed)),
            hold: Duration::from_nanos(self.hold.swap(0, Ordering::Relaxed)),
        }
    }
}

impl Session {
    /// Starts `options.program` in a new PTY with `env`, the user's login-shell environment, and registers it with
    /// `spawner`. `listener` hears every `Event` on the session's reader thread and must not block it.
    ///
    /// ```no_run
    /// use std::sync::mpsc::channel;
    ///
    /// use std::sync::Arc;
    ///
    /// use term_base::{Processes, Spawner};
    /// use term_session::{Event, Options, Program, Session};
    ///
    /// let mut options = Options::new("/tmp");
    /// options.program = Program::Command { program: "/bin/echo".into(), args: vec!["hello".into()] };
    /// options.shell_integration = None;
    /// let (tx, rx) = channel();
    /// let spawner: Arc<dyn Spawner> = Arc::new(Processes::new());
    /// let session = Session::spawn(&spawner, &[], options, move |event| drop(tx.send(event)))?;
    /// while !matches!(rx.recv()?, Event::Exited(_)) {}
    /// assert_eq!(session.visible_text()[0].trim_end(), "hello");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn spawn(
        spawner: &Arc<dyn Spawner>,
        env: &[(String, String)],
        options: Options,
        listener: impl Fn(Event) + Send + Sync + 'static,
    ) -> io::Result<Session> {
        let poller = Arc::new(Poller::new()?);
        let pty = pty::spawn(&launch(&options, env), &options.size.winsize())?;
        let pid = pty.pid();
        let master = pty.file().try_clone()?;
        let hang_up_master = pty.file().try_clone()?;
        let mut terminal = term_vt::Terminal::new(options.size.grid(), options.scrollback.min(MAX_SCROLLBACK));
        let product = term_base::identity();
        terminal.set_identity(&product.name, &product.version);
        terminal.allow_clipboard_read(options.allow_clipboard_read);
        terminal.set_base_colors(&options.palette.base_colors());
        let shared = Arc::new(Shared {
            own: FairMutex::new(crate::own::Own::new(terminal)),
            palette: Mutex::new(options.palette.clone()),
            size: Mutex::new(options.size),
            clipboard_reply: Mutex::new(None),
            redraw_pending: AtomicBool::new(false),
            full_redraw: AtomicBool::new(true),
            running: AtomicBool::new(true),
            closed: AtomicBool::new(false),
            listener: Box::new(listener),
            publisher: crate::frames::Publisher::default(),
        });
        let process = spawner.register(pid, &label(&options.program));
        let shell = i32::try_from(pid).unwrap_or(0);
        let grace = options.close_grace;
        spawner.on_stop(process, Box::new(move || hang_up(&hang_up_master, shell, grace)));
        let (input, inbox) = channel();
        let reader = reader::Reader::new(pty, Arc::clone(&shared), inbox, Arc::clone(&poller));
        restored::replay(&shared.own, &options.restored);
        let thread = reader.start(Arc::clone(spawner), process);
        if let Err(error) = thread {
            spawner.stop(process, grace);
            return Err(error);
        }
        Ok(Session {
            shared,
            input,
            poller,
            master,
            pid,
            process,
            spawner: Arc::clone(spawner),
            close_grace: grace,
            closed: AtomicBool::new(false),
            lock_times: LockClock::default(),
        })
    }

    /// The process number of the program, which leads the terminal's session.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Whether the program still runs.
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Acquire)
    }

    /// Sends `bytes` to the program as typed input.
    pub fn write(&self, bytes: impl Into<Vec<u8>>) {
        let bytes = bytes.into();
        if bytes.is_empty() {
            return;
        }
        if self.input.send(bytes).is_ok() {
            let _ = self.poller.notify();
        }
    }

    /// Sends a key and returns whether it produced any input; typing scrolls back to the live screen.
    pub fn key(&self, key: &Key) -> bool {
        let bytes = encode_key(key, &self.input_modes());
        if bytes.is_empty() {
            return false;
        }
        self.scroll_to_bottom();
        self.write(bytes);
        true
    }

    /// Pastes `text`, in bracketed-paste markers when the program asked for them.
    pub fn paste(&self, text: &str) {
        let bytes = encode_paste(text, &self.input_modes());
        self.scroll_to_bottom();
        self.write(bytes);
    }

    /// Reports a mouse event if the program asked for it; `false` means the view handles it, such as for a selection.
    pub fn mouse(&self, event: &MouseEvent) -> bool {
        match encode_mouse(event, &self.input_modes()) {
            Some(report) => {
                self.write(report);
                true
            }
            None => false,
        }
    }

    /// Turns the wheel by `lines` (positive away from the user) over `point`.
    pub fn wheel(&self, lines: i32, point: Point, modifiers: Modifiers) {
        match encode_wheel(lines, point.line, point.column, modifiers, &self.input_modes()) {
            Wheel::Send(bytes) => self.write(bytes),
            Wheel::Scroll(lines) => self.scroll(Scroll::Lines(lines)),
        }
    }

    /// Tells the session whether its view has the keyboard focus.
    pub fn focus(&self, focused: bool) {
        self.shared.own.lock().focused = focused;
        self.shared.full_redraw.store(true, Ordering::Release);
        if let Some(report) = encode_focus(focused, &self.input_modes()) {
            self.write(report);
        }
    }

    /// Changes the size; the program hears of it through SIGWINCH.
    pub fn resize(&self, size: Size) {
        *self.shared.size.lock_ignoring_poison() = size;
        self.shared.own.lock().terminal.resize(size.grid());
        let _ = sys::set_window_size(&self.master, &size.winsize());
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// Has the renderer on another thread draw the session's frames (E5): `sink` hears on the reader thread when a
    /// new one waits in the returned source. The reader thread builds a first frame at once.
    pub fn render_to(&self, sink: Arc<dyn crate::frames::FrameSink>) -> crate::frames::FrameSource {
        self.shared.publisher.set_sink(Some(sink));
        self.shared.full_redraw.store(true, Ordering::Release);
        self.shared.publisher.publish(&self.shared);
        crate::frames::FrameSource {
            shared: Arc::clone(&self.shared),
            poller: Arc::clone(&self.poller),
        }
    }

    /// The latest frame the reader thread built, for what the owner reads of the screen between frames; with no
    /// renderer, none.
    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        self.shared.publisher.latest()
    }

    /// Re-arms `Event::Redraw` without a snapshot, for an owner whose renderer takes the frames.
    pub fn rearm(&self) {
        self.shared.redraw_pending.store(false, Ordering::Release);
    }

    /// Builds a new frame for the renderer now, after the owner changed what shows: scrolled, selected, resized or
    /// recoloured.
    pub fn republish(&self) {
        if self.shared.publisher.has_sink() {
            self.shared.publisher.publish(&self.shared);
        }
    }

    /// The visible screen; also re-arms `Event::Redraw`.
    pub fn snapshot(&self) -> Snapshot {
        self.shared.redraw_pending.store(false, Ordering::Release);
        let palette = self.shared.palette.lock_ignoring_poison().clone();
        let asked = Instant::now();
        let own = self.shared.own.lock();
        let locked = Instant::now();
        let mut snapshot = own.snapshot(&palette);
        drop(own);
        self.lock_times.note(locked - asked, locked.elapsed());
        if self.shared.full_redraw.swap(false, Ordering::AcqRel) {
            snapshot.damage = snapshot::Damage::Full;
        }
        snapshot
    }

    /// The longest `snapshot` waited for the screen's lock and held it since the last call, which starts them anew.
    pub fn take_lock_times(&self) -> LockTimes {
        self.lock_times.take()
    }

    /// The visible lines as plain text, for VoiceOver.
    pub fn visible_text(&self) -> Vec<String> {
        self.shared.own.lock().visible_text()
    }

    /// Scrolls through the history.
    pub fn scroll(&self, scroll: Scroll) {
        self.shared.own.lock().scroll(scroll);
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    fn scroll_to_bottom(&self) {
        let scrolled = {
            let mut own = self.shared.own.lock();
            let grid = own.terminal.grid_mut();
            let scrolled = grid.display_offset != 0;
            grid.display_offset = 0;
            scrolled
        };
        if scrolled {
            self.shared.full_redraw.store(true, Ordering::Release);
        }
    }

    /// Clears the history and, with `above_cursor`, the lines above the cursor's, which moves that line to the top as a
    /// terminal's Clear command does; the program is not told.
    pub fn clear(&self, above_cursor: bool) {
        {
            let mut own = self.shared.own.lock();
            own.terminal.clear_for_user(above_cursor);
            own.selection = None;
        }
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// Starts a selection at `point`.
    pub fn select(&self, kind: SelectionKind, point: Point, side: CellSide) {
        self.shared.own.lock().select(kind, point, side);
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// Moves the end of the selection to `point`.
    pub fn extend_selection(&self, point: Point, side: CellSide) {
        self.shared.own.lock().extend_selection(point, side);
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// Removes the selection.
    pub fn clear_selection(&self) {
        self.shared.own.lock().selection = None;
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// Scrolls to the prompt `step` prompts from the one at the view's top (F80); whether the view moved.
    pub fn jump_to_prompt(&self, step: isize) -> bool {
        let moved = self.shared.own.lock().terminal.jump_to_prompt(step);
        if moved {
            self.shared.full_redraw.store(true, Ordering::Release);
        }
        moved
    }

    /// Searches the screen and history for `pattern` and shows the next match, or the previous one (F104); an
    /// error for an invalid expression.
    pub fn find(&self, pattern: &str, options: term_vt::search::Options, forward: bool) -> Result<Found, String> {
        let found = self.shared.own.lock().find(pattern, options, forward);
        self.shared.full_redraw.store(true, Ordering::Release);
        found
    }

    /// Ends the search and its highlight.
    pub fn end_find(&self) {
        self.shared.own.lock().end_find();
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// The selected text.
    pub fn selected_text(&self) -> Option<String> {
        self.shared.own.lock().selected_text()
    }

    /// The input modes the program has switched on.
    pub fn input_modes(&self) -> InputModes {
        self.shared.own.lock().input_modes()
    }

    /// Changes the colours.
    pub fn set_palette(&self, palette: Palette) {
        self.shared.own.lock().terminal.set_base_colors(&palette.base_colors());
        *self.shared.palette.lock_ignoring_poison() = palette;
        self.shared.full_redraw.store(true, Ordering::Release);
    }

    /// What runs in front: an agent, a remote session, the shell or another program.
    pub fn foreground(&self) -> Foreground {
        foreground::of_terminal(&self.master)
    }

    /// Whether the program waits for a line it does not echo, such as a password.
    pub fn reads_password(&self) -> bool {
        sys::reads_hidden_line(&self.master)
    }

    /// Pastes the PNG `png` as `paste::paste_image` decides, storing it in `dir` when its path is pasted.
    pub fn paste_image(&self, png: &[u8], dir: &Path) -> io::Result<PasteAction> {
        let front = self.foreground();
        let action = paste::paste_image(&front, png, dir)?;
        match &action {
            PasteAction::SendCtrlV => {
                let ctrl_v = Key {
                    code: KeyCode::Char('v'),
                    modifiers: Modifiers {
                        control: true,
                        ..Modifiers::default()
                    },
                    text: Some("v".into()),
                    keypad: false,
                    action: crate::input::KeyAction::Press,
                };
                self.key(&ctrl_v);
            }
            PasteAction::InsertPath(path) => {
                self.paste(&paste::quote_path(path, QuoteStyle::for_foreground(&front)));
            }
            PasteAction::Refuse(_) => {}
        }
        Ok(action)
    }

    /// Pastes the quoted paths of files dropped onto the terminal.
    pub fn paste_paths(&self, paths: &[PathBuf]) {
        let style = QuoteStyle::for_foreground(&self.foreground());
        self.paste(&paste::quote_paths(paths, style));
    }

    /// Answers the program's last request to read the clipboard (`Event::ClipboardLoad`) with `text`.
    pub fn answer_clipboard(&self, text: &str) {
        let reply = self.shared.clipboard_reply.lock_ignoring_poison().take();
        if let Some(reply) = reply {
            self.write(reply(text));
        }
    }

    /// Ends the program and everything it started in the terminal, without waiting: hang-up, then SIGTERM, then
    /// SIGKILL, each after `Options::close_grace`. No events follow.
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.shared.closed.store(true, Ordering::Release);
        self.spawner.stop(self.process, self.close_grace);
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

fn launch(options: &Options, env: &[(String, String)]) -> pty::Launch {
    let (program, args, extra) = match &options.program {
        Program::LoginShell(shell) => {
            let scripts = options
                .shell_integration
                .as_deref()
                .filter(|dir| integration::install(dir).is_ok());
            let launch = integration::launch(shell, scripts, options.ssh_integration, env);
            (shell.clone(), launch.args, launch.env)
        }
        Program::Command { program, args } => (program.clone(), args.clone(), Vec::new()),
        Program::Ssh(args) => {
            let scripts = options
                .shell_integration
                .as_deref()
                .filter(|dir| options.ssh_integration && integration::install(dir).is_ok());
            let (program, args) = integration::ssh(scripts, args);
            (program, args, Vec::new())
        }
    };
    let mut vars: HashMap<String, String> = env.iter().cloned().collect();
    vars.extend(extra);
    vars.insert("TERM".into(), "xterm-256color".into());
    vars.insert("COLORTERM".into(), "truecolor".into());
    let product = term_base::identity();
    vars.insert("TERM_PROGRAM".into(), product.name.clone());
    vars.insert("TERM_PROGRAM_VERSION".into(), product.version.clone());
    let has_locale = ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .any(|name| vars.get(*name).is_some_and(|value| !value.is_empty()));
    if !has_locale {
        // Without a UTF-8 character type shells treat every non-ASCII byte as a character of its own.
        vars.insert("LC_CTYPE".into(), "UTF-8".into());
    }
    pty::Launch {
        program,
        args,
        cwd: options.cwd.clone(),
        env: vars,
    }
}

fn label(program: &Program) -> String {
    let path = match program {
        Program::LoginShell(path) | Program::Command { program: path, .. } => path,
        Program::Ssh(_) => return "ssh".into(),
    };
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), |name| name.to_string_lossy())
        .into_owned()
}

/// What closing a terminal window does: SIGHUP to the shell and to the job in front, which lives in a process group
/// of its own and so is beyond the spawner's SIGTERM and SIGKILL for the shell's group.
fn hang_up(master: &File, shell: i32, grace: Duration) {
    let front = sys::foreground_group(master).filter(|&group| group != shell);
    sys::signal_group(shell, libc::SIGHUP);
    let Some(front) = front else { return };
    sys::signal_group(front, libc::SIGHUP);
    term_base::thread::detach_or_run("terminal-hang-up", move || {
        for signal in [libc::SIGTERM, libc::SIGKILL] {
            if term_base::system::group_ends_within(front, grace) {
                return;
            }
            sys::signal_group(front, signal);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixel_sizes_stay_within_what_the_pty_can_hold() {
        let huge = Size {
            columns: 1000,
            lines: 500,
            cell_width: 200,
            cell_height: 300,
        };
        let window = huge.winsize();
        assert_eq!((window.ws_col, window.ws_row), (1000, 500));
        assert_eq!((window.ws_xpixel, window.ws_ypixel), (1000 * 65, 500 * 131));
        let tiny = Size {
            columns: 0,
            lines: 0,
            cell_width: 8,
            cell_height: 16,
        };
        assert_eq!((tiny.winsize().ws_col, tiny.winsize().ws_row), (2, 1));
    }

    #[test]
    fn the_program_gets_the_terminals_variables_on_top_of_the_users() {
        let options = Options {
            program: Program::Command {
                program: "/bin/echo".into(),
                args: vec!["x".into()],
            },
            ..Options::new("/tmp")
        };
        let env = [
            ("PATH".to_string(), "/u/bin".to_string()),
            ("TERM".to_string(), "dumb".to_string()),
        ];
        let pty = launch(&options, &env);
        assert_eq!(pty.env["PATH"], "/u/bin");
        assert_eq!(pty.env["TERM"], "xterm-256color");
        assert_eq!(pty.env["COLORTERM"], "truecolor");
        assert_eq!(pty.env["TERM_PROGRAM"], term_base::identity().name);
        assert_eq!(pty.env["LC_CTYPE"], "UTF-8");
        let with_lang = [("LANG".to_string(), "de_DE.UTF-8".to_string())];
        assert!(!launch(&options, &with_lang).env.contains_key("LC_CTYPE"));
        assert_eq!(label(&options.program), "echo");
    }

    #[test]
    fn lock_times_keep_the_longest_until_taken() {
        let clock = LockClock::default();
        let ms = Duration::from_millis;
        clock.note(ms(3), ms(1));
        clock.note(ms(1), ms(7));
        assert_eq!(
            clock.take(),
            LockTimes {
                wait: ms(3),
                hold: ms(7)
            }
        );
        assert_eq!(clock.take(), LockTimes::default());
    }
}
