//! The reader thread: the gather stage's batches of PTY output into the emulator and the scanner, typed input and the
//! emulator's replies out.
//!
//! Every batch also goes through `Scanner`, which needs the raw bytes for OSC 7, OSC 133 and server addresses. The
//! thread reserves the screen for a batch and parses at most `MAX_LOCKED_READ` bytes at a time under the lock. Reading
//! the PTY is the gather stage's (`gather.rs`), so the program writes on while a batch is parsed.

use std::collections::VecDeque;
use std::io::{ErrorKind, Write};
use std::num::NonZeroUsize;
use std::os::unix::process::ExitStatusExt;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::mpsc::Receiver;

use std::time::Instant;
use term_base::LockIgnoringPoison;

use polling::{Event as Interest, Events, Poller};
use term_base::{Ending, ProcessId, Spawner};

use crate::event::{Clipboard, Event};
use crate::fair::FairMutex;
use crate::gather::{Batch, Gather};
use crate::pty::Pty;
use crate::session::Shared;
use term_vt::Scanner;

/// The keys the poller reports the PTY and the program's end under.
const PTY_KEY: usize = 0;
const CHILD_KEY: usize = 1;

/// How long the reader waits for the program's last output after it ended.
const LAST_WORDS: std::time::Duration = std::time::Duration::from_millis(200);

/// The longest a synchronized update (mode 2026) holds the screen back.
const SYNC_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(150);

/// The longest output the program is still writing waits to be shown, so a screen it redraws does not show half done.
const UNSETTLED: std::time::Duration = std::time::Duration::from_millis(8);

/// How long the program has to be quiet before the history is packed (F8).
const IDLE: std::time::Duration = std::time::Duration::from_secs(1);

/// How many history rows one step packs; about a millisecond under the lock, so a keystroke's echo barely waits.
const PACK_STEP: usize = 2000;

/// Bytes parsed under one hold of the lock.
const MAX_LOCKED_READ: usize = u16::MAX as usize;

/// Writes waiting for the PTY beyond which the emulator's answers to the program are dropped: a program that leaves
/// this many unread does not read its input, and output asking for answers would otherwise grow the queue without end.
const MAX_QUEUED_WRITES: usize = 4096;

/// The reader thread's state.
pub(crate) struct Reader {
    pty: Pty,
    shared: Arc<Shared>,
    inbox: Receiver<Vec<u8>>,
    poller: Arc<Poller>,
    scanner: Scanner,
    pending: VecDeque<Vec<u8>>,
    written: usize,
    /// Since when the program has been quiet, while there may be history left to pack.
    quiet_since: Option<Instant>,
    /// When parsed output that came amid a burst is shown at the latest.
    show_by: Option<Instant>,
    gather: Option<Gather>,
}

impl Reader {
    pub(crate) fn new(pty: Pty, shared: Arc<Shared>, inbox: Receiver<Vec<u8>>, poller: Arc<Poller>) -> Reader {
        Reader {
            pty,
            shared,
            inbox,
            poller,
            scanner: Scanner::new(),
            pending: VecDeque::new(),
            written: 0,
            quiet_since: None,
            show_by: None,
            gather: None,
        }
    }

    /// Runs the reader on a thread of its own until the program ends, then reports that and forgets it.
    pub(crate) fn start(self, spawner: Arc<dyn Spawner>, process: ProcessId) -> std::io::Result<()> {
        std::thread::Builder::new()
            .name("terminal-reader".into())
            .spawn(move || {
                term_base::priority::set_qos(term_base::priority::Qos::UserInitiated);
                let mut reader = self;
                let status = reader.serve();
                reader.finish(status, spawner.as_ref(), process);
            })
            .map(drop)
    }

