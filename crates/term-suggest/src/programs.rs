//! How the delayed sources run a program such as `docker` or `kubectl`, and the `PATH` they look for it in: straight
//! from this process by default; an app that keeps its child processes itself installs its own way, as Dango does
//! with its supervisor and the login shell's environment.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The app's way of running the programs the suggestions ask.
pub trait Programs: Send + Sync {
    /// The `PATH` the programs are looked up in.
    fn path(&self) -> Option<OsString>;
    /// What `program` with `args` printed, run in `folder`, if it ended well by `deadline`; it is ended then.
    fn output(&self, program: &Path, args: &[&str], folder: &Path, deadline: Instant) -> Option<String>;
}

static PROGRAMS: OnceLock<&'static dyn Programs> = OnceLock::new();

/// Runs the suggestions' programs through `programs` from now on; the first installation wins.
pub fn install_programs(programs: &'static dyn Programs) {
    let _ = PROGRAMS.set(programs);
}

/// The installed way, else this process's own.
pub(crate) fn programs() -> &'static dyn Programs {
    PROGRAMS.get().copied().unwrap_or(&Direct)
}

/// Programs started by this process with its own `PATH`.
struct Direct;

impl Programs for Direct {
    fn path(&self) -> Option<OsString> {
        std::env::var_os("PATH")
    }

    fn output(&self, program: &Path, args: &[&str], folder: &Path, deadline: Instant) -> Option<String> {
        let mut child = Command::new(program)
            .args(args)
            .current_dir(folder)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let reader = std::thread::spawn(move || {
            let mut out = String::new();
            let _ = stdout.read_to_string(&mut out);
            out
        });
        loop {
            match child.try_wait() {
                Ok(Some(status)) if status.success() => break,
                Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        reader.join().ok()
    }
}
