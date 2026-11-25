//! The gather stage, as Ghostty's `termio/Exec.zig` runs it: a thread of its own drains the PTY into a ring of
//! buffers while the reader thread parses the full ones, so the program writing never waits for the parser. A
//! stream that keeps the kernel queue full leaves short gaps while the program refills it; a few immediate retries
//! bridge those instead of handing over a tiny batch.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

use polling::Poller;

/// Bytes per buffer; Ghostty's size.
const CAPACITY: usize = 64 * 1024;

/// Buffers in the ring; when the parser is that far behind, the stage stops reading and the kernel queue holds the
/// program back.
const BUFFERS: usize = 4;

/// A batch this large belongs to a stream that keeps the kernel queue full, which hands over at most about 1 KiB a
/// read on macOS, so a dry read is bridged; a smaller one is interactive output and goes at once.
const BRIDGE_FROM: usize = 1024;

/// Immediate retries of a dry read before the stage waits in poll. Ghostty measured 8 to 16 on an M4 Max; here 64
/// halved the gap to the kernel's own drain rate, as waking from poll costs more than the writer's refill.
const RETRIES: usize = 64;

/// How long one bridging wait lasts; quiet for that long, the burst is over.
const BRIDGE_WAIT_MS: i32 = 1;

/// The longest a batch bridges gaps before it goes regardless, well under a display frame.
const BRIDGE_BUDGET: Duration = Duration::from_millis(3);

/// How long the stage waits for the PTY before it looks whether it should stop.
const WAIT_MS: i32 = 100;

/// What the gather stage hands over.
pub(crate) enum Batch {
    /// Output, to parse and give back; `settled` when the program paused after it rather than writing on.
    Bytes { bytes: Vec<u8>, settled: bool },
    /// The PTY is closed: the program and everything it started are gone.
    Closed,
}

