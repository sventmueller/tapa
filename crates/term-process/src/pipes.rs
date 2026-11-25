//! The standard streams of a started process, served from the thread that waits for it through poll(2), instead of a
//! thread for each stream.

use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::process::{ChildStderr, ChildStdin, ChildStdout};
use std::time::Duration;

/// Which output stream bytes came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A process's pipes: the input still to be written, and the outputs still open.
///
/// ```
/// use std::process::{Command, Stdio};
///
/// use term_process::pipes::{Pipes, Stream};
///
/// let mut child = Command::new("/bin/cat").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn()?;
/// let mut pipes = Pipes::new(child.stdin.take(), Some(b"hello".to_vec()), child.stdout.take(), None)?;
/// let mut out = Vec::new();
/// while pipes.open() {
///     pipes.step(None, &mut |stream, bytes| {
///         assert_eq!(stream, Stream::Stdout);
///         out.extend_from_slice(bytes);
///     })?;
/// }
/// assert_eq!(out, b"hello");
/// child.wait()?;
/// # Ok::<(), std::io::Error>(())
/// ```
pub struct Pipes {
    stdin: Option<ChildStdin>,
    input: Vec<u8>,
    written: usize,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    /// Whether the process had an output to read, after whose end its input is no longer written.
    outputs: bool,
    buffer: Box<[u8]>,
}

impl Pipes {
    /// The pipes of a process; `input` goes to `stdin`, which is closed once it is written, at once without input, and
    /// once the outputs ended, since a process that left unread input behind has ended.
    pub fn new(
        stdin: Option<ChildStdin>,
        input: Option<Vec<u8>>,
        stdout: Option<ChildStdout>,
        stderr: Option<ChildStderr>,
    ) -> io::Result<Pipes> {
        let stdin = stdin.filter(|_| input.as_ref().is_some_and(|i| !i.is_empty()));
        for fd in [
            stdin.as_ref().map(AsRawFd::as_raw_fd),
            stdout.as_ref().map(AsRawFd::as_raw_fd),
            stderr.as_ref().map(AsRawFd::as_raw_fd),
        ]
        .into_iter()
        .flatten()
        {
            nonblocking(fd)?;
        }
        Ok(Pipes {
            outputs: stdout.is_some() || stderr.is_some(),
            stdin,
            input: input.unwrap_or_default(),
            written: 0,
            stdout,
            stderr,
            buffer: vec![0; 64 * 1024].into_boxed_slice(),
        })
    }

    /// Whether anything is left to write or to read.
    pub fn open(&self) -> bool {
        self.stdin.is_some() || self.stdout.is_some() || self.stderr.is_some()
    }

    /// Waits up to `timeout`, or for as long as it takes with `None`, until a stream is ready, then writes what input
    /// it can and hands what output arrived to `arrived`.
    pub fn step(&mut self, timeout: Option<Duration>, arrived: &mut dyn FnMut(Stream, &[u8])) -> io::Result<()> {
        let mut fds = Vec::with_capacity(3);
        if let Some(stdin) = &self.stdin {
            fds.push(pollfd(stdin.as_raw_fd(), libc::POLLOUT));
        }
        if let Some(stdout) = &self.stdout {
            fds.push(pollfd(stdout.as_raw_fd(), libc::POLLIN));
        }
        if let Some(stderr) = &self.stderr {
            fds.push(pollfd(stderr.as_raw_fd(), libc::POLLIN));
        }
        if fds.is_empty() {
            return Ok(());
        }
        let wait = timeout.map_or(-1, |t| {
            libc::c_int::try_from(t.as_millis().max(u128::from(!t.is_zero()))).unwrap_or(libc::c_int::MAX)
        });
        let count = libc::nfds_t::try_from(fds.len()).unwrap_or(0);
        // SAFETY: `fds` holds `count` live pollfd entries, which poll reads and whose `revents` it writes.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), count, wait) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            return if error.kind() == io::ErrorKind::Interrupted {
                Ok(())
            } else {
                Err(error)
            };
        }
        let ready = |fd: RawFd| fds.iter().any(|p| p.fd == fd && p.revents != 0);
        if self.stdin.as_ref().is_some_and(|s| ready(s.as_raw_fd())) {
            self.write();
        }
        if self.stdout.as_ref().is_some_and(|s| ready(s.as_raw_fd()))
            && drain(self.stdout.as_mut(), &mut self.buffer, Stream::Stdout, arrived)
        {
            self.stdout = None;
        }
        if self.stderr.as_ref().is_some_and(|s| ready(s.as_raw_fd()))
            && drain(self.stderr.as_mut(), &mut self.buffer, Stream::Stderr, arrived)
        {
            self.stderr = None;
        }
        if self.outputs && self.stdout.is_none() && self.stderr.is_none() {
            self.stdin = None;
        }
        Ok(())
    }

    fn write(&mut self) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        loop {
            match stdin.write(&self.input[self.written..]) {
                Ok(0) => break,
                Ok(n) => {
                    self.written += n;
                    if self.written == self.input.len() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return,
                // The process closed its input, as git does when it needs no more of it.
                Err(_) => break,
            }
        }
        self.stdin = None;
    }
}

