//! The environment of the user's login shell. An app started from the Finder inherits launchd's, whose PATH lacks
//! Homebrew, rustup, go and npm's folders, so shells started without it would not find them.

use std::ffi::CStr;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Marks the start and end of the environment in the shell's output, which may also hold what its startup files print.
const MARK: &str = "__IDE_ENV_7f3a__";

/// The most the shell may print, startup files included, before it is given up on; the system caps a process's
/// arguments and environment together far below this.
const MOST_PRINTED: usize = 64 * 1024 * 1024;

/// Variables that describe the capturing shell itself, not the user's environment.
const SESSION_ONLY: [&str; 4] = ["SHLVL", "PWD", "OLDPWD", "_"];

/// Why the environment could not be read.
#[derive(Debug)]
pub enum CaptureError {
    /// The shell did not start.
    Start(std::io::Error),
    /// The shell did not finish in time, for example because a startup file waits for input.
    Timeout,
    /// The output held no environment.
    NoEnvironment,
}

/// The user's login shell from the user database, else `/bin/zsh`.
pub fn user_shell() -> PathBuf {
    login_shell()
        .filter(|s| !s.is_empty())
        .map_or_else(|| PathBuf::from("/bin/zsh"), PathBuf::from)
}

/// The shell the user database names for the user, read with `getpwuid_r`, which any thread may call.
fn login_shell() -> Option<String> {
    // SAFETY: getuid only reads the calling process's user id.
    let uid = unsafe { libc::getuid() };
    let mut space = 1024;
    loop {
        let mut buffer = vec![0 as libc::c_char; space];
        // SAFETY: passwd is plain data for which all zero bytes are a valid value.
        let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
        let mut found: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: entry and found are live locals and the buffer holds `space` bytes; getpwuid_r writes the entry's
        // strings into that buffer and nowhere else.
        let status = unsafe { libc::getpwuid_r(uid, &raw mut entry, buffer.as_mut_ptr(), space, &raw mut found) };
        if status == libc::ERANGE && space < 1 << 20 {
            space *= 4;
            continue;
        }
        if status != 0 || found.is_null() || entry.pw_shell.is_null() {
            return None;
        }
        // SAFETY: On success pw_shell points to a string ending in a zero byte within `buffer`, which is still alive.
        return Some(unsafe { CStr::from_ptr(entry.pw_shell) }.to_string_lossy().into_owned());
    }
}

/// The environment `shell` has as an interactive login shell in `dir`.
pub fn capture(shell: &Path, dir: &Path, timeout: Duration) -> Result<Vec<(String, String)>, CaptureError> {
    let script = format!(
        "cd {} 2>/dev/null; printf '%s' '{MARK}'; /usr/bin/env -0; printf '%s' '{MARK}'",
        quoted(dir)
    );
    let mut command = Command::new(shell);
    command
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = command.spawn().map_err(CaptureError::Start)?;
    let group = i32::try_from(child.id()).unwrap_or(0);
    let stop = |child: &mut std::process::Child| {
        crate::system::signal_group(group, libc::SIGKILL);
        let _ = child.wait();
    };
    let Some(mut stdout) = child.stdout.take() else {
        stop(&mut child);
        return Err(CaptureError::NoEnvironment);
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("login-shell-env".into())
        .spawn(move || {
            let mut output = Vec::new();
            let mut buffer = [0u8; 64 * 1024];
            loop {
                match stdout.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => output.extend_from_slice(&buffer[..n]),
                }
                if output.len() > MOST_PRINTED {
                    let _ = tx.send(None);
                    return;
                }
            }
            let _ = tx.send(Some(output));
        });
    if let Err(error) = reader {
        stop(&mut child);
        return Err(CaptureError::Start(error));
    }
    let deadline = Instant::now() + timeout;
    let received = rx.recv_timeout(deadline.saturating_duration_since(Instant::now()));
    match received {
        Ok(Some(output)) => {
            let _ = child.wait();
            parse(&output).ok_or(CaptureError::NoEnvironment)
        }
        Ok(None) => {
            stop(&mut child);
            Err(CaptureError::NoEnvironment)
        }
        Err(_) => {
            stop(&mut child);
            Err(CaptureError::Timeout)
        }
    }
}

