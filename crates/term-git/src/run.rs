//! One git process per call, through the supervisor, with the user's environment, limits on what it may print, and an
//! end that can be waited for, cancelled or timed out.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use term_process::pipes::{Pipes, Stream};
use term_process::tail::Tail;
use term_process::tool;
use term_process::{Ending, Kind, ProcessId, Spawned, Supervisor};

use crate::error::{Error, Result};
use crate::neutral::{self, Policy, UserConfig};
use crate::said::Said;

/// How much of standard error is kept.
const STDERR_LIMIT: usize = 64 * 1024;

/// How much of standard output is read before the call fails; diffs and patches are useless when cut.
const STDOUT_LIMIT: usize = 256 * 1024 * 1024;

/// The limit for commands that only read or write the local repository.
pub(crate) const LOCAL_TIMEOUT: Duration = Duration::from_secs(300);

/// How long a stopped git gets for each step from SIGTERM to SIGKILL.
const STOP_GRACE: Duration = Duration::from_millis(500);

/// SIGHUP, SIGINT, SIGKILL and SIGTERM: someone stopped git, it did not crash.
const STOP_SIGNALS: [i32; 4] = [1, 2, 9, 15];

/// How often a call that can be cancelled looks whether it was; nothing wakes it when that happens.
const CANCEL_LOOK: Duration = Duration::from_millis(50);

/// How long a call that was stopped waits at a time for its process to end, which SIGKILL guarantees.
const STOPPED_LOOK: Duration = Duration::from_secs(60);

/// How often a call whose pipes are still open looks whether its process ended.
const END_LOOK: Duration = Duration::from_millis(250);

/// How long the pipes of a git that ended may stay open before they are given up: a grandchild outside its process
/// group, such as an ssh ControlPersist master, can hold them for as long as it lives.
const AFTERMATH: Duration = Duration::from_secs(2);

/// Variables that tie git to one repository; `git rev-parse --local-env-vars` lists them.
const REPOSITORY_VARIABLES: [&str; 15] = [
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

/// Variables this crate sets itself.
const OWN_VARIABLES: [&str; 10] = [
    "GIT_TERMINAL_PROMPT",
    "GIT_TRACE2_PARENT_SID",
    "GIT_TRACE2_PARENT_NAME",
    "GIT_EDITOR",
    "GIT_MERGE_AUTOEDIT",
    "GIT_TRACE2_EVENT",
    "LC_ALL",
    "LANG",
    "LANGUAGE",
    "GIT_PAGER",
];

pub use term_base::Cancel;

/// The app's way of running git: the program from the login shell's PATH, that shell's environment, and the
/// supervisor every process goes through. Make one and clone it: clones share what it found.
#[derive(Clone)]
pub struct Git {
    inner: Arc<Inner>,
}

struct Inner {
    program: Option<PathBuf>,
    environment: Vec<(OsString, OsString)>,
    supervisor: Supervisor,
}

impl std::fmt::Debug for Git {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Git")
            .field("program", &self.inner.program)
            .finish_non_exhaustive()
    }
}

impl Git {
    /// git with the login shell's `environment`, which carries PATH, `SSH_AUTH_SOCK` and what credential helpers need.
    pub fn new(supervisor: &Supervisor, environment: Vec<(String, String)>) -> Git {
        let path = tool::var(&environment, "PATH").map(str::to_string);
        let environment = prepared(environment);
        let program = tool::which("git", path.as_deref()).or_else(|| {
            let fallback = PathBuf::from("/usr/bin/git");
            fallback.exists().then_some(fallback)
        });
        Git {
            inner: Arc::new(Inner {
                program,
                environment,
                supervisor: supervisor.clone(),
            }),
        }
    }

    /// The git program in use, if one was found.
    pub fn program(&self) -> Option<&Path> {
        self.inner.program.as_deref()
    }

