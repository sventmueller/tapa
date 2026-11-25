//! Short-lived helper programs, such as `go env`, `xcrun --find`, `cargo build`, `ps` or an installer's `npm install`:
//! found on the login shell's PATH and run through the supervisor with that environment, a time limit, a way to cancel
//! and a cap on what of their output is kept.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use term_base::Cancel;

use crate::pipes::{Pipes, Stream};
use crate::supervisor::{Ending, Kind, ProcessId, Supervisor};
use crate::tail::Tail;

/// The most of a helper's standard output and of its standard error that is kept: the last bytes, from a whole line.
pub(crate) const KEPT: usize = 1024 * 1024;

/// How often a running helper is looked at for being cancelled or past its time.
const POLL: Duration = Duration::from_millis(50);

/// How long the output of a helper that ended may still take to arrive, as a grandchild may hold its pipes open.
const AFTERMATH: Duration = Duration::from_secs(2);

/// How long a stopped helper gets between SIGTERM and SIGKILL.
const GRACE: Duration = Duration::from_millis(500);

/// What a helper printed and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    /// How it ended.
    pub ending: Ending,
    /// The end of its standard output, at most [`KEPT`] bytes.
    pub stdout: String,
    /// The end of its standard error, at most [`KEPT`] bytes, one line after the other.
    pub stderr: String,
}

impl Output {
    /// Whether it ended with exit code 0.
    pub fn succeeded(&self) -> bool {
        self.ending == Ending::Code(0)
    }
}

/// Why a helper has no output.
#[derive(Debug)]
pub enum RunError {
    /// It could not be started.
    Start(io::Error),
    /// It ran longer than its time limit and was stopped.
    Timeout,
    /// It was cancelled and stopped.
    Cancelled,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Start(error) => write!(f, "could not start: {error}"),
            RunError::Timeout => f.write_str("took too long"),
            RunError::Cancelled => f.write_str("was cancelled"),
        }
    }
}

impl std::error::Error for RunError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RunError::Start(error) => Some(error),
            RunError::Timeout | RunError::Cancelled => None,
        }
    }
}

impl From<RunError> for io::Error {
    fn from(error: RunError) -> io::Error {
        match error {
            RunError::Start(error) => error,
            RunError::Timeout => io::Error::new(io::ErrorKind::TimedOut, error.to_string()),
            RunError::Cancelled => io::Error::new(io::ErrorKind::Interrupted, error.to_string()),
        }
    }
}

/// How helpers are run: through `supervisor`, with `env` only, the login shell's environment (E10, E23), for at most
/// `timeout`, and stopped once `cancel` is.
#[derive(Clone, Copy)]
pub struct Helper<'a> {
    /// The supervisor every helper runs under.
    pub supervisor: &'a Supervisor,
    /// The whole environment of a helper; a later entry of a name wins, as for [`Command::envs`].
    pub env: &'a [(String, String)],
    /// How long a helper may run.
    pub timeout: Duration,
    /// What stops a helper early.
    pub cancel: Option<&'a Cancel>,
}