/// `path` as one word for a POSIX shell.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

/// The variables between the marks, without those of the shell session itself.
fn parse(output: &[u8]) -> Option<Vec<(String, String)>> {
    let text = String::from_utf8_lossy(output);
    let start = text.find(MARK)? + MARK.len();
    let end = text.rfind(MARK)?;
    if end < start {
        return None;
    }
    let variables: Vec<(String, String)> = text[start..end]
        .split('\0')
        .filter_map(|entry| entry.split_once('='))
        .filter(|(name, _)| !name.is_empty() && !SESSION_ONLY.contains(name))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    (!variables.is_empty()).then_some(variables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_read_between_the_marks_and_survives_noise() {
        let output = format!("motd\n{MARK}PATH=/a:/b\0MULTI=x\ny\0SHLVL=2\0EMPTY=\0junk\0{MARK}bye");
        let parsed = parse(output.as_bytes()).unwrap();
        assert_eq!(
            parsed,
            vec![
                ("PATH".into(), "/a:/b".into()),
                ("MULTI".into(), "x\ny".into()),
                ("EMPTY".into(), String::new()),
            ]
        );
        assert!(parse(b"no marks here").is_none());
    }

    #[test]
    fn a_path_is_quoted_for_the_shell() {
        assert_eq!(quoted(Path::new("/a b/it's")), r"'/a b/it'\''s'");
    }

    #[test]
    fn the_login_shell_gives_a_path() {
        let env = capture(Path::new("/bin/sh"), &std::env::temp_dir(), Duration::from_secs(10)).unwrap();
        assert!(env.iter().any(|(name, value)| name == "PATH" && !value.is_empty()));
    }

    #[test]
    fn a_shell_that_hangs_is_given_up_on() {
        let script_dir = crate::testing::scratch("hang");
        let shell = script_dir.join("hang.sh");
        std::fs::write(&shell, "#!/bin/sh\nsleep 30\n").unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let result = capture(&shell, &script_dir, Duration::from_millis(300));
        assert!(matches!(result, Err(CaptureError::Timeout)));
    }

    #[test]
    fn a_shell_that_prints_without_end_is_given_up_on_before_its_time() {
        let script_dir = crate::testing::scratch("flood");
        let shell = script_dir.join("flood.sh");
        let script = format!("#!/bin/sh\nhead -c {} /dev/zero\nsleep 30\n", MOST_PRINTED + 1024);
        std::fs::write(&shell, script).unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let started = Instant::now();
        let result = capture(&shell, &script_dir, Duration::from_secs(60));
        assert!(matches!(result, Err(CaptureError::NoEnvironment)), "{result:?}");
        assert!(started.elapsed() < Duration::from_secs(30));
    }

    #[test]
    fn an_environment_printed_in_pieces_is_read_whole() {
        let script_dir = crate::testing::scratch("pieces");
        let shell = script_dir.join("slow.sh");
        let script = format!(
            "#!/bin/sh\nprintf '%s' '{MARK}'\nsleep 0.2\nprintf 'A=1\\000'\nsleep 0.2\nprintf 'B=2\\000{MARK}'\n"
        );
        std::fs::write(&shell, script).unwrap();
        std::fs::set_permissions(&shell, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let env = capture(&shell, &script_dir, Duration::from_secs(10)).unwrap();
        assert_eq!(
            env,
            [("A".to_string(), "1".to_string()), ("B".to_string(), "2".to_string())]
        );
    }

    #[test]
    fn the_users_shell_is_a_path() {
        assert!(user_shell().is_absolute());
    }

    #[test]
    fn threads_reading_the_users_shell_at_once_all_read_the_same() {
        let first = login_shell().expect("the user database knows the user running the tests");
        let threads: Vec<_> = (0..16)
            .map(|_| std::thread::spawn(|| (0..200).map(|_| login_shell()).collect::<Vec<_>>()))
            .collect();
        for thread in threads {
            for shell in thread.join().unwrap() {
                assert_eq!(shell.as_ref(), Some(&first));
            }
        }
    }
}