    /// git's version, such as `git version 2.50.1`; `GitMissing` when git cannot run.
    pub fn version(&self) -> Result<String> {
        let dir = std::env::temp_dir();
        let output = self.ok(Call::new(&dir, ["--version"], Policy::Untrusted))?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Runs `call` and fails unless git exits with 0.
    pub(crate) fn ok(&self, call: Call<'_>) -> Result<Output> {
        let output = self.run(call)?;
        if output.code == 0 {
            Ok(output)
        } else {
            Err(failure(&output))
        }
    }

    /// Runs `call` to its end; the exit code is the caller's to judge.
    pub(crate) fn run(&self, mut call: Call<'_>) -> Result<Output> {
        let program = self.inner.program.as_ref().ok_or(Error::GitMissing)?;
        if call.policy == Policy::Untrusted {
            let neutral = neutral::environment(call.dir, &self.inner.environment, |at| self.users(at))?;
            call.env.extend(neutral);
        }
        self.run_program(program, call)
    }

    /// The user's system and global configuration as git reads it in `dir`, where conditional includes may apply.
    fn users(&self, dir: &Path) -> UserConfig {
        let read = |scope: &str| {
            let call = Call::new(dir, ["config", "--includes", "-z", "--list", scope], Policy::Trusted)
                .envs(&neutral::fixed_environment());
            self.run(call)
                .ok()
                .filter(|output| output.code == 0)
                .map(|output| output.stdout)
        };
        UserConfig::parse(&[read("--system"), read("--global")])
    }

    /// Runs another program the same way.
    pub(crate) fn run_program(&self, program: &Path, call: Call<'_>) -> Result<Output> {
        let mut command = Command::new(program);
        command
            .args(&call.args)
            .current_dir(call.dir)
            .env_clear()
            .envs(self.inner.environment.iter().map(|(k, v)| (k, v)))
            .envs(call.env.iter().map(|(k, v)| (k, v)))
            .stdin(if call.input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let kind = if program.file_name() == Some(OsStr::new("git")) {
            Kind::Git
        } else {
            Kind::Tool
        };
        let label = label(program, &call.args);
        let spawned = match self.inner.supervisor.spawn_awaited(command, kind, label.clone()) {
            Ok(spawned) => spawned,
            Err(error) if error.kind() == io::ErrorKind::NotFound && !program.exists() => {
                return Err(Error::GitMissing);
            }
            Err(error) => return Err(Error::Io(error)),
        };
        self.collect(spawned, call, label)
    }

    fn collect(&self, spawned: Spawned, mut call: Call<'_>, command: String) -> Result<Output> {
        let id = spawned.id;
        let supervisor = &self.inner.supervisor;
        let pipes = Pipes::new(spawned.stdin, call.input.take(), spawned.stdout, spawned.stderr);
        let (mut stdout, mut stderr) = (Limited::new(STDOUT_LIMIT), Errors::new(STDERR_LIMIT));
        let served = pipes.and_then(|mut pipes| {
            let deadline = call.timeout.map(|t| Instant::now() + t);
            let mut stopped = None;
            let mut ended: Option<(Ending, Instant)> = None;
            while pipes.open() {
                let look = match ended {
                    Some((_, until)) => match until.checked_duration_since(Instant::now()) {
                        Some(left) if !left.is_zero() => left,
                        _ => break,
                    },
                    None => next_look(&call, deadline, stopped.is_some()).map_or(END_LOOK, |look| look.min(END_LOOK)),
                };
                pipes.step(Some(look), &mut |stream, bytes| match stream {
                    Stream::Stdout => stdout.push(bytes),
                    Stream::Stderr => stderr.push(bytes, call.progress.as_deref_mut()),
                })?;
                if ended.is_none() {
                    self.check_stop(id, &call, deadline, &mut stopped);
                    ended = supervisor
                        .wait(id, Duration::ZERO)
                        .map(|ending| (ending, Instant::now() + AFTERMATH));
                }
            }
            stderr.finish(call.progress.as_deref_mut());
            let ending = match ended {
                Some((ending, _)) => ending,
                None => loop {
                    let look = next_look(&call, deadline, stopped.is_some()).unwrap_or(STOPPED_LOOK);
                    if let Some(ending) = supervisor.wait(id, look) {
                        break ending;
                    }
                    self.check_stop(id, &call, deadline, &mut stopped);
                },
            };
            Ok((ending, stopped))
        });
        let (ending, stopped) = match served {
            Ok(served) => served,
            Err(error) => {
                // The process already runs; without its pipes served nobody would end it or collect its end.
                supervisor.stop(id, Duration::from_millis(200));
                supervisor.release(id);
                return Err(Error::Io(error));
            }
        };
        let Limited {
            kept: stdout, overflow, ..
        } = stdout;
        if let Some(error) = stopped {
            return Err(error);
        }
        let stderr = stderr.tail.text();
        let code = match ending {
            Ending::Code(code) => code,
            Ending::Signal(signal) if STOP_SIGNALS.contains(&signal) => {
                return Err(Error::Cancelled);
            }
            Ending::Signal(_) | Ending::Unknown => {
                return Err(Error::Failed {
                    command,
                    code: None,
                    output: stderr,
                });
            }
        };
        if overflow {
            return Err(Error::OutputTooLarge);
        }
        Ok(Output {
            code,
            stdout,
            stderr,
            command,
            dir: call.dir.to_path_buf(),
        })
    }

    fn check_stop(&self, id: ProcessId, call: &Call<'_>, deadline: Option<Instant>, stopped: &mut Option<Error>) {
        if stopped.is_some() {
            return;
        }
        if call.cancel.is_some_and(Cancel::is_cancelled) {
            *stopped = Some(Error::Cancelled);
        } else if deadline.is_some_and(|d| Instant::now() >= d) {
            *stopped = Some(Error::TimedOut);
        } else {
            return;
        }
        self.inner.supervisor.stop(id, STOP_GRACE);
    }
}

/// One git command: its arguments, folder, extra environment, input, limits and listeners.
pub(crate) struct Call<'a> {
    args: Vec<OsString>,
    dir: &'a Path,
    env: Vec<(OsString, OsString)>,
    input: Option<Vec<u8>>,
    timeout: Option<Duration>,
    cancel: Option<&'a Cancel>,
    progress: Option<&'a mut dyn FnMut(&str)>,
    policy: Policy,
}

impl<'a> Call<'a> {
    /// `git <args>` in `dir` under `policy`, with the local time limit.
    pub(crate) fn new<I, S>(dir: &'a Path, args: I, policy: Policy) -> Call<'a>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        Call {
            args: args.into_iter().map(|a| a.as_ref().to_os_string()).collect(),
            dir,
            env: Vec::new(),
            input: None,
            timeout: Some(LOCAL_TIMEOUT),
            cancel: None,
            progress: None,
            policy,
        }
    }

