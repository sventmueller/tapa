//! Frames for a renderer on a thread of its own (E5): the reader thread, which owns the screen, builds an immutable
//! snapshot whenever output changed it and the renderer has taken the one before, and tells the renderer's sink. While
//! the renderer has not taken a frame, output only marks the screen changed; taking a frame then asks the reader
//! thread for the next. So a keystroke's echo is built at once and drawn at the next display frame, and a flood of
//! output costs one snapshot per display frame, not one per read.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use polling::Poller;
use term_base::LockIgnoringPoison;

use crate::session::Shared;
use crate::snapshot::Snapshot;

/// Who draws a session's frames: told on the reader thread that a new one waits.
pub trait FrameSink: Send + Sync {
    /// A new frame waits in the session's `FrameSource`.
    fn frame_ready(&self);
}

/// The latest frame and whether the renderer has it already.
#[derive(Default)]
pub(crate) struct Frames {
    latest: Option<Arc<Snapshot>>,
    taken: bool,
    sink: Option<Arc<dyn FrameSink>>,
    generation: u64,
}

/// What the session shares with its frames' renderer.
#[derive(Default)]
pub(crate) struct Publisher {
    frames: Mutex<Frames>,
    /// Output changed the screen since the latest frame was built.
    dirty: AtomicBool,
    /// The renderer took a frame while the screen was dirty; the reader thread builds the next when it wakes.
    wanted: AtomicBool,
}

impl Publisher {
    pub(crate) fn has_sink(&self) -> bool {
        self.frames.lock_ignoring_poison().sink.is_some()
    }

    pub(crate) fn set_sink(&self, sink: Option<Arc<dyn FrameSink>>) -> u64 {
        let mut frames = self.frames.lock_ignoring_poison();
        frames.generation += 1;
        frames.latest = None;
        frames.taken = false;
        frames.sink = sink;
        frames.generation
    }

    /// The latest frame, built or not taken yet.
    pub(crate) fn latest(&self) -> Option<Arc<Snapshot>> {
        self.frames.lock_ignoring_poison().latest.clone()
    }

    /// Output changed the screen: builds a frame at once while the renderer has the one before, else waits for it.
    pub(crate) fn changed(&self, shared: &Shared) {
        self.dirty.store(true, Ordering::Release);
        let waiting = {
            let frames = self.frames.lock_ignoring_poison();
            frames.sink.is_none() || (frames.latest.is_some() && !frames.taken)
        };
        if !waiting {
            self.publish(shared);
        }
    }

    /// Builds a frame from the screen as it is now and tells the sink.
    pub(crate) fn publish(&self, shared: &Shared) {
        let generation = {
            let frames = self.frames.lock_ignoring_poison();
            if frames.sink.is_none() {
                return;
            }
            self.dirty.store(false, Ordering::Release);
            frames.generation
        };
        let snapshot = Arc::new(shared.build_snapshot());
        let sink = {
            let mut frames = self.frames.lock_ignoring_poison();
            if frames.generation != generation {
                return;
            }
            frames.latest = Some(snapshot);
            frames.taken = false;
            frames.sink.clone()
        };
        if let Some(sink) = sink {
            sink.frame_ready();
        }
    }

    /// Whether the renderer asked for the next frame; once.
    pub(crate) fn take_wanted(&self) -> bool {
        self.wanted.swap(false, Ordering::AcqRel)
    }
}

/// Where a renderer takes a session's frames from, on any thread.
#[derive(Clone)]
pub struct FrameSource {
    pub(crate) shared: Arc<Shared>,
    pub(crate) poller: Arc<Poller>,
    pub(crate) generation: u64,
}

impl FrameSource {
    /// The frame not taken yet, if any; asks the reader thread for the next one when output came after it.
    pub fn take(&self) -> Option<Arc<Snapshot>> {
        let publisher = &self.shared.publisher;
        let frame = {
            let mut frames = publisher.frames.lock_ignoring_poison();
            if frames.generation != self.generation {
                return None;
            }
            if frames.taken {
                None
            } else {
                frames.taken = true;
                frames.latest.clone()
            }
        };
        if publisher.dirty.load(Ordering::Acquire) {
            publisher.wanted.store(true, Ordering::Release);
            let _ = self.poller.notify();
        }
        frame
    }

    /// The latest frame, taken or not, for drawing again what did not change.
    pub fn latest(&self) -> Option<Arc<Snapshot>> {
        let frames = self.shared.publisher.frames.lock_ignoring_poison();
        if frames.generation == self.generation {
            frames.latest.clone()
        } else {
            None
        }
    }
}

impl std::fmt::Debug for FrameSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameSource").finish_non_exhaustive()
    }
}