impl Helper<'_> {
    /// Runs `program` with `args` in `dir`, without input; `line` hears each line of its standard error as it comes.
    ///
    /// ```
    /// use std::path::Path;
    /// use std::time::Duration;
    ///
    /// use term_process::Supervisor;
    /// use term_process::tool::Helper;
    ///
    /// let supervisor = Supervisor::new();
    /// let env = [("PATH".to_string(), "/usr/bin:/bin".to_string())];
    /// let helper = Helper { supervisor: &supervisor, env: &env, timeout: Duration::from_secs(10), cancel: None };
    /// let output = helper.run(Path::new("/bin/echo"), &["hello"], Path::new("/"), &mut |_| {})?;
    /// assert!(output.succeeded());
    /// assert_eq!(output.stdout, "hello\n");
    /// # Ok::<(), term_process::tool::RunError>(())
    /// ```
    pub fn run(
        &self,
        program: &Path,
        args: &[&str],
        dir: &Path,
        line: &mut dyn FnMut(&str),
    ) -> Result<Output, RunError> {
        let mut command = Command::new(program);
        command
            .args(args)
            .current_dir(dir)
            .env_clear()
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let label = program
            .file_name()
            .map_or_else(|| program.display().to_string(), |n| n.to_string_lossy().into_owned());
        let spawned = self
            .supervisor
            .spawn_awaited(command, Kind::Tool, label)
            .map_err(RunError::Start)?;
        let pipes = match Pipes::new(None, None, spawned.stdout, spawned.stderr) {
            Ok(pipes) => pipes,
            Err(error) => {
                self.give_up(spawned.id);
                return Err(RunError::Start(error));
            }
        };
        let mut stdout = Tail::new(KEPT);
        let mut stderr = Lines::new();
        let ending = self.serve(spawned.id, pipes, &mut |stream, bytes| match stream {
            Stream::Stdout => stdout.push(bytes),
            Stream::Stderr => stderr.push(bytes, line),
        })?;
        stderr.finish(line);
        Ok(Output {
            ending,
            stdout: stdout.text(),
            stderr: stderr.tail.text(),
        })
    }

    /// Serves the pipes of the helper `id` on this thread until it ended and its pipes closed, or [`AFTERMATH`] after
    /// its end, as a grandchild may hold them open; stops it when it is cancelled or past its time.
    fn serve(&self, id: ProcessId, pipes: Pipes, arrived: &mut dyn FnMut(Stream, &[u8])) -> Result<Ending, RunError> {
        let deadline = Instant::now() + self.timeout;
        let mut pipes = Some(pipes);
        let mut ended: Option<(Ending, Instant)> = None;
        loop {
            let open = pipes.as_ref().is_some_and(Pipes::open);
            let now = Instant::now();
            let look = match ended {
                Some((ending, until)) => {
                    if !open || now >= until {
                        return Ok(ending);
                    }
                    until - now
                }
                None => {
                    if self.cancel.is_some_and(Cancel::is_cancelled) {
                        self.give_up(id);
                        return Err(RunError::Cancelled);
                    }
                    if now >= deadline {
                        self.give_up(id);
                        return Err(RunError::Timeout);
                    }
                    let look = POLL.min(deadline - now);
                    if let Some(ending) = self.supervisor.wait(id, if open { Duration::ZERO } else { look }) {
                        ended = Some((ending, Instant::now() + AFTERMATH));
                        continue;
                    }
                    look
                }
            };
            if open
                && let Some(served) = pipes.as_mut()
                && served.step(Some(look), arrived).is_err()
            {
                pipes = None;
            }
        }
    }

    /// Stops the helper `id` and gives up its end, for a caller that no longer waits for it.
    fn give_up(&self, id: ProcessId) {
        self.supervisor.stop(id, GRACE);
        self.supervisor.release(id);
    }
}

/// A helper's standard error: its tail, one line after the other, and the line being read; a line longer than [`KEPT`]
/// is dropped.
struct Lines {
    tail: Tail,
    pending: Vec<u8>,
    too_long: bool,
}

impl Lines {
    fn new() -> Lines {
        Lines {
            tail: Tail::new(KEPT),
            pending: Vec::new(),
            too_long: false,
        }
    }

    /// Takes in `bytes` as they arrived, handing each line they end to `line` without its line break.
    fn push(&mut self, bytes: &[u8], line: &mut dyn FnMut(&str)) {
        for piece in bytes.split_inclusive(|&b| b == b'\n') {
            let (content, ends) = match piece.strip_suffix(b"\n") {
                Some(content) => (content, true),
                None => (piece, false),
            };
            if !self.too_long {
                if self.pending.len() + content.len() > KEPT {
                    self.too_long = true;
                    self.pending.clear();
                } else {
                    self.pending.extend_from_slice(content);
                }
            }
            if ends {
                self.end_line(line);
            }
        }
    }

    /// Hands on the last line, which the stream ended without a line break.
    fn finish(&mut self, line: &mut dyn FnMut(&str)) {
        if !self.pending.is_empty() || self.too_long {
            self.end_line(line);
        }
    }

    fn end_line(&mut self, line: &mut dyn FnMut(&str)) {
        if !self.too_long {
            let text = String::from_utf8_lossy(&self.pending);
            let text = text.trim_end_matches(['\r', '\n']);
            line(text);
            self.tail.push(text.as_bytes());
            self.tail.push(b"\n");
        }
        self.pending.clear();
        self.too_long = false;
    }
}

/// The first executable called `name` in the absolute folders of `path`, a PATH-style list such as [`var`] gives for
/// `PATH`.
pub fn which(name: &str, path: Option<&str>) -> Option<PathBuf> {
    found(name, path, |_| true)
}

/// The first executable called `name` in the absolute folders of `path` that do not lie in `base`, such as a
/// project's `node_modules/.bin`, since nothing from a project folder runs (E16).
pub fn which_outside(name: &str, path: Option<&str>, base: &Path) -> Option<PathBuf> {
    found(name, path, |dir| !lies_in(dir, base))
}