    fn serve(&mut self) -> Option<ExitStatus> {
        let mut interest = Interest::none(PTY_KEY);
        let mut ended_early = match self.pty.register(&self.poller, interest, CHILD_KEY) {
            Ok(watched) => !watched,
            Err(_) => {
                self.pty.deregister(&self.poller);
                return None;
            }
        };
        self.gather = Gather::start(self.pty.file(), Arc::clone(&self.poller)).ok();
        self.gather.as_ref()?;
        let mut events = Events::with_capacity(NonZeroUsize::MIN.saturating_add(15));
        let mut registered = (interest.readable, interest.writable);
        let status = loop {
            let deadline = self.sync_deadline();
            let packing = self.quiet_since.map(|since| since + IDLE);
            let wake_at = [deadline, packing, self.show_by].into_iter().flatten().min();
            let mut timeout = wake_at.map(|at| at.saturating_duration_since(Instant::now()));
            if self.gather.as_ref().is_some_and(|gather| !gather.may_sleep()) {
                timeout = Some(std::time::Duration::ZERO);
            }
            events.clear();
            let waited = self.poller.wait(&mut events, timeout);
            if let Some(gather) = &self.gather {
                gather.awake();
            }
            if let Err(error) = waited {
                if error.kind() == ErrorKind::Interrupted {
                    continue;
                }
                break None;
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                self.end_synchronized_update();
            }
            if self.show_by.is_some_and(|at| Instant::now() >= at) {
                self.show_by = None;
                self.shared.wake();
            }
            if packing.is_some_and(|at| Instant::now() >= at) {
                self.pack_history();
            }
            if self.shared.publisher.take_wanted() {
                self.shared.publisher.publish(&self.shared);
            }
            while let Ok(bytes) = self.inbox.try_recv() {
                self.pending.push_back(bytes);
            }
            let mut exited = None;
            if std::mem::take(&mut ended_early) || events.iter().any(|event| event.key == CHILD_KEY) {
                exited = self.pty.exit_status().map(Some);
            }
            self.take_batches();
            if let Some(status) = exited {
                // The program's last words may still be in the PTY: the stage hands them over until it is closed.
                self.take_last_words();
                break status;
            }
            self.flush();
            interest.writable = !self.pending.is_empty();
            if (interest.readable, interest.writable) != registered {
                registered = (interest.readable, interest.writable);
                if self.pty.reregister(&self.poller, interest).is_err() {
                    break None;
                }
            }
        };
        self.pty.deregister(&self.poller);
        status
    }

    /// Parses every batch the gather stage has ready.
    fn take_batches(&mut self) {
        while let Some(batch) = self.gather.as_ref().and_then(Gather::try_next) {
            match batch {
                Batch::Bytes { bytes, settled } => {
                    self.consume(&bytes, settled);
                    if let Some(gather) = &self.gather {
                        gather.give_back(bytes);
                    }
                }
                Batch::Closed => return,
            }
        }
    }

    /// Parses what the gather stage still drains after the program ended, until the PTY closes or stays quiet.
    fn take_last_words(&mut self) {
        while let Some(batch) = self.gather.as_ref().and_then(|gather| gather.next_within(LAST_WORDS)) {
            match batch {
                Batch::Bytes { bytes, settled } => {
                    self.consume(&bytes, settled);
                    if let Some(gather) = &self.gather {
                        gather.give_back(bytes);
                    }
                }
                Batch::Closed => return,
            }
        }
    }

    fn finish(mut self, status: Option<ExitStatus>, spawner: &dyn Spawner, process: ProcessId) {
        let ending = match status {
            Some(status) => match (status.code(), status.signal()) {
                (Some(code), _) => Ending::Code(code),
                (None, Some(signal)) => Ending::Signal(signal),
                (None, None) => Ending::Unknown,
            },
            None => Ending::Unknown,
        };
        if let Some(gather) = self.gather.take() {
            gather.stop();
        }
        self.shared.running.store(false, std::sync::atomic::Ordering::Release);
        self.shared.wake();
        self.shared.emit(Event::Exited(ending));
        // Dropping the PTY hangs up and waits for the program, which has ended already unless reading failed.
        drop(self.pty);
        spawner.forget(process);
    }