/// Reads what `reader` holds now into `arrived`; whether it reached its end or failed.
fn drain(
    reader: Option<&mut impl Read>,
    buffer: &mut [u8],
    stream: Stream,
    arrived: &mut dyn FnMut(Stream, &[u8]),
) -> bool {
    let Some(reader) = reader else {
        return true;
    };
    loop {
        match reader.read(buffer) {
            Ok(0) => return true,
            Ok(n) => arrived(stream, &buffer[..n]),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return false,
            Err(_) => return true,
        }
    }
}

fn pollfd(fd: RawFd, events: libc::c_short) -> libc::pollfd {
    libc::pollfd { fd, events, revents: 0 }
}

fn nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl reads and sets the flags of a descriptor the caller owns and keeps open.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    // SAFETY: As above.
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};
    use std::time::Instant;

    use super::*;

    fn collect(pipes: &mut Pipes) -> (Vec<u8>, Vec<u8>) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        while pipes.open() {
            pipes
                .step(Some(Duration::from_secs(5)), &mut |stream, bytes| match stream {
                    Stream::Stdout => out.extend_from_slice(bytes),
                    Stream::Stderr => err.extend_from_slice(bytes),
                })
                .unwrap();
        }
        (out, err)
    }

    #[test]
    fn input_larger_than_a_pipe_goes_through_while_output_is_read() {
        let mut child = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let input: Vec<u8> = (0..=250u8).cycle().take(4 * 1024 * 1024).collect();
        let mut pipes = Pipes::new(
            child.stdin.take(),
            Some(input.clone()),
            child.stdout.take(),
            child.stderr.take(),
        )
        .unwrap();
        let (out, err) = collect(&mut pipes);
        assert!(out == input, "cat gave back what it got");
        assert!(err.is_empty());
        assert!(child.wait().unwrap().success());
    }

    #[test]
    fn both_outputs_arrive_apart() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "echo out; echo err >&2; echo more"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipes = Pipes::new(None, None, child.stdout.take(), child.stderr.take()).unwrap();
        let (out, err) = collect(&mut pipes);
        assert_eq!((out.as_slice(), err.as_slice()), (&b"out\nmore\n"[..], &b"err\n"[..]));
        child.wait().unwrap();
    }

    #[test]
    fn a_step_returns_after_its_timeout_when_nothing_arrives() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pipes = Pipes::new(None, None, child.stdout.take(), None).unwrap();
        let started = Instant::now();
        pipes
            .step(Some(Duration::from_millis(100)), &mut |_, _| {
                panic!("sleep prints nothing")
            })
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(90));
        assert!(pipes.open());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn a_process_that_does_not_read_its_input_does_not_hold_the_caller() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exec 0<&-; echo done"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = vec![b'x'; 1024 * 1024];
        let mut pipes = Pipes::new(child.stdin.take(), Some(input), child.stdout.take(), None).unwrap();
        let (out, _) = collect(&mut pipes);
        assert_eq!(out, b"done\n");
        child.wait().unwrap();
    }
}
