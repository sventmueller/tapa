use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{BRIDGE_WAIT_MS, Batch, Gather};

/// The bytes of one piece and the pieces of one screen, as htop writes them.
const PIECE: usize = 90;
const PIECES: usize = 50;

/// A pipe whose reading end does not block, as the PTY's does not.
fn pipe() -> (File, std::io::PipeWriter) {
    let (reader, writer) = std::io::pipe().expect("a pipe");
    let reader = File::from(std::os::fd::OwnedFd::from(reader));
    crate::pty::set_nonblocking(reader.as_raw_fd()).expect("the pipe does not block");
    (reader, writer)
}

/// The batches `gather` hands over until `total` bytes came, each with whether it counted as settled.
fn batches(gather: &Gather, total: usize) -> Vec<(usize, bool)> {
    let mut seen = Vec::new();
    let mut got = 0;
    while got < total {
        match gather.next_within(Duration::from_secs(5)) {
            Some(Batch::Bytes { bytes, settled }) => {
                got += bytes.len();
                seen.push((bytes.len(), settled));
                gather.give_back(bytes);
            }
            Some(Batch::Closed) | None => break,
        }
    }
    seen
}

/// A screen written as htop writes one: dozens of small pieces a few microseconds apart; when each piece went out.
fn write_in_pieces(mut writer: std::io::PipeWriter) -> Vec<Instant> {
    let piece = [b'x'; PIECE];
    let mut sent = Vec::with_capacity(PIECES);
    for _ in 0..PIECES {
        sent.push(Instant::now());
        writer.write_all(&piece).expect("the pipe takes the piece");
        std::thread::sleep(Duration::from_micros(20));
    }
    sent
}

#[test]
fn a_full_screen_program_writing_in_pieces_shows_only_once_it_paused() {
    let (reader, writer) = pipe();
    let poller = Arc::new(polling::Poller::new().expect("a poller"));
    let gather = Gather::start(&reader, poller).expect("the stage starts");
    gather.set_full_screen(true);
    let sent = std::thread::scope(|scope| scope.spawn(|| write_in_pieces(writer)).join().expect("the writer ends"));
    let seen = batches(&gather, PIECES * PIECE);
    // A writer the scheduler held back longer than the stage waits did pause, and its screen may show there; only a
    // batch that ends amid pieces in quick succession must not count as the end.
    let stall = Duration::from_millis(BRIDGE_WAIT_MS.unsigned_abs().into()) * 4 / 5;
    let mut pieces = 0;
    for (bytes, settled) in &seen[..seen.len() - 1] {
        pieces += bytes / PIECE;
        let paused = sent[pieces] - sent[pieces - 1];
        assert!(
            !settled || paused >= stall,
            "a piece in the middle counted as the end after {paused:?}: {seen:?}"
        );
    }
    // The last pieces may go out unsettled once the stage bridged as long as it may; the writer's end then says so.
    let ended = seen.last().is_some_and(|(_, settled)| *settled)
        || matches!(gather.next_within(Duration::from_secs(5)), Some(Batch::Closed));
    assert!(ended, "the end of the screen was never announced: {seen:?}");
    gather.stop();
}

#[test]
fn output_at_a_prompt_shows_at_once() {
    let (reader, mut writer) = pipe();
    let poller = Arc::new(polling::Poller::new().expect("a poller"));
    let gather = Gather::start(&reader, poller).expect("the stage starts");
    writer.write_all(b"x").expect("the pipe takes the echo");
    let started = std::time::Instant::now();
    let seen = batches(&gather, 1);
    assert_eq!(seen, vec![(1, true)]);
    assert!(started.elapsed() < Duration::from_millis(50), "{:?}", started.elapsed());
    gather.stop();
}
