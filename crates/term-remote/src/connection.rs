//! One master connection per remote workspace (E25): `ssh -M -N` keeps it open and every terminal and agent of the
//! workspace goes through its socket, so the user answers ssh's questions once. Its end is the workspace's
//! disconnection.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use term_base::LockIgnoringPoison;
use term_process::tail::{self, Tail};
use term_process::{Kind, ProcessId, Supervisor};

use crate::script::agent_script;
use crate::{SSH, Target};

/// How often the connection is looked at while it opens.
const CHECK_EVERY: Duration = Duration::from_millis(200);
/// How much of ssh's own messages is kept, for the reason of an end.
const MESSAGES: usize = 8 * 1024;

/// Where a connection is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Not opened yet, or closed by the user.
    Closed,
    /// ssh connects and may be asking questions.
    Connecting,
    /// Terminals and agents can start.
    Connected,
    /// It was open and ended, with ssh's last message.
    Disconnected(String),
    /// It did not open, with ssh's last message.
    Failed(String),
}

/// What a look at the machine found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Probe {
    /// The home folder there.
    pub home: Option<String>,
    /// The programs asked about that are installed there.
    pub installed: Vec<String>,
}

impl Probe {
    /// What `output` of the probe's script says about `programs`.
    fn read(output: &str, programs: &[&str]) -> Probe {
        let mut probe = Probe::default();
        for line in output.lines() {
            match line.strip_prefix("HOME=") {
                Some(home) if home.starts_with('/') => probe.home = Some(home.to_string()),
                Some(_) => {}
                None if programs.contains(&line) => probe.installed.push(line.to_string()),
                None => {}
            }
        }
        probe
    }
}

/// The master connection of one remote workspace.
pub struct Connection {
    target: Target,
    control: PathBuf,
    askpass: Vec<(String, String)>,
    /// Options before all others, such as a configuration file of a test's own.
    options: Vec<String>,
    state: Mutex<State>,
    master: Mutex<Option<ProcessId>>,
    /// Counts the openings, so that a watcher of an earlier one stays quiet.
    opening: AtomicU64,
}

impl Connection {
    /// The connection to `target` through the socket at `control`, with `askpass` the variables that have ssh ask
    /// through the app (`askpass::environment`). The socket's path must stay under the 104 bytes macOS allows (R25).
    pub fn new(target: Target, control: PathBuf, askpass: Vec<(String, String)>) -> Arc<Connection> {
        Connection::with_options(target, control, askpass, Vec::new())
    }

    /// The connection `new` makes, with `options` before ssh's others, such as `-F` and a configuration file.
    pub fn with_options(
        target: Target,
        control: PathBuf,
        askpass: Vec<(String, String)>,
        options: Vec<String>,
    ) -> Arc<Connection> {
        Arc::new(Connection {
            target,
            control,
            askpass,
            options,
            state: Mutex::new(State::Closed),
            master: Mutex::new(None),
            opening: AtomicU64::new(0),
        })
    }

    /// The machine and folder it reaches.
    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Where it is now.
    pub fn state(&self) -> State {
        self.state.lock_ignoring_poison().clone()
    }

    /// Opens the master connection unless one is open or opening; `changed` hears every later state on a thread of
    /// the connection's own.
    pub fn open(self: &Arc<Self>, changed: impl Fn(&State) + Send + Sync + 'static) {
        if matches!(self.state(), State::Connecting | State::Connected) {
            return;
        }
        let opening = self.opening.fetch_add(1, Ordering::SeqCst) + 1;
        if !self.check() {
            let _ = std::fs::remove_file(&self.control);
        }
        if let Some(dir) = self.control.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        self.set(State::Connecting, &changed);
        let messages = Arc::new(Mutex::new(Tail::new(MESSAGES)));
        let spawned = Supervisor::global().spawn(
            self.master_command(),
            Kind::Tool,
            format!("ssh {}", self.target.account()),
        );
        let spawned = match spawned {
            Ok(spawned) => spawned,
            Err(error) => {
                self.set(State::Failed(error.to_string()), &changed);
                return;
            }
        };
        if let Some(stderr) = spawned.stderr {
            let _ = tail::collect(stderr, messages.clone());
        }
        *self.master.lock_ignoring_poison() = Some(spawned.id);
        let this = self.clone();
        let watched = std::thread::Builder::new()
            .name("remote-connection".into())
            .spawn(move || this.watch(spawned.id, opening, &messages, &changed));
        if watched.is_err() {
            Supervisor::global().stop(spawned.id, Duration::ZERO);
        }
    }

    /// Ends the master connection, and with it every terminal and agent that went through it.
    pub fn close(&self) {
        self.opening.fetch_add(1, Ordering::SeqCst);
        let _ = self.control_command("exit").status();
        if let Some(id) = self.master.lock_ignoring_poison().take() {
            Supervisor::global().stop(id, Duration::from_millis(500));
        }
        *self.state.lock_ignoring_poison() = State::Closed;
    }

