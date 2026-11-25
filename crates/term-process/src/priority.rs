//! The nice values of the processes the supervisor starts: agents in the middle and everything an agent starts, in
//! whatever language, last, so the app stays quick while agents build. Nice values alone set that order: macOS's
//! background state would also keep a process off the performance cores and throttle its disk access on an otherwise
//! idle machine.

use std::collections::HashSet;

use term_base::system::children;

use crate::Kind;

/// The short name of process `pid`.
fn name(pid: u32) -> Option<String> {
    term_base::system::name(libc::c_int::try_from(pid).ok()?)
}

/// The nice value of the commands agents run: the lowest there is.
pub(crate) const AGENT_COMMAND_NICE: i32 = 20;

/// The nice value of the agents, and of the agent programs an adapter starts, whose subagents run inside them.
pub(crate) const AGENT_NICE: i32 = 10;

/// The nice value a process of `kind` starts with, above the app's 0; `None` keeps the app's.
pub(crate) fn nice(kind: Kind) -> Option<i32> {
    match kind {
        Kind::LanguageServer | Kind::Tool => Some(5),
        Kind::Agent => Some(AGENT_NICE),
        Kind::AgentCommand | Kind::Gate => Some(AGENT_COMMAND_NICE),
        Kind::Terminal | Kind::DevServer | Kind::Git | Kind::DebugAdapter | Kind::Test => None,
    }
}

/// Gives process `pid` the nice value `nice`; children it starts afterwards inherit it.
pub(crate) fn lower(pid: u32, nice: i32) {
    // SAFETY: setpriority reads only its arguments; for a process that ended meanwhile it fails and changes nothing.
    unsafe { libc::setpriority(libc::PRIO_PROCESS, pid, nice) };
}

/// The agent programs an adapter starts, which keep [`AGENT_NICE`] where everything else an agent starts gets
/// [`AGENT_COMMAND_NICE`].
const AGENTS: &[&str] = &["claude", "codex", "opencode", "gemini"];

/// The programs that run an agent's commands: shells, and the build tools and language servers agents start on
/// their own.
const COMMANDS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "fish",
    "sandbox-exec",
    "cargo",
    "rustc",
    "rust-analyzer",
    "gopls",
    "go",
    "clang",
    "clang++",
    "cc",
    "c++",
    "ld",
    "swift",
    "swiftc",
    "swift-frontend",
    "xcodebuild",
    "make",
    "cmake",
    "ninja",
    "tsc",
    "javac",
    "gradle",
    "mvn",
];

/// Whether a process named `name` is an agent program an adapter starts.
fn is_agent_program(name: Option<&str>) -> bool {
    name.is_some_and(|name| AGENTS.contains(&name))
}

/// Whether a process named `name` runs an agent's commands.
pub(crate) fn runs_commands(name: &str) -> bool {
    COMMANDS.contains(&name.trim_start_matches('-'))
}

/// Gives every process below one of `agents` [`AGENT_COMMAND_NICE`], whatever it runs, except the agent programs an
/// adapter starts, which get [`AGENT_NICE`]. `lowered` remembers the ones done between calls and loses those that
/// ended.
pub(crate) fn lower_agent_commands(agents: &[u32], lowered: &mut HashSet<u32>) {
    let mut seen = HashSet::new();
    for &agent in agents {
        walk(agent, false, &mut seen, lowered);
    }
    lowered.retain(|pid| seen.contains(pid));
}

fn walk(parent: u32, below_command: bool, seen: &mut HashSet<u32>, lowered: &mut HashSet<u32>) {
    for child in children(parent) {
        if !seen.insert(child) {
            continue;
        }
        let command = below_command || !is_agent_program(name(child).as_deref());
        if lowered.insert(child) {
            lower(child, if command { AGENT_COMMAND_NICE } else { AGENT_NICE });
        }
        walk(child, command, seen, lowered);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;

    fn nice_of(pid: u32) -> i32 {
        // SAFETY: getpriority reads only its arguments.
        unsafe { libc::getpriority(libc::PRIO_PROCESS, pid) }
    }

    #[test]
    fn everything_an_agent_starts_yields_the_most() {
        // A stand-in agent: a helper, and a shell whose `sleep` started before the shell was lowered.
        let mut agent = Command::new("/bin/sh")
            .args(["-c", "/bin/sleep 30 & /bin/zsh -c '/bin/sleep 30 & wait' & wait"])
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = agent.id();
        let started = std::time::Instant::now();
        while started.elapsed() < std::time::Duration::from_secs(5) {
            let below = children(pid);
            let shell = below.iter().copied().find(|&c| name(c).as_deref() == Some("zsh"));
            // A child forked but not yet exec'd still carries its parent's name.
            let sleeps = |pids: &[u32]| pids.iter().any(|&c| name(c).as_deref() == Some("sleep"));
            if sleeps(&below) && shell.is_some_and(|shell| sleeps(&children(shell))) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let mut lowered = HashSet::new();
        lower_agent_commands(&[pid], &mut lowered);
        let below = children(pid);
        let named = |wanted: &str| {
            below
                .iter()
                .copied()
                .find(|&c| name(c).as_deref() == Some(wanted))
                .unwrap()
        };
        let (helper, shell) = (named("sleep"), named("zsh"));
        let command = children(shell)[0];
        assert_eq!(
            [nice_of(helper), nice_of(shell), nice_of(command)],
            [AGENT_COMMAND_NICE; 3],
            "everything, and what it started before, yields the most"
        );
        assert_eq!(lowered.len(), 3);
        // SAFETY: Ends the stand-in's own process group, which the test started.
        unsafe { libc::killpg(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
        let _ = agent.wait();
        lower_agent_commands(&[pid], &mut lowered);
        assert!(lowered.is_empty(), "ended processes are forgotten");
    }

    #[test]
    fn only_agent_programs_keep_the_agents_nice_value() {
        assert!(is_agent_program(Some("claude")) && is_agent_program(Some("codex")));
        assert!(!is_agent_program(Some("node")) && !is_agent_program(Some("python3")) && !is_agent_program(None));
    }

    #[test]
    fn language_servers_yield_a_little_and_agent_commands_most() {
        assert_eq!(nice(Kind::LanguageServer), Some(5));
        assert_eq!(nice(Kind::AgentCommand), Some(AGENT_COMMAND_NICE));
        assert_eq!(
            nice(Kind::Gate),
            Some(AGENT_COMMAND_NICE),
            "gates yield like agents' commands (F70)"
        );
        assert_eq!(nice(Kind::Agent), Some(AGENT_NICE));
        assert_eq!(nice(Kind::Terminal), None);
        assert!(runs_commands("-zsh") && runs_commands("cargo") && !runs_commands("claude"));
    }
}
