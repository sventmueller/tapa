//! How tasks start programs: `System` runs them; tests hand in `fake::Fake`, which starts nothing, answers in their
//! place and keeps the list of what it was asked to run.

use std::process::{Command, ExitStatus, Output, Stdio};

/// What starts the programs of a task.
pub(crate) trait Shell {
    /// Runs `cmd` with the terminal as its output and waits for its exit.
    fn status(&mut self, cmd: &mut Command) -> std::io::Result<ExitStatus>;

    /// Runs `cmd` and collects its standard output, and its standard error unless `cmd` sends that elsewhere.
    fn output(&mut self, cmd: &mut Command) -> std::io::Result<Output>;

    /// Runs `cmd`; an error names it, and its exit status if it ran.
    fn run(&mut self, cmd: &mut Command) -> Result<(), String> {
        let status = self.status(cmd).map_err(|e| format!("{cmd:?}: {e}"))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("{cmd:?} failed with {status}"))
        }
    }

    /// What `cmd` printed, with its standard error on the terminal; an error when it failed.
    fn stdout(&mut self, cmd: &mut Command) -> Result<String, String> {
        let out = self
            .output(cmd.stderr(Stdio::inherit()))
            .map_err(|e| format!("{cmd:?}: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(format!("{cmd:?} failed with {}", out.status))
        }
    }
}

/// The shell that runs the programs on this Mac.
pub(crate) struct System;

impl Shell for System {
    fn status(&mut self, cmd: &mut Command) -> std::io::Result<ExitStatus> {
        cmd.status()
    }

    fn output(&mut self, cmd: &mut Command) -> std::io::Result<Output> {
        cmd.output()
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use std::ffi::OsStr;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::{Command, ExitStatus, Output};

    use super::Shell;

    /// A command as one line: the program and its arguments, separated by spaces.
    pub(crate) fn line(cmd: &Command) -> String {
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|part| part.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The value `cmd` sets for the environment variable `key`, if it sets one.
    pub(crate) fn env(cmd: &Command, key: &str) -> Option<String> {
        cmd.get_envs()
            .find(|(name, _)| *name == key)
            .and_then(|(_, value)| value)
            .map(|value| value.to_string_lossy().into_owned())
    }

    /// The argument after `flag`, or the last one without a flag.
    pub(crate) fn arg(cmd: &Command, flag: Option<&str>) -> PathBuf {
        let args: Vec<&OsStr> = cmd.get_args().collect();
        let found = match flag {
            Some(flag) => args.iter().position(|a| *a == flag).and_then(|at| args.get(at + 1)),
            None => args.last(),
        };
        PathBuf::from(found.expect("the command has the argument"))
    }

    /// An empty folder of its own for the test `name`.
    pub(crate) fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// What a fake program does: its exit code and what it prints.
    pub(crate) type Answer = (i32, String);

    /// A shell that runs nothing: `answer` stands in for each program, given the command and its line, and may leave
    /// the files behind the real program would.
    pub(crate) struct Fake<F> {
        /// Every command it was handed, by `line`.
        pub(crate) commands: Vec<String>,
        answer: F,
    }

    impl<F: FnMut(&Command, &str) -> Answer> Fake<F> {
        pub(crate) fn new(answer: F) -> Self {
            Fake {
                commands: Vec::new(),
                answer,
            }
        }

        fn answer(&mut self, cmd: &Command) -> Answer {
            let line = line(cmd);
            let answer = (self.answer)(cmd, &line);
            self.commands.push(line);
            answer
        }

        /// The commands whose program is `program`.
        pub(crate) fn of(&self, program: &str) -> Vec<&str> {
            self.commands
                .iter()
                .map(String::as_str)
                .filter(|line| line.split(' ').next() == Some(program))
                .collect()
        }
    }

    fn exit(code: i32) -> ExitStatus {
        ExitStatus::from_raw(code << 8)
    }

    impl<F: FnMut(&Command, &str) -> Answer> Shell for Fake<F> {
        fn status(&mut self, cmd: &mut Command) -> std::io::Result<ExitStatus> {
            Ok(exit(self.answer(cmd).0))
        }

        fn output(&mut self, cmd: &mut Command) -> std::io::Result<Output> {
            let (code, printed) = self.answer(cmd);
            Ok(Output {
                status: exit(code),
                stdout: printed.into_bytes(),
                stderr: Vec::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    #[test]
    fn a_failing_program_is_an_error_naming_the_command_and_its_status() {
        let mut sh = Fake::new(|_: &Command, line: &str| match line {
            "true" => (0, "printed\n".into()),
            _ => (3, String::new()),
        });
        assert_eq!(sh.run(&mut Command::new("true")), Ok(()));
        assert_eq!(sh.stdout(&mut Command::new("true")).unwrap(), "printed\n");
        let failed = sh.run(Command::new("false").arg("-x")).unwrap_err();
        assert!(
            failed.contains("\"false\" \"-x\"") && failed.ends_with("exit status: 3"),
            "{failed}"
        );
        assert!(
            sh.stdout(&mut Command::new("false"))
                .unwrap_err()
                .contains("exit status: 3")
        );
        assert_eq!(sh.commands, ["true", "true", "false -x", "false"]);
        assert_eq!(sh.of("false"), ["false -x", "false"]);
    }

    #[test]
    fn the_system_runs_programs_and_reports_those_that_cannot_start() {
        assert_eq!(System.stdout(Command::new("/bin/echo").arg("hi")).unwrap(), "hi\n");
        assert!(
            System
                .run(&mut Command::new("/usr/bin/false"))
                .unwrap_err()
                .contains("failed with")
        );
        let missing = System.run(&mut Command::new("/nonexistent/program")).unwrap_err();
        assert!(missing.starts_with("\"/nonexistent/program\": "), "{missing}");
        assert!(System.stdout(&mut Command::new("/nonexistent/program")).is_err());
    }

    #[test]
    fn a_line_shows_the_program_its_arguments_and_reads_its_environment() {
        let mut cmd = Command::new("cargo");
        cmd.args(["build", "--release"])
            .env("GGML_NATIVE", "OFF")
            .env_remove("RUSTC");
        assert_eq!(line(&cmd), "cargo build --release");
        assert_eq!(env(&cmd, "GGML_NATIVE").as_deref(), Some("OFF"));
        assert_eq!(env(&cmd, "RUSTC"), None);
        assert_eq!(env(&cmd, "PATH"), None);
    }
}
