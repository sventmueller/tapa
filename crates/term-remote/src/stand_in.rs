//! Agents of a remote workspace (E25): a folder of stand-ins, one small script per agent program installed there,
//! comes first in the `PATH` the workspace's agents get. The agent crates find, check and start a stand-in as they
//! would the program; the stand-in starts the app's executable in a mode that replaces itself with ssh running the
//! program there, its pipes passed through unchanged.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::script::{agent_script, quote, through_any_shell};
use crate::{SSH, Target};

/// The first argument of the app's executable in the stand-in mode.
pub const MODE: &str = "--tapa-remote";

/// Where and how a stand-in reaches the other machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    /// The machine and folder.
    pub target: Target,
    /// The master connection's socket.
    pub control: PathBuf,
    /// Options before all others.
    #[serde(default)]
    pub options: Vec<String>,
}

impl Forward {
    /// ssh running `script` in `/bin/sh` there through the master connection, without a terminal; it never asks a
    /// question, so a closed connection fails at once instead of waiting for an answer nobody gives.
    pub fn command(&self, script: &str) -> Command {
        let mut command = Command::new(SSH);
        command
            .args(&self.options)
            .args(["-T", "-o", "BatchMode=yes", "-S"])
            .arg(&self.control)
            .args(["-o", "ControlMaster=no"])
            .args(self.target.port_args())
            .args(["--", &self.target.destination(), &through_any_shell(script)])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }
}

/// Whether a variable of the agent's environment may travel to the other machine: the agents' own settings, never
/// what looks like a secret (R24), as the command line there is visible to every user in `ps`.
pub fn travels(name: &str) -> bool {
    const OWN: [&str; 4] = ["CLAUDE_CODE_", "CLAUDE_AGENT_", "CODEX_", "MCP_"];
    const SECRET: [&str; 5] = ["KEY", "TOKEN", "SECRET", "PASSWORD", "AUTH"];
    let upper = name.to_ascii_uppercase();
    OWN.iter().any(|own| upper.starts_with(own)) && !SECRET.iter().any(|secret| upper.contains(secret))
}

/// Writes a stand-in for each of `programs` into `bin`, which starts `app` in the stand-in mode, and the forward they
/// read beside them; earlier stand-ins go.
pub fn write(bin: &Path, app: &Path, forward: &Forward, programs: &[String]) -> io::Result<()> {
    if bin.exists() {
        std::fs::remove_dir_all(bin)?;
    }
    std::fs::create_dir_all(bin)?;
    let spec = bin.join(".forward.json");
    std::fs::write(&spec, serde_json::to_vec(forward).map_err(io::Error::other)?)?;
    for program in programs {
        if program.is_empty() || program.contains('/') || program.starts_with('.') {
            continue;
        }
        let path = bin.join(program);
        let script = format!(
            "#!/bin/sh\nexec {} {MODE} {} {} \"$@\"\n",
            quote(&app.to_string_lossy()),
            quote(&spec.to_string_lossy()),
            quote(program)
        );
        std::fs::write(&path, script)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// The stand-in mode: `args` after `MODE` are the forward's file, the program and its arguments. Replaces this
/// process with ssh running the program in the target's folder there, with the variables of this process that
/// travel; returns the exit status only when that fails.
pub fn run(args: &[String]) -> i32 {
    let [spec, program, rest @ ..] = args else {
        eprintln!("tapa: a stand-in needs its forward and program");
        return 2;
    };
    let forward: Forward = match std::fs::read(spec)
        .map_err(|e| e.to_string())
        .and_then(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string()))
    {
        Ok(forward) => forward,
        Err(error) => {
            eprintln!("tapa: {spec}: {error}");
            return 2;
        }
    };
    let env: Vec<(String, String)> = std::env::vars().filter(|(name, _)| travels(name)).collect();
    let script = agent_script(&forward.target.folder, program, rest, &env);
    let mut command = forward.command(&script);
    command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    let error = command.exec();
    eprintln!("tapa: ssh: {error}");
    127
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_agents_own_settings_travel() {
        assert!(travels("CLAUDE_CODE_ENTRYPOINT"));
        assert!(travels("CODEX_HOME"));
        assert!(!travels("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(!travels("ANTHROPIC_API_KEY"));
        assert!(!travels("PATH"));
        assert!(!travels("MCP_SECRET_X"));
    }

    #[test]
    fn a_stand_in_starts_the_app_in_its_mode_with_the_programs_arguments() {
        let dir = term_base::testing::scratch("remote-stand-in");
        let app = dir.join("fake app");
        std::fs::write(&app, "#!/bin/sh\nprintf '%s|' \"$@\"\n").unwrap();
        std::fs::set_permissions(&app, std::fs::Permissions::from_mode(0o755)).unwrap();
        let bin = dir.join("bin");
        let forward = Forward {
            target: Target {
                host: "box".into(),
                ..Target::default()
            },
            control: dir.join("c"),
            options: Vec::new(),
        };
        write(&bin, &app, &forward, &["claude".into(), "../evil".into()]).unwrap();
        assert!(!bin.join("../evil").exists());
        let output = Command::new(bin.join("claude"))
            .args(["--print", "a b"])
            .output()
            .unwrap();
        let shown = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            shown,
            format!("{MODE}|{}|claude|--print|a b|", bin.join(".forward.json").display())
        );
        let read: Forward = serde_json::from_slice(&std::fs::read(bin.join(".forward.json")).unwrap()).unwrap();
        assert_eq!(read, forward);
    }
}