/// Whether `path` lies in the workspace folder `base`, from which nothing runs (E16); never when `base` is the home
/// folder, which holds the user's own programs and is no project.
pub fn lies_in(path: &Path, base: &Path) -> bool {
    let home = term_base::home();
    let resolved = std::fs::canonicalize(base).ok();
    if base == home
        || resolved
            .as_deref()
            .is_some_and(|b| std::fs::canonicalize(&home).is_ok_and(|h| b == h))
    {
        return false;
    }
    let inside = |base: &Path| path.starts_with(base) || std::fs::canonicalize(path).is_ok_and(|p| p.starts_with(base));
    inside(base) || resolved.as_deref().is_some_and(inside)
}

/// A relative entry, such as `.` or `bin`, would find programs in whatever folder the app is in.
fn found(name: &str, path: Option<&str>, allowed: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    path?
        .split(':')
        .map(Path::new)
        .filter(|dir| dir.is_absolute() && allowed(dir))
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

/// Whether `path` is a file someone may execute.
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The value of `name` in `env` as a child started with it sees it: the last entry of the name, since
/// [`Command::envs`] sets the entries in order, so an entry added to the login shell's list overrides it.
pub fn var<'a>(env: &'a [(String, String)], name: &str) -> Option<&'a str> {
    env.iter().rev().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> Vec<(String, String)> {
        vec![("PATH".into(), "/usr/bin:/bin".into())]
    }

    fn helper<'a>(supervisor: &'a Supervisor, env: &'a [(String, String)], timeout: Duration) -> Helper<'a> {
        Helper {
            supervisor,
            env,
            timeout,
            cancel: None,
        }
    }

    #[test]
    fn a_helper_reports_its_output_lines_and_exit_code() {
        let supervisor = Supervisor::new();
        let env = env();
        let mut heard = vec![];
        let output = helper(&supervisor, &env, Duration::from_secs(10))
            .run(
                Path::new("/bin/sh"),
                &["-c", "echo out; echo e1 >&2; echo e2 >&2; exit 4"],
                Path::new("/"),
                &mut |l| heard.push(l.to_string()),
            )
            .unwrap();
        assert_eq!(output.stdout, "out\n");
        assert_eq!(output.stderr, "e1\ne2\n");
        assert_eq!(heard, ["e1", "e2"]);
        assert_eq!(output.ending, Ending::Code(4));
        assert!(!output.succeeded());
    }

    #[test]
    fn a_helper_gets_only_the_environment_it_is_given_with_the_last_entry_winning() {
        let supervisor = Supervisor::new();
        let mut env = env();
        env.push(("ONLY".into(), "first".into()));
        env.push(("ONLY".into(), "last".into()));
        assert_eq!(var(&env, "ONLY"), Some("last"));
        let output = helper(&supervisor, &env, Duration::from_secs(10))
            .run(Path::new("/usr/bin/env"), &[], Path::new("/"), &mut |_| {})
            .unwrap();
        let lines: Vec<&str> = output.stdout.lines().collect();
        assert!(
            lines.contains(&"ONLY=last") && lines.contains(&"PATH=/usr/bin:/bin"),
            "{lines:?}"
        );
        assert!(
            !lines.contains(&"ONLY=first") && !lines.iter().any(|l| l.starts_with("HOME=")),
            "{lines:?}"
        );
    }

    #[test]
    fn a_helper_that_writes_a_lot_keeps_the_last_mebibyte_of_each_stream() {
        let supervisor = Supervisor::new();
        let env = env();
        let script = "i=0; while [ $i -lt 3000 ]; do \
                      printf '%0999d\\n' $i; printf '%0999d\\n' $i >&2; i=$((i+1)); done; echo last; echo last >&2";
        let output = helper(&supervisor, &env, Duration::from_secs(60))
            .run(Path::new("/bin/sh"), &["-c", script], Path::new("/"), &mut |_| {})
            .unwrap();
        assert!(output.succeeded());
        for stream in [&output.stdout, &output.stderr] {
            assert!(stream.len() <= KEPT, "{} bytes kept", stream.len());
            assert!(stream.len() > KEPT / 2);
            assert!(stream.ends_with("last\n"));
            assert_eq!(stream.lines().next().map(str::len), Some(999), "kept from a whole line");
        }
    }

    #[test]
    fn a_helper_is_stopped_when_it_takes_too_long_or_is_cancelled() {
        let supervisor = Supervisor::new();
        let env = env();
        let slow = helper(&supervisor, &env, Duration::from_millis(200));
        let result = slow.run(Path::new("/bin/sleep"), &["30"], Path::new("/"), &mut |_| {});
        assert!(matches!(result, Err(RunError::Timeout)));
        let cancel = Cancel::new();
        let cancelled = Helper {
            cancel: Some(&cancel),
            timeout: Duration::from_secs(30),
            ..slow
        };
        let started = Instant::now();
        let canceller = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                cancel.cancel();
            })
        };
        let result = cancelled.run(Path::new("/bin/sleep"), &["30"], Path::new("/"), &mut |_| {});
        canceller.join().unwrap();
        assert!(matches!(result, Err(RunError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(10));
        let deadline = Instant::now() + Duration::from_secs(5);
        while !supervisor.running().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(supervisor.running().is_empty());
    }

    #[test]
    fn a_line_too_long_to_keep_is_dropped_and_the_lines_after_it_are_heard() {
        let supervisor = Supervisor::new();
        let env = env();
        let mut heard = vec![];
        let script = "{ echo first; head -c 1100000 /dev/zero | tr '\\0' x; echo; printf after; } >&2";
        let output = helper(&supervisor, &env, Duration::from_secs(30))
            .run(Path::new("/bin/sh"), &["-c", script], Path::new("/"), &mut |l| {
                heard.push(l.to_string());
            })
            .unwrap();
        assert!(output.succeeded());
        assert_eq!(
            heard,
            ["first", "after"],
            "the last line arrives without its line break"
        );
        assert_eq!(output.stderr, "first\nafter\n");
    }

    #[test]
    fn a_helper_whose_grandchild_keeps_its_pipes_open_returns_soon_after_it_ended() {
        let supervisor = Supervisor::new();
        let env = env();
        let started = Instant::now();
        let output = helper(&supervisor, &env, Duration::from_secs(60))
            .run(
                Path::new("/bin/sh"),
                &["-c", "sleep 30 & echo $!; echo gone >&2"],
                Path::new("/"),
                &mut |_| {},
            )
            .unwrap();
        let elapsed = started.elapsed();
        if let Ok(pid) = output.stdout.trim().parse::<libc::pid_t>() {
            // SAFETY: kill only sends a signal, to the sleep the script just started and printed the number of.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        assert!(output.succeeded());
        assert_eq!(output.stderr, "gone\n");
        assert!(elapsed < AFTERMATH + Duration::from_secs(5), "{elapsed:?}");
    }

    #[test]
    fn a_program_that_cannot_start_says_so() {
        let supervisor = Supervisor::new();
        let env = env();
        let result = helper(&supervisor, &env, Duration::from_secs(5)).run(
            Path::new("/nonexistent/program"),
            &[],
            Path::new("/"),
            &mut |_| {},
        );
        let Err(RunError::Start(error)) = result else {
            panic!("{result:?}")
        };
        assert_eq!(io::Error::from(RunError::Start(error)).kind(), io::ErrorKind::NotFound);
        assert_eq!(io::Error::from(RunError::Timeout).kind(), io::ErrorKind::TimedOut);
    }

    #[test]
    fn executables_are_found_on_the_given_path_only() {
        assert_eq!(which("sh", Some("/nonexistent:/bin")), Some(PathBuf::from("/bin/sh")));
        assert_eq!(which("sh", Some("/nonexistent")), None);
        assert_eq!(which("sh", None), None);
        assert_eq!(which("sh", Some("bin:.:")), None, "relative entries are skipped");
        assert!(is_executable(Path::new("/bin/sh")));
        assert!(!is_executable(Path::new("/bin")));
        assert_eq!(var(&env(), "PATH"), Some("/usr/bin:/bin"));
        assert_eq!(var(&env(), "HOME"), None);
    }

    #[test]
    fn programs_inside_the_base_path_are_not_found() {
        let project = term_base::testing::scratch("which-outside");
        let bin = project.path().join("node_modules/.bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join("sh");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let path = format!("{}:/bin", bin.display());
        assert_eq!(which("sh", Some(&path)), Some(program));
        assert_eq!(
            which_outside("sh", Some(&path), project.path()),
            Some(PathBuf::from("/bin/sh"))
        );
        assert_eq!(which_outside("sh", Some(&path), Path::new("/")), None);
    }

    #[test]
    fn the_home_folder_is_no_project() {
        let home = term_base::home();
        let program = home.join(".local/bin/claude");
        assert!(!lies_in(&program, &home));
        assert!(lies_in(&program, &home.join(".local")));
    }
}