    /// Parses a batch of output into the screen and the scanner, and shows it once the program paused or
    /// `UNSETTLED` passed.
    fn consume(&mut self, bytes: &[u8], settled: bool) {
        let shared = Arc::clone(&self.shared);
        parse_locked(&shared.own, bytes, &mut self.scanner);
        if !bytes.is_empty() {
            self.quiet_since = Some(Instant::now());
            let show = {
                let mut own = shared.own.lock();
                if let Some(gather) = &self.gather {
                    gather.set_full_screen(own.terminal.modes().contains(term_vt::term::Modes::ALT_SCREEN));
                }
                if own.terminal.modes().contains(term_vt::term::Modes::SYNC) {
                    let since = *own.sync_since.get_or_insert_with(Instant::now);
                    since.elapsed() >= SYNC_TIMEOUT
                } else {
                    own.sync_since = None;
                    true
                }
            };
            if show && settled {
                self.show_by = None;
                self.shared.wake();
            } else if show {
                self.show_by.get_or_insert_with(|| Instant::now() + UNSETTLED);
            }
            self.dispatch();
        }
    }

    /// Packs a step of the history; once nothing is left, waits for the program's next output.
    fn pack_history(&mut self) {
        if self.shared.own.lock().terminal.pack_history(PACK_STEP) > 0 {
            return;
        }
        self.quiet_since = None;
    }

    /// When a synchronized update must show even though the program has not ended it.
    fn sync_deadline(&self) -> Option<Instant> {
        self.shared.own.lock().sync_since.map(|since| since + SYNC_TIMEOUT)
    }

    fn end_synchronized_update(&mut self) {
        self.shared.own.lock().sync_since = None;
        self.shared.wake();
        self.dispatch();
    }

    /// Acts on what the emulator and the scanner reported while the screen was locked.
    fn dispatch(&mut self) {
        let reported = self.shared.own.lock().terminal.take_events();
        for event in coalesce(reported) {
            self.on_own_event(event);
        }
        for mark in self.scanner.take_events() {
            self.shared.emit(mark.into());
        }
    }

    fn on_own_event(&mut self, event: term_vt::Event) {
        match event {
            term_vt::Event::Reply(bytes) => self.answer(bytes),
            term_vt::Event::Title(title) => self.shared.emit(Event::Title(title)),
            term_vt::Event::Bell => self.shared.emit(Event::Bell),
            term_vt::Event::ClipboardStore(target, text) => self.shared.emit(Event::ClipboardStore {
                clipboard: clipboard(target),
                text,
            }),
            term_vt::Event::ClipboardLoad(target, terminator) => {
                let reply: crate::session::ClipboardReply = Arc::new(move |text: &str| {
                    format!("\x1b]52;{};{}{terminator}", char::from(target), base64(text.as_bytes()))
                });
                *self.shared.clipboard_reply.lock_ignoring_poison() = Some(reply);
                self.shared.emit(Event::ClipboardLoad(clipboard(target)));
            }
            term_vt::Event::TextAreaSizeRequest => {
                let (width, height) = self.shared.size.lock_ignoring_poison().pixels();
                self.answer(format!("\x1b[4;{height};{width}t").into_bytes());
            }
        }
    }

    /// Queues the emulator's answer to the program.
    fn answer(&mut self, bytes: Vec<u8>) {
        if self.pending.len() < MAX_QUEUED_WRITES {
            self.pending.push_back(bytes);
        }
    }

    /// Writes as much pending input as the PTY takes without blocking.
    fn flush(&mut self) {
        while let Some(front) = self.pending.front() {
            match self.pty.file().write(&front[self.written..]) {
                Ok(0) => break,
                Ok(count) => {
                    self.written += count;
                    if self.written >= front.len() {
                        self.pending.pop_front();
                        self.written = 0;
                    }
                }
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(_) => {
                    self.pending.clear();
                    self.written = 0;
                }
            }
        }
    }
}

fn clipboard(target: u8) -> Clipboard {
    if target == b'c' {
        Clipboard::Clipboard
    } else {
        Clipboard::Selection
    }
}

