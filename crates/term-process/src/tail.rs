//! The latest output of a process, kept within a limit however much it writes.

use std::collections::VecDeque;
use std::io::Read;
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(test)]
use std::time::Instant;
use term_base::LockIgnoringPoison;

/// The last `limit` bytes a process wrote.
#[derive(Debug, Clone)]
pub struct Tail {
    limit: usize,
    bytes: VecDeque<u8>,
    dropped: u64,
}

impl Tail {
    /// An empty tail that keeps up to `limit` bytes.
    pub fn new(limit: usize) -> Tail {
        Tail {
            limit,
            bytes: VecDeque::new(),
            dropped: 0,
        }
    }

    /// Adds `data`, dropping the oldest bytes beyond the limit.
    pub fn push(&mut self, data: &[u8]) {
        let data = if data.len() > self.limit {
            self.dropped += (data.len() - self.limit) as u64;
            &data[data.len() - self.limit..]
        } else {
            data
        };
        let overflow = (self.bytes.len() + data.len()).saturating_sub(self.limit);
        self.bytes.drain(..overflow);
        self.dropped += overflow as u64;
        self.bytes.extend(data);
    }

    /// How many bytes were dropped so far.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// The kept output as text; after a drop it starts at the next full line.
    pub fn text(&self) -> String {
        let bytes: Vec<u8> = self.bytes.iter().copied().collect();
        let start = if self.dropped > 0 {
            bytes.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1)
        } else {
            0
        };
        String::from_utf8_lossy(&bytes[start..]).into_owned()
    }
}

/// A reader being collected into a tail on its own thread.
pub struct Collecting {
    done: Receiver<()>,
}

impl Collecting {
    /// Waits until the reader reached its end or `limit` passed; whether it reached its end. A grandchild that
    /// keeps the pipe open must not hold its owner, so the wait is always bounded.
    pub fn wait(&self, limit: Duration) -> bool {
        !matches!(self.done.recv_timeout(limit), Err(RecvTimeoutError::Timeout))
    }

    /// Waits for all of `readers` together, up to `limit` in all; whether every one reached its end.
    #[cfg(test)]
    pub(crate) fn wait_all<'a>(readers: impl IntoIterator<Item = &'a Collecting>, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        readers
            .into_iter()
            .all(|reader| reader.wait(deadline.saturating_duration_since(Instant::now())))
    }
}

/// Reads `reader` to its end into `tail` on a thread of its own.
pub fn collect(mut reader: impl Read + Send + 'static, tail: Arc<Mutex<Tail>>) -> std::io::Result<Collecting> {
    let (finished, done) = channel();
    std::thread::Builder::new().name("tail".into()).spawn(move || {
        term_base::priority::set_qos(term_base::priority::Qos::Utility);
        let mut buffer = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buffer) {
            if n == 0 {
                break;
            }
            tail.lock_ignoring_poison().push(&buffer[..n]);
        }
        let _ = finished.send(());
    })?;
    Ok(Collecting { done })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_keeps_the_latest_bytes_and_starts_at_a_line() {
        let mut tail = Tail::new(10);
        tail.push(b"first\nsecond\nthird");
        assert_eq!(tail.dropped(), 8);
        assert_eq!(tail.text(), "third");
        let mut small = Tail::new(64);
        small.push(b"a\n");
        small.push(b"b\n");
        assert_eq!(small.text(), "a\nb\n");
        assert_eq!(small.dropped(), 0);
    }

    #[test]
    fn a_reader_is_collected_to_its_end() {
        let tail = Arc::new(Mutex::new(Tail::new(4)));
        assert!(
            collect(&b"0123456789"[..], tail.clone())
                .unwrap()
                .wait(Duration::from_secs(5))
        );
        assert_eq!(tail.lock().unwrap().dropped(), 6);
    }

    #[test]
    fn waiting_for_a_reader_that_stays_open_ends_at_the_limit() {
        let (reader, _writer) = std::io::pipe().unwrap();
        let tail = Arc::new(Mutex::new(Tail::new(4)));
        let collecting = collect(reader, tail).unwrap();
        let start = Instant::now();
        assert!(!Collecting::wait_all([&collecting], Duration::from_millis(50)));
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}