    /// The options every command through the connection takes: its socket, never a master of its own, the port.
    pub fn client_options(&self) -> Vec<String> {
        let mut options = self.options.clone();
        options.extend([
            "-S".to_string(),
            self.control.to_string_lossy().into_owned(),
            "-o".into(),
            "ControlMaster=no".into(),
        ]);
        options.extend(self.target.port_args());
        options
    }

    /// The arguments of `ssh` for a terminal that runs `command` there with a terminal of its own.
    pub fn terminal_args(&self, command: &str) -> Vec<String> {
        let mut args = vec!["-t".to_string()];
        args.extend(self.client_options());
        args.extend(["--".to_string(), self.target.destination(), command.to_string()]);
        args
    }

    /// `script` run in `/bin/sh` there, without a terminal, its three pipes ready for the caller.
    pub fn command(&self, script: &str) -> Command {
        let mut command = self.forward().command(script);
        command.envs(self.askpass.iter().map(|(name, value)| (name, value)));
        command
    }

    /// What a stand-in needs to start a program through this connection.
    pub fn forward(&self) -> crate::Forward {
        crate::Forward {
            target: self.target.clone(),
            control: self.control.clone(),
            options: self.options.clone(),
        }
    }

    /// The local program and arguments of a terminal: `/bin/sh` waits until the connection is open, saying
    /// `waiting` once, then hands over to `ssh -t` with `command` for the shell there.
    pub fn terminal_program(&self, command: &str, waiting: &str) -> (PathBuf, Vec<String>) {
        let quoted = |args: Vec<String>| {
            args.iter()
                .map(|arg| crate::script::quote(arg))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let mut check = self.options.clone();
        check.extend([
            "-S".to_string(),
            self.control.to_string_lossy().into_owned(),
            "-O".into(),
            "check".into(),
        ]);
        check.extend(self.target.port_args());
        check.extend(["--".to_string(), self.target.destination()]);
        let script = format!(
            "w=0;while ! {ssh} {check} 2>/dev/null;do [ $w = 0 ]&&printf '%s\\r\\n' {waiting};w=1;sleep 0.3;done;exec {ssh} {open}",
            ssh = SSH,
            check = quoted(check),
            waiting = crate::script::quote(waiting),
            open = quoted(self.terminal_args(command)),
        );
        (PathBuf::from("/bin/sh"), vec!["-c".into(), script])
    }

    /// `local`, a program as it would start on this Mac, started there instead: by its name in the `PATH` there, with
    /// its arguments, in the target's folder, with only the variables named in `keep` (R24).
    pub fn wrap(&self, local: &Command, keep: &[&str]) -> Command {
        let program = Path::new(local.get_program())
            .file_name()
            .map_or_else(|| local.get_program().to_string_lossy(), |name| name.to_string_lossy())
            .into_owned();
        let args: Vec<String> = local.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
        let env: Vec<(String, String)> = local
            .get_envs()
            .filter_map(|(name, value)| {
                let name = name.to_string_lossy();
                let value = value?.to_string_lossy().into_owned();
                keep.contains(&name.as_ref()).then(|| (name.into_owned(), value))
            })
            .collect();
        self.command(&agent_script(&self.target.folder, &program, &args, &env))
    }

    /// The home folder there and which of `programs` the `PATH` there has, as an agent started through `wrap` finds
    /// them.
    pub fn probe(&self, programs: &[&str]) -> Probe {
        let mut script = String::from("printf 'HOME=%s\\n' \"$HOME\";for p in");
        for program in programs {
            script.push(' ');
            script.push_str(&crate::script::quote(program));
        }
        script.push_str(";do command -v \"$p\" >/dev/null 2>&1&&echo \"$p\";done");
        let probe = agent_script("", "/bin/sh", &["-c".into(), script], &[]);
        let Ok(output) = self.command(&probe).stdin(Stdio::null()).output() else {
            return Probe::default();
        };
        Probe::read(&String::from_utf8_lossy(&output.stdout), programs)
    }

    fn master_command(&self) -> Command {
        let mut command = Command::new(SSH);
        command
            .args(&self.options)
            .args(["-M", "-N", "-S"])
            .arg(&self.control)
            .args([
                "-o",
                "ControlPersist=no",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
            ])
            .args(["-o", "ConnectTimeout=20"])
            .args(self.target.port_args())
            .args(["--", &self.target.destination()])
            .envs(self.askpass.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        command
    }

    fn control_command(&self, operation: &str) -> Command {
        let mut command = Command::new(SSH);
        command
            .args(&self.options)
            .arg("-S")
            .arg(&self.control)
            .args(["-O", operation])
            .args(self.target.port_args())
            .args(["--", &self.target.destination()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        command
    }

    /// Whether a master answers at the socket.
    fn check(&self) -> bool {
        self.control.exists()
            && self
                .control_command("check")
                .status()
                .is_ok_and(|status| status.success())
    }

    fn set(&self, state: State, changed: &impl Fn(&State)) {
        *self.state.lock_ignoring_poison() = state.clone();
        changed(&state);
    }

    fn watch(&self, id: ProcessId, opening: u64, messages: &Mutex<Tail>, changed: &impl Fn(&State)) {
        let current = || self.opening.load(Ordering::SeqCst) == opening;
        let mut connected = false;
        loop {
            let wait = if connected { Duration::from_secs(5) } else { CHECK_EVERY };
            if Supervisor::global().wait(id, wait).is_some() {
                break;
            }
            if !current() {
                return;
            }
            if !connected && self.check() {
                connected = true;
                self.set(State::Connected, changed);
            }
        }
        if !current() {
            return;
        }
        *self.master.lock_ignoring_poison() = None;
        let message = last_message(&messages.lock_ignoring_poison().text());
        self.set(
            if connected {
                State::Disconnected(message)
            } else {
                State::Failed(message)
            },
            changed,
        );
    }
}

/// The last line ssh wrote, which says why it ended.
fn last_message(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection() -> Arc<Connection> {
        Connection::new(
            Target {
                host: "box".into(),
                port: Some(2222),
                user: Some("sven".into()),
                folder: "~/app".into(),
            },
            PathBuf::from("/tmp/c"),
            Vec::new(),
        )
    }

    #[test]
    fn every_command_goes_through_the_socket_to_the_destination() {
        let connection = connection();
        assert_eq!(
            connection.terminal_args("x"),
            [
                "-t",
                "-S",
                "/tmp/c",
                "-o",
                "ControlMaster=no",
                "-p",
                "2222",
                "--",
                "sven@box",
                "x"
            ]
        );
        let mut local = Command::new("/Users/me/.local/bin/claude");
        local
            .args(["--output-format", "stream-json"])
            .env("SECRET_KEY", "x")
            .env("CLAUDE_CODE_ENTRYPOINT", "tapa");
        let wrapped = connection.wrap(&local, &["CLAUDE_CODE_ENTRYPOINT"]);
        let args: Vec<String> = wrapped.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            &args[..9],
            [
                "-T",
                "-o",
                "BatchMode=yes",
                "-S",
                "/tmp/c",
                "-o",
                "ControlMaster=no",
                "-p",
                "2222"
            ]
        );
        assert_eq!(args[10], "sven@box");
        let encoded = args[11].split("printf %s ").nth(1).unwrap().split('|').next().unwrap();
        let output = Command::new("/bin/sh")
            .args(["-c", &format!("printf %s {encoded}|base64 -D")])
            .output()
            .unwrap();
        let script = String::from_utf8_lossy(&output.stdout);
        assert!(script.contains("cd \"$HOME\"/'app'||exit 1;"), "{script}");
        assert!(script.ends_with("exec env 'CLAUDE_CODE_ENTRYPOINT=tapa' 'claude' '--output-format' 'stream-json'"));
        assert!(!script.contains("SECRET_KEY"), "only the variables kept travel (R24)");
    }

    #[test]
    fn a_terminal_waits_for_the_connection_then_opens_the_shell_there() {
        let connection = connection();
        let (program, args) = connection.terminal_program("exec /bin/sh -c 'x'", "Verbinde mit sven@box …");
        assert_eq!(program, PathBuf::from("/bin/sh"));
        assert_eq!(args[0], "-c");
        let script = &args[1];
        assert!(
            script.contains("'-S' '/tmp/c' '-O' 'check' '-p' '2222' '--' 'sven@box'"),
            "{script}"
        );
        assert!(
            script.ends_with(r#"'--' 'sven@box' 'exec /bin/sh -c '\''x'\'''"#),
            "{script}"
        );
        assert!(script.contains("'Verbinde mit sven@box …'"));
    }

    #[test]
    fn the_probe_reads_the_home_folder_and_the_programs_installed() {
        let probe = Probe::read(
            "HOME=/home/sven\nclaude\nmotd line\nopencode\n",
            &["claude", "codex", "opencode"],
        );
        assert_eq!(probe.home.as_deref(), Some("/home/sven"));
        assert_eq!(probe.installed, ["claude", "opencode"]);
        assert_eq!(Probe::read("HOME=\n", &["claude"]), Probe::default());
    }

    #[test]
    fn the_last_line_of_ssh_says_why_it_ended() {
        assert_eq!(
            last_message("Warning: x\nsven@box: Permission denied (publickey,password).\n\n"),
            "sven@box: Permission denied (publickey,password)."
        );
        assert_eq!(last_message(""), "");
    }
}