/// What the emulator reported while parsing one read of the output, without what a later report of the same read
/// makes moot: the bells after the first, every title but the last, and every write to a clipboard but the last one
/// to it unless the program also asked to read one. Output that repeats these would otherwise send the main thread
/// one event apiece.
fn coalesce(reported: Vec<term_vt::Event>) -> Vec<term_vt::Event> {
    use term_vt::Event as TermEvent;
    let reads = reported
        .iter()
        .any(|event| matches!(event, TermEvent::ClipboardLoad(..)));
    let mut later_title = false;
    let mut later_stores: Vec<Clipboard> = Vec::new();
    let mut kept = Vec::with_capacity(reported.len());
    for event in reported.into_iter().rev() {
        let moot = match &event {
            TermEvent::Title(_) => std::mem::replace(&mut later_title, true),
            TermEvent::ClipboardStore(target, _) if !reads => {
                let target = clipboard(*target);
                let seen = later_stores.contains(&target);
                if !seen {
                    later_stores.push(target);
                }
                seen
            }
            _ => false,
        };
        if !moot {
            kept.push(event);
        }
    }
    kept.reverse();
    let mut rung = false;
    kept.retain(|event| !matches!(event, TermEvent::Bell) || !std::mem::replace(&mut rung, true));
    kept
}

/// `bytes` in standard base64, as OSC 52 carries them.
fn base64(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b)) << (8 * (3 - chunk.len()));
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(char::from(
                    DIGITS[usize::try_from((value >> (18 - 6 * index)) & 63).unwrap_or(0)],
                ));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Parses `bytes` into the screen with it locked and feeds them to `scanner`: the lease first, so the view waits for
/// the batch rather than the other way round.
fn parse_locked(screen: &FairMutex<crate::own::Own>, bytes: &[u8], scanner: &mut Scanner) {
    let lease = screen.lease();
    let mut own = screen.lock_unfair();
    for chunk in bytes.chunks(MAX_LOCKED_READ) {
        own.terminal.advance(chunk);
    }
    drop(own);
    drop(lease);
    scanner.feed(bytes);
}

#[cfg(test)]
mod tests {
    use term_vt::Event as TermEvent;

    use super::*;

    fn names(events: &[TermEvent]) -> Vec<String> {
        events
            .iter()
            .map(|event| match event {
                TermEvent::Title(Some(title)) => format!("title {title}"),
                TermEvent::Title(None) => "reset".to_string(),
                TermEvent::Bell => "bell".to_string(),
                TermEvent::ClipboardStore(target, text) => format!("store {:?} {text}", clipboard(*target)),
                TermEvent::ClipboardLoad(target, _) => format!("load {:?}", clipboard(*target)),
                _ => "other".to_string(),
            })
            .collect()
    }

    fn store(target: u8, text: &str) -> TermEvent {
        TermEvent::ClipboardStore(target, text.to_string())
    }

    #[test]
    fn one_read_keeps_the_first_bell_the_last_title_and_the_last_write_to_each_clipboard() {
        let reported = vec![
            TermEvent::Bell,
            TermEvent::Title(Some("one".into())),
            store(b'c', "a"),
            TermEvent::Reply(b"x".to_vec()),
            TermEvent::Bell,
            store(b's', "s"),
            TermEvent::Title(None),
            TermEvent::Title(Some("two".into())),
            store(b'c', "b"),
            TermEvent::Bell,
        ];
        assert_eq!(
            names(&coalesce(reported)),
            ["bell", "other", "store Selection s", "title two", "store Clipboard b"]
        );
        assert_eq!(names(&coalesce(vec![TermEvent::Title(None)])), ["reset"]);
    }

    #[test]
    fn writes_to_a_clipboard_all_stay_when_the_program_also_reads_one() {
        let reported = vec![
            store(b'c', "a"),
            TermEvent::ClipboardLoad(b'c', "\x07"),
            store(b'c', "b"),
        ];
        assert_eq!(
            names(&coalesce(reported)),
            ["store Clipboard a", "load Clipboard", "store Clipboard b"]
        );
    }
}
