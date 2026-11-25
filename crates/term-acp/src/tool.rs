//! Short-lived helper processes of an installation, such as `npm install` or `ditto`, run with
//! `term_process::tool` with the login shell's environment and a time limit.

use std::path::{Path, PathBuf};
use std::time::Duration;

use term_process::Supervisor;
use term_process::tool::{self, Helper, Output, RunError};

/// Runs an installation's helpers with the login shell's environment.
#[derive(Clone)]
pub(crate) struct Runner {
    supervisor: Supervisor,
    env: Vec<(String, String)>,
}

impl Runner {
    pub(crate) fn new(supervisor: &Supervisor, env: Vec<(String, String)>) -> Runner {
        Runner {
            supervisor: supervisor.clone(),
            env,
        }
    }

    /// Runs `program` with `args` in `dir`, with `extra` added to the environment, and waits at most `timeout`.
    pub(crate) fn run(
        &self,
        program: &Path,
        args: &[&str],
        dir: &Path,
        extra: &[(String, String)],
        timeout: Duration,
    ) -> std::io::Result<Output> {
        let env: Vec<(String, String)> = self.env.iter().chain(extra).cloned().collect();
        let helper = Helper {
            supervisor: &self.supervisor,
            env: &env,
            timeout,
            cancel: None,
        };
        helper
            .run(program, args, dir, &mut |_| {})
            .map_err(|error| match error {
                RunError::Timeout => std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("{} took too long", program.display()),
                ),
                error => error.into(),
            })
    }

    /// The first executable called `name` on the login shell's PATH.
    pub(crate) fn which(&self, name: &str) -> Option<PathBuf> {
        tool::which(name, tool::var(&self.env, "PATH"))
    }
}