    /// Adds arguments.
    pub(crate) fn args<I, S>(mut self, args: I) -> Call<'a>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    /// Sets an environment variable for this call.
    pub(crate) fn env(mut self, name: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Call<'a> {
        self.env
            .push((name.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    /// Sets the variables of `env` for this call.
    pub(crate) fn envs(mut self, env: &[(OsString, OsString)]) -> Call<'a> {
        self.env.extend_from_slice(env);
        self
    }

    /// Writes `input` to git's standard input.
    pub(crate) fn input(mut self, input: Vec<u8>) -> Call<'a> {
        self.input = Some(input);
        self
    }

    /// Lets the call run as long as it takes; for network operations and hooks, which can be cancelled instead.
    pub(crate) fn unlimited(mut self) -> Call<'a> {
        self.timeout = None;
        self
    }

    /// Stops the call when `cancel` is requested.
    pub(crate) fn cancel(mut self, cancel: &'a Cancel) -> Call<'a> {
        self.cancel = Some(cancel);
        self
    }

    /// Hands each line or carriage-return-separated piece of standard error to `progress` as it arrives.
    pub(crate) fn progress(mut self, progress: &'a mut dyn FnMut(&str)) -> Call<'a> {
        self.progress = Some(progress);
        self
    }
}

/// What a finished git printed.
#[derive(Debug)]
pub(crate) struct Output {
    pub(crate) code: i32,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: String,
    pub(crate) command: String,
    pub(crate) dir: PathBuf,
}

impl Output {
    /// Standard output as text, trimmed.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.stdout).trim().to_string()
    }

    /// Standard error, then standard output, for errors that show what git said.
    pub(crate) fn said(&self) -> String {
        let stdout = String::from_utf8_lossy(&self.stdout);
        match (self.stderr.trim().is_empty(), stdout.trim().is_empty()) {
            (_, true) => self.stderr.clone(),
            (true, false) => stdout.into_owned(),
            (false, false) => format!("{}\n{}", self.stderr.trim_end(), stdout.trim_end()),
        }
    }
}

/// The typed error for a git that failed; specific commands look for their own cases first.
pub(crate) fn failure(output: &Output) -> Error {
    let said = output.said();
    if Said::GitMissing.in_text(&said) {
        Error::GitMissing
    } else if Said::NotARepository.in_text(&said) {
        Error::NotARepository {
            path: output.dir.clone(),
        }
    } else if Said::DubiousOwnership.in_text(&said) {
        Error::UnsafeRepository {
            path: output.dir.clone(),
            output: said,
        }
    } else if Said::AuthFailed.in_text(&said) {
        Error::AuthFailed { output: said }
    } else if Said::Network.in_text(&said) {
        Error::Network { output: said }
    } else {
        Error::Failed {
            command: output.command.clone(),
            code: Some(output.code),
            output: said,
        }
    }
}

/// How long the calling thread may wait before it looks at the call's cancel and time limit again; `None` for as long
/// as it takes.
fn next_look(call: &Call<'_>, deadline: Option<Instant>, stopped: bool) -> Option<Duration> {
    if stopped {
        return None;
    }
    let left = deadline.map(|d| d.saturating_duration_since(Instant::now()));
    if call.cancel.is_some() {
        Some(left.map_or(CANCEL_LOOK, |left| left.min(CANCEL_LOOK)))
    } else {
        left
    }
}

/// The login shell's environment without what would point git elsewhere, plus what keeps git from waiting for a
/// person who is not there.
fn prepared(environment: Vec<(String, String)>) -> Vec<(OsString, OsString)> {
    let mut prepared: Vec<(OsString, OsString)> = environment
        .into_iter()
        .filter(|(name, _)| !REPOSITORY_VARIABLES.contains(&name.as_str()) && !OWN_VARIABLES.contains(&name.as_str()))
        .map(|(name, value)| (name.into(), value.into()))
        .collect();
    for (name, value) in [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GIT_EDITOR", "true"),
        ("GIT_MERGE_AUTOEDIT", "no"),
        ("GIT_PAGER", "cat"),
        // A few errors are told apart by git's English words (`said.rs`); UTF-8 keeps hooks reading files as before.
        ("LC_ALL", "en_US.UTF-8"),
        ("LANG", "en_US.UTF-8"),
    ] {
        prepared.push((name.into(), value.into()));
    }
    prepared
}

/// What the activity view shows, such as `git status`.
fn label(program: &Path, args: &[OsString]) -> String {
    let name = program
        .file_name()
        .map_or_else(|| "git".into(), |n| n.to_string_lossy().into_owned());
    let mut args = args.iter().map(|a| a.to_string_lossy());
    let mut words = vec![name];
    while let Some(arg) = args.next() {
        if arg == "-c" || arg == "-C" {
            args.next();
        } else if !arg.starts_with('-') {
            words.push(arg.into_owned());
            break;
        }
    }
    words.join(" ")
}

/// Standard output up to a limit, and whether more came; what is kept is the output's start, without gaps.
struct Limited {
    kept: Vec<u8>,
    limit: usize,
    overflow: bool,
}

impl Limited {
    fn new(limit: usize) -> Limited {
        Limited {
            kept: Vec::new(),
            limit,
            overflow: false,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        if self.overflow {
            return;
        }
        let room = self.limit.saturating_sub(self.kept.len());
        if bytes.len() > room {
            self.kept.extend_from_slice(&bytes[..room]);
            self.overflow = true;
        } else {
            self.kept.extend_from_slice(bytes);
        }
    }
}

/// Standard error: its tail, and the pieces between line ends or carriage returns for a progress listener.
struct Errors {
    tail: Tail,
    pending: Vec<u8>,
}

impl Errors {
    fn new(limit: usize) -> Errors {
        Errors {
            tail: Tail::new(limit),
            pending: Vec::new(),
        }
    }

