//! The program's PTY: started as the leader of a new session with the terminal as its controlling one, and its end
//! watched through the poller.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::sync::Arc;

use polling::os::kqueue::{PollerKqueueExt, Process, ProcessOps};
use polling::{Event, PollMode, Poller};

/// What to start in the PTY.
#[derive(Debug, Clone)]
pub(crate) struct Launch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: PathBuf,
    /// Set on top of the app's own environment.
    pub(crate) env: HashMap<String, String>,
}

/// A program in a PTY. Dropping it hangs up and waits for the program.
pub(crate) struct Pty {
    child: Child,
    file: File,
}

/// Starts `launch` in a new PTY of `size`.
pub(crate) fn spawn(launch: &Launch, size: &libc::winsize) -> io::Result<Pty> {
    let (master, slave) = open(size)?;
    let slave_fd = slave.as_raw_fd();
    let mut command = Command::new(&launch.program);
    command
        .args(&launch.args)
        .envs(&launch.env)
        .stdin(slave.try_clone()?)
        .stderr(slave.try_clone()?)
        .stdout(slave);
    // A folder that went away leaves the program in the app's instead of failing the start, as `chdir` here allows.
    let cwd = CString::new(launch.cwd.as_os_str().as_bytes()).ok();
    // SAFETY: the closure runs between fork and exec and calls only async-signal-safe functions on values made before.
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            if let Some(cwd) = &cwd {
                libc::chdir(cwd.as_ptr());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY.into(), 0) == -1 {
                return Err(io::Error::last_os_error());
            }
            for signal in [
                libc::SIGCHLD,
                libc::SIGHUP,
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGALRM,
            ] {
                libc::signal(signal, libc::SIG_DFL);
            }
            Ok(())
        });
    }
    let child = command.spawn().map_err(|error| {
        io::Error::new(
            error.kind(),
            format!("could not start {}: {error}", launch.program.display()),
        )
    })?;
    let file = File::from(master);
    set_nonblocking(file.as_raw_fd())?;
    Ok(Pty { child, file })
}

/// A new PTY pair, both ends closed on exec, with UTF-8 input so the line editor erases whole characters.
fn open(size: &libc::winsize) -> io::Result<(OwnedFd, OwnedFd)> {
    let (mut master, mut slave): (RawFd, RawFd) = (-1, -1);
    let mut size = *size;
    // SAFETY: openpty writes two descriptors into live values and reads one winsize; name and termios may be null.
    let status = unsafe {
        libc::openpty(
            &raw mut master,
            &raw mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &raw mut size,
        )
    };
    if status != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both descriptors are open and owned by nothing else.
    let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
    for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
        // SAFETY: fcntl changes the flags of a descriptor that `master` or `slave` keeps open.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    // SAFETY: An all-zero termios is a valid value for tcgetattr to overwrite.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: tcgetattr and tcsetattr read and write one live termios for a descriptor `master` keeps open.
    unsafe {
        if libc::tcgetattr(master.as_raw_fd(), &raw mut settings) == 0 {
            settings.c_iflag |= libc::IUTF8;
            libc::tcsetattr(master.as_raw_fd(), libc::TCSANOW, &raw const settings);
        }
    }
    Ok((master, slave))
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    // SAFETY: fcntl reads and changes the status flags of a descriptor the caller keeps open.
    let status = unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL, 0) | libc::O_NONBLOCK) };
    if status == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

impl Pty {
    /// The master side, which reads the program's output and takes its input.
    pub(crate) fn file(&self) -> &File {
        &self.file
    }

    /// The program's process number.
    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// Adds the master side with `interest` and the program's end under `exit_key` to `poller`; `Ok(false)` when the
    /// program ended before it could be watched.
    pub(crate) fn register(&self, poller: &Arc<Poller>, interest: Event, exit_key: usize) -> io::Result<bool> {
        // SAFETY: the reader thread owns both the PTY and the poller and calls `deregister` before either is dropped.
        unsafe { poller.add_with_mode(&self.file, interest, PollMode::Level)? };
        // SAFETY: the child is kept in `self`, which outlives the registration as above.
        let exit = unsafe { Process::new(&self.child, ProcessOps::Exit) };
        match poller.add_filter(exit, exit_key, PollMode::Oneshot) {
            Ok(()) => Ok(true),
            Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Changes what the master side is watched for.
    pub(crate) fn reregister(&self, poller: &Arc<Poller>, interest: Event) -> io::Result<()> {
        poller.modify_with_mode(&self.file, interest, PollMode::Level)
    }

    /// Takes the PTY and the program's end out of `poller`.
    pub(crate) fn deregister(&self, poller: &Arc<Poller>) {
        let _ = poller.delete(&self.file);
        // SAFETY: as in `register`; deleting a filter the kernel already dropped after the exit only fails.
        let exit = unsafe { Process::new(&self.child, ProcessOps::Exit) };
        let _ = poller.delete_filter(exit);
    }

    /// How the program ended, once it has.
    pub(crate) fn exit_status(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        if let (Ok(None), Ok(pid)) = (self.child.try_wait(), i32::try_from(self.child.id()))
            && pid > 0
        {
            // SAFETY: kill only sends a signal to the program, a child of this process that has not been waited for.
            unsafe {
                libc::kill(pid, libc::SIGHUP);
            }
        }
        let _ = self.child.wait();
    }
}