/// The gather stage of a session, owned by its reader thread.
pub(crate) struct Gather {
    batches: Receiver<Batch>,
    /// Batches handed over and not given back yet: while any, the parser is busy and bridging costs no latency.
    out: Arc<AtomicUsize>,
    /// Batches handed over and not taken yet.
    queued: Arc<AtomicUsize>,
    /// The reader thread is about to wait or waits in its poller, so a batch has to wake it.
    sleeping: Arc<AtomicBool>,
    free: Sender<Vec<u8>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Gather {
    /// Starts draining `pty`, a duplicate of the PTY's descriptor, waking `poller` for each batch.
    pub(crate) fn start(pty: &File, poller: Arc<Poller>) -> std::io::Result<Gather> {
        let file = pty.try_clone()?;
        let (batch_sender, batches) = channel();
        let (free, free_buffers) = channel();
        for _ in 0..BUFFERS {
            let _ = free.send(Vec::with_capacity(CAPACITY));
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let out = Arc::new(AtomicUsize::new(0));
        let shared = Shared {
            out: Arc::clone(&out),
            queued: Arc::new(AtomicUsize::new(0)),
            sleeping: Arc::new(AtomicBool::new(false)),
        };
        let (queued, sleeping) = (Arc::clone(&shared.queued), Arc::clone(&shared.sleeping));
        let thread = std::thread::Builder::new()
            .name("terminal-gather".into())
            .spawn(move || {
                term_base::priority::set_qos(term_base::priority::Qos::UserInitiated);
                drain(file, &batch_sender, &free_buffers, &poller, &stopping, &shared);
            })?;
        Ok(Gather {
            batches,
            out,
            queued,
            sleeping,
            free,
            stop,
            thread: Some(thread),
        })
    }

    /// Tells the stage the reader thread is about to wait; false when a batch waits already, so it should not.
    pub(crate) fn may_sleep(&self) -> bool {
        self.sleeping.store(true, Ordering::SeqCst);
        if self.queued.load(Ordering::SeqCst) > 0 {
            self.sleeping.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }

    /// Tells the stage the reader thread is awake again, taking batches without being woken.
    pub(crate) fn awake(&self) {
        self.sleeping.store(false, Ordering::SeqCst);
    }

    /// The next batch waiting, if any.
    pub(crate) fn try_next(&self) -> Option<Batch> {
        match self.batches.try_recv() {
            Ok(batch) => {
                self.queued.fetch_sub(1, Ordering::SeqCst);
                Some(batch)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Batch::Closed),
        }
    }

    /// The next batch, waiting at most `patience`.
    pub(crate) fn next_within(&self, patience: Duration) -> Option<Batch> {
        let batch = self.batches.recv_timeout(patience).ok()?;
        self.queued.fetch_sub(1, Ordering::SeqCst);
        Some(batch)
    }

    /// Hands a parsed batch's buffer back for the next one.
    pub(crate) fn give_back(&self, mut buffer: Vec<u8>) {
        self.out.fetch_sub(1, Ordering::AcqRel);
        buffer.clear();
        let _ = self.free.send(buffer);
    }

    /// Stops the stage and waits for it, so that its descriptor is closed before the PTY hangs up.
    pub(crate) fn stop(mut self) {
        self.stop.store(true, Ordering::Release);
        // A stage waiting for a free buffer wakes when the senders go.
        drop(std::mem::replace(&mut self.free, channel().0));
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The counters both stages read.
struct Shared {
    out: Arc<AtomicUsize>,
    queued: Arc<AtomicUsize>,
    sleeping: Arc<AtomicBool>,
}

impl Shared {
    /// Hands `batch` over, waking the reader thread only where it waits.
    fn send(&self, batches: &Sender<Batch>, batch: Batch, poller: &Poller) -> bool {
        self.queued.fetch_add(1, Ordering::SeqCst);
        if batches.send(batch).is_err() {
            return false;
        }
        if self.sleeping.swap(false, Ordering::SeqCst) {
            let _ = poller.notify();
        }
        true
    }
}

fn drain(
    mut pty: File,
    batches: &Sender<Batch>,
    free: &Receiver<Vec<u8>>,
    poller: &Poller,
    stop: &AtomicBool,
    shared: &Shared,
) {
    let out = &shared.out;
    let mut held: Option<Vec<u8>> = None;
    // The last batch filled the kernel queue, so the stream is likely still running and worth asking at once.
    let mut hot = false;
    loop {
        let mut buffer = match held.take() {
            Some(buffer) => buffer,
            None => match free.recv() {
                Ok(buffer) => buffer,
                Err(_) => return,
            },
        };
        if !hot && !readable(&pty, stop) {
            return;
        }
        buffer.resize(CAPACITY, 0);
        let mut filled = 0;
        let mut retries = 0;
        let mut bridging: Option<std::time::Instant> = None;
        let mut closed = false;
        let mut settled = false;
        while filled < CAPACITY {
            match pty.read(&mut buffer[filled..]) {
                Ok(0) => {
                    closed = true;
                    break;
                }
                Ok(count) => filled += count,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if filled < BRIDGE_FROM && !(hot && filled == 0) {
                        settled = true;
                        break;
                    }
                    if retries < RETRIES {
                        retries += 1;
                        continue;
                    }
                    // Waiting hides behind the parser only while it works; an idle one wants the batch now.
                    let started = *bridging.get_or_insert_with(std::time::Instant::now);
                    if out.load(Ordering::Acquire) == 0 || started.elapsed() >= BRIDGE_BUDGET {
                        break;
                    }
                    if !crate::sys::wait_readable(&pty, BRIDGE_WAIT_MS) {
                        settled = true;
                        break;
                    }
                }
                // EIO once the other side is gone.
                Err(_) => {
                    closed = true;
                    break;
                }
            }
        }
        buffer.truncate(filled);
        hot = filled >= BRIDGE_FROM;
        if filled == 0 {
            held = Some(buffer);
        } else {
            out.fetch_add(1, Ordering::AcqRel);
            let batch = Batch::Bytes {
                bytes: buffer,
                settled: settled || closed,
            };
            if !shared.send(batches, batch, poller) {
                return;
            }
        }
        if closed {
            shared.send(batches, Batch::Closed, poller);
        }
        if closed || stop.load(Ordering::Acquire) {
            return;
        }
    }
}

/// Waits until `pty` can be read; false once the stage should stop.
fn readable(pty: &File, stop: &AtomicBool) -> bool {
    loop {
        if stop.load(Ordering::Acquire) {
            return false;
        }
        if crate::sys::wait_readable(pty, WAIT_MS) {
            return true;
        }
    }
}