    fn push<'p>(&mut self, bytes: &[u8], progress: Option<&mut (dyn FnMut(&str) + 'p)>) {
        self.tail.push(bytes);
        let Some(progress) = progress else {
            return;
        };
        for &byte in bytes {
            if byte == b'\n' || byte == b'\r' {
                if !self.pending.is_empty() {
                    progress(&String::from_utf8_lossy(&self.pending));
                    self.pending.clear();
                }
            } else if self.pending.len() < 4096 {
                self.pending.push(byte);
            }
        }
    }

    fn finish<'p>(&mut self, progress: Option<&mut (dyn FnMut(&str) + 'p)>) {
        if let Some(progress) = progress
            && !self.pending.is_empty()
        {
            progress(&String::from_utf8_lossy(&self.pending));
            self.pending.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_loses_repository_pointers_and_gains_no_prompts() {
        let prepared = prepared(vec![
            ("PATH".into(), "/usr/bin".into()),
            ("GIT_DIR".into(), "/elsewhere".into()),
            ("LANGUAGE".into(), "de".into()),
            ("GIT_TERMINAL_PROMPT".into(), "1".into()),
            ("SSH_AUTH_SOCK".into(), "/tmp/agent".into()),
        ]);
        let get = |name: &str| {
            prepared
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.to_string_lossy().into_owned())
        };
        assert_eq!(get("GIT_DIR"), None);
        assert_eq!(get("LANGUAGE"), None);
        assert_eq!(get("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
        assert_eq!(get("SSH_AUTH_SOCK").as_deref(), Some("/tmp/agent"));
        assert_eq!(prepared.iter().filter(|(k, _)| k == "GIT_TERMINAL_PROMPT").count(), 1);
    }

    #[test]
    fn the_label_names_the_subcommand() {
        let args: Vec<OsString> = ["-c", "core.quotepath=false", "--no-optional-locks", "status", "-z"]
            .iter()
            .map(OsString::from)
            .collect();
        assert_eq!(label(Path::new("/usr/bin/git"), &args), "git status");
    }

    #[test]
    fn output_beyond_the_limit_is_flagged() {
        let mut out = Limited::new(4);
        out.push(b"0123456789");
        assert!(out.overflow);
        assert_eq!(out.kept, b"0123");
        let mut out = Limited::new(4);
        out.push(b"0123");
        assert_eq!((out.kept.as_slice(), out.overflow), (&b"0123"[..], false));
    }

    #[test]
    fn what_is_kept_past_the_limit_has_no_gap() {
        let mut out = Limited::new(6);
        for chunk in [&b"0123"[..], b"456789", b"ab"] {
            out.push(chunk);
        }
        assert!(out.overflow);
        assert_eq!(
            out.kept, b"012345",
            "the start of the output, not pieces from either side of a gap"
        );
    }

    #[test]
    fn standard_error_is_split_at_carriage_returns_and_line_ends() {
        let mut errors = Errors::new(1024);
        let mut segments = Vec::new();
        let mut progress = |s: &str| segments.push(s.to_string());
        for chunk in [&b"Counting:  50% (1/2)\rCount"[..], b"ing: 100% (2/2), done.\nlast"] {
            errors.push(chunk, Some(&mut progress));
        }
        errors.finish(Some(&mut progress));
        assert_eq!(
            segments,
            ["Counting:  50% (1/2)", "Counting: 100% (2/2), done.", "last"]
        );
        assert_eq!(
            errors.tail.text(),
            "Counting:  50% (1/2)\rCounting: 100% (2/2), done.\nlast"
        );
    }

    #[test]
    fn a_call_without_cancel_or_limit_waits_as_long_as_it_takes() {
        let dir = std::env::temp_dir();
        let cancel = Cancel::new();
        let unlimited = Call::new(&dir, ["status"], Policy::Trusted).unlimited();
        assert_eq!(next_look(&unlimited, None, false), None);
        let cancellable = Call::new(&dir, ["fetch"], Policy::Trusted).unlimited().cancel(&cancel);
        assert_eq!(next_look(&cancellable, None, false), Some(CANCEL_LOOK));
        let limited = Call::new(&dir, ["status"], Policy::Trusted);
        let soon = Instant::now() + Duration::from_secs(1);
        assert!(next_look(&limited, Some(soon), false).is_some_and(|d| d <= Duration::from_secs(1) && d > CANCEL_LOOK));
        assert_eq!(next_look(&limited, Some(Instant::now()), false), Some(Duration::ZERO));
        assert_eq!(
            next_look(&cancellable, None, true),
            None,
            "a stopped call only waits for the end"
        );
    }

    fn shell() -> Git {
        Git::new(&Supervisor::new(), vec![("PATH".into(), "/usr/bin:/bin".into())])
    }

    #[test]
    fn a_call_feeds_its_input_and_reads_both_outputs_on_the_calling_thread() {
        let dir = std::env::temp_dir();
        let mut heard = Vec::new();
        let mut progress = |s: &str| heard.push(s.to_string());
        let call = Call::new(&dir, ["-c", "cat; echo 'half\rdone' >&2; exit 3"], Policy::Trusted)
            .input(b"given".to_vec())
            .progress(&mut progress);
        let output = shell().run_program(Path::new("/bin/sh"), call).unwrap();
        assert_eq!((output.code, output.stdout.as_slice()), (3, &b"given"[..]));
        assert_eq!(output.stderr, "half\rdone\n");
        assert_eq!(heard, ["half", "done"]);
    }

    #[test]
    fn a_call_past_its_limit_is_stopped() {
        let dir = std::env::temp_dir();
        let mut call = Call::new(&dir, ["-c", "sleep 30"], Policy::Trusted);
        call.timeout = Some(Duration::from_millis(200));
        let started = Instant::now();
        let result = shell().run_program(Path::new("/bin/sh"), call);
        assert!(matches!(result, Err(Error::TimedOut)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_cancelled_call_ends_soon() {
        let dir = std::env::temp_dir();
        let cancel = Cancel::new();
        let later = cancel.clone();
        let canceller = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            later.cancel();
        });
        let started = Instant::now();
        let result = shell().run_program(
            Path::new("/bin/sh"),
            Call::new(&dir, ["-c", "sleep 30"], Policy::Trusted)
                .unlimited()
                .cancel(&cancel),
        );
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(5));
        canceller.join().unwrap();
    }

    /// Ends the sleeper whose number `text` holds, which a test's script left behind.
    fn kill(text: &str) {
        let pid = text.trim();
        if !pid.is_empty() {
            let dir = std::env::temp_dir();
            let _ = shell().run_program(Path::new("/bin/kill"), Call::new(&dir, ["-9", pid], Policy::Trusted));
        }
    }

    #[test]
    fn a_call_whose_grandchild_keeps_its_pipes_open_ends_soon_after_git() {
        let dir = std::env::temp_dir();
        let started = Instant::now();
        let output = shell()
            .run_program(
                Path::new("/bin/sh"),
                Call::new(&dir, ["-c", "sleep 30 & echo $!; echo done >&2"], Policy::Trusted).unlimited(),
            )
            .unwrap();
        let elapsed = started.elapsed();
        kill(&output.text());
        assert_eq!(output.code, 0);
        assert_eq!(output.stderr, "done\n");
        assert!(elapsed < AFTERMATH + Duration::from_secs(5), "{elapsed:?}");
    }

    #[test]
    fn a_stopped_call_gives_up_pipes_a_grandchild_in_another_process_group_holds() {
        let scratch = term_base::testing::scratch("git-grandchild");
        std::fs::create_dir_all(scratch.path()).unwrap();
        let pidfile = scratch.join("pid");
        let script = r#"/usr/bin/perl -e 'setpgrp(0, 0); open(my $f, ">", $ARGV[0]) or die; print $f $$; close $f; sleep 30' "$0" & wait"#;
        let cancel = Cancel::new();
        let later = cancel.clone();
        let written = pidfile.clone();
        let canceller = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(10);
            while !std::fs::read_to_string(&written).is_ok_and(|pid| !pid.is_empty()) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(20));
            }
            later.cancel();
        });
        let started = Instant::now();
        let arg = pidfile.to_string_lossy().into_owned();
        let result = shell().run_program(
            Path::new("/bin/sh"),
            Call::new(scratch.path(), ["-c", script, arg.as_str()], Policy::Trusted)
                .unlimited()
                .cancel(&cancel),
        );
        let elapsed = started.elapsed();
        canceller.join().unwrap();
        kill(&std::fs::read_to_string(&pidfile).unwrap_or_default());
        assert!(matches!(result, Err(Error::Cancelled)), "{result:?}");
        assert!(elapsed < Duration::from_secs(10) + AFTERMATH, "{elapsed:?}");
    }

    #[test]
    fn failures_are_sorted_by_what_git_said() {
        let output = |stderr: &str| Output {
            code: 128,
            stdout: Vec::new(),
            stderr: stderr.into(),
            command: "git fetch".into(),
            dir: PathBuf::from("/r"),
        };
        assert!(matches!(
            failure(&output(
                "fatal: could not read Username for 'https://x': terminal prompts disabled"
            )),
            Error::AuthFailed { .. }
        ));
        assert!(matches!(
            failure(&output("ssh: Could not resolve hostname x")),
            Error::Network { .. }
        ));
        assert!(matches!(
            failure(&output(
                "fatal: not a git repository (or any of the parent directories): .git"
            )),
            Error::NotARepository { .. }
        ));
        assert!(matches!(
            failure(&output("fatal: something else")),
            Error::Failed { code: Some(128), .. }
        ));
    }
}
