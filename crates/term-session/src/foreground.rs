//! What runs in front in a terminal: a coding agent, a remote session, the shell itself or some other program.

use std::fs::File;

use crate::sys;

/// A coding agent's command-line interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Agent {
    /// Claude Code.
    Claude,
    /// OpenAI's Codex CLI.
    Codex,
    /// Google's Gemini CLI.
    Gemini,
    /// Google Antigravity's `agy`.
    Antigravity,
    /// OpenCode.
    OpenCode,
}

/// The program in a terminal's foreground process group, as far as it matters to the app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Foreground {
    /// A coding agent, which reads images from the pasteboard itself.
    Agent(Agent),
    /// A session on another machine, such as `ssh`, named by its program.
    Remote(String),
    /// The shell waits for input; its name, such as `zsh`.
    Shell(String),
    /// Any other program, by name.
    Other(String),
    /// The terminal could not be asked.
    Unknown,
}

/// Agents by the name of their executable.
const AGENT_BINARIES: &[(&str, Agent)] = &[
    ("claude", Agent::Claude),
    ("codex", Agent::Codex),
    ("gemini", Agent::Gemini),
    ("agy", Agent::Antigravity),
    ("opencode", Agent::OpenCode),
];

/// Agents by their npm package, which shows in the arguments when they run under a JavaScript runtime.
const AGENT_PACKAGES: &[(&str, Agent)] = &[
    ("@anthropic-ai/claude-code", Agent::Claude),
    ("@openai/codex", Agent::Codex),
    ("@google/gemini-cli", Agent::Gemini),
    ("opencode-ai", Agent::OpenCode),
];

/// Programs that run an agent's JavaScript: runtimes and package runners.
const RUNTIMES: &[&str] = &["node", "bun", "deno", "npx", "bunx", "pnpx", "npm", "pnpm", "yarn"];

const REMOTE: &[&str] = &["ssh", "mosh", "mosh-client", "et", "autossh"];

const SHELLS: &[&str] = &[
    "zsh", "bash", "fish", "sh", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "elvish", "xonsh", "pwsh",
];

/// The foreground of the terminal whose master side is `master`.
pub(crate) fn of_terminal(master: &File) -> Foreground {
    let Some(group) = sys::foreground_group(master) else {
        return Foreground::Unknown;
    };
    let processes: Vec<Vec<String>> = term_base::system::group_members(group)
        .into_iter()
        .filter_map(|pid| sys::arguments(pid).or_else(|| term_base::system::name(pid).map(|name| vec![name])))
        .collect();
    classify_group(&processes)
}

/// The foreground of a process group from its processes' arguments; an agent anywhere in the group wins, then a
/// remote session, then any program other than a shell.
pub(crate) fn classify_group(processes: &[Vec<String>]) -> Foreground {
    processes
        .iter()
        .map(|argv| classify(argv))
        .max_by_key(rank)
        .unwrap_or(Foreground::Unknown)
}

fn rank(foreground: &Foreground) -> u8 {
    match foreground {
        Foreground::Agent(_) => 4,
        Foreground::Remote(_) => 3,
        Foreground::Other(_) => 2,
        Foreground::Shell(_) => 1,
        Foreground::Unknown => 0,
    }
}

/// What one process is, from its arguments.
pub(crate) fn classify(argv: &[String]) -> Foreground {
    let Some(first) = argv.first() else {
        return Foreground::Unknown;
    };
    let name = program_name(first);
    if let Some(agent) = agent_named(name) {
        return Foreground::Agent(agent);
    }
    if RUNTIMES.contains(&name)
        && let Some(agent) = agent_run_by(&argv[1..])
    {
        return Foreground::Agent(agent);
    }
    if REMOTE.contains(&name) {
        return Foreground::Remote(name.to_string());
    }
    if SHELLS.contains(&name) {
        return Foreground::Shell(name.to_string());
    }
    Foreground::Other(name.to_string())
}

/// The file name of an `argv[0]`, without the dash a login shell carries.
fn program_name(arg0: &str) -> &str {
    let name = arg0.rsplit('/').next().unwrap_or(arg0);
    name.strip_prefix('-').unwrap_or(name)
}

fn agent_named(name: &str) -> Option<Agent> {
    AGENT_BINARIES
        .iter()
        .find(|(binary, _)| *binary == name)
        .map(|(_, agent)| *agent)
}

/// The agent a runtime runs: its package anywhere in the arguments, or its executable as the script.
fn agent_run_by(args: &[String]) -> Option<Agent> {
    let by_package = args.iter().find_map(|arg| {
        AGENT_PACKAGES
            .iter()
            .find(|(package, _)| contains_package(arg, package))
            .map(|(_, agent)| *agent)
    });
    by_package.or_else(|| {
        let script = args.iter().find(|arg| !arg.starts_with('-'))?;
        agent_named(program_name(script))
    })
}

/// Whether `arg` names `package`, as a path into `node_modules`, a package spec or a bare name.
fn contains_package(arg: &str, package: &str) -> bool {
    arg.match_indices(package).any(|(start, _)| {
        let before = arg[..start].chars().next_back();
        let after = arg[start + package.len()..].chars().next();
        matches!(before, None | Some('/' | '=' | ' ' | ':')) && matches!(after, None | Some('/' | '@' | ' ' | ':'))
    })
}

/// The arguments in a `KERN_PROCARGS2` buffer: the argument count, the executable path, padding, then the arguments.
pub(crate) fn parse_procargs(buffer: &[u8]) -> Option<Vec<String>> {
    let count_bytes: [u8; 4] = buffer.get(..4)?.try_into().ok()?;
    let count = usize::try_from(i32::from_ne_bytes(count_bytes)).ok()?;
    let rest = &buffer[4..];
    let path_end = rest.iter().position(|&b| b == 0)?;
    let args_start = path_end + rest[path_end..].iter().position(|&b| b != 0)?;
    let args: Vec<String> = rest[args_start..]
        .split(|&b| b == 0)
        .take(count)
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    (args.len() == count && count > 0).then_some(args)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn native_agent_binaries_are_recognised_by_name() {
        assert_eq!(classify(&argv(&["claude"])), Foreground::Agent(Agent::Claude));
        assert_eq!(
            classify(&argv(&["/Users/me/.local/bin/claude", "--resume"])),
            Foreground::Agent(Agent::Claude)
        );
        assert_eq!(
            classify(&argv(&["/opt/homebrew/bin/codex"])),
            Foreground::Agent(Agent::Codex)
        );
        assert_eq!(
            classify(&argv(&["gemini", "-m", "x"])),
            Foreground::Agent(Agent::Gemini)
        );
        assert_eq!(classify(&argv(&["agy"])), Foreground::Agent(Agent::Antigravity));
        assert_eq!(classify(&argv(&["opencode"])), Foreground::Agent(Agent::OpenCode));
    }

    #[test]
    fn agents_running_under_node_or_a_package_runner_are_recognised_by_their_package_or_script() {
        let cases = [
            (
                vec![
                    "node",
                    "/opt/homebrew/lib/node_modules/@anthropic-ai/claude-code/cli.js",
                ],
                Agent::Claude,
            ),
            (vec!["node", "/opt/homebrew/bin/claude", "--continue"], Agent::Claude),
            (vec!["npx", "-y", "@anthropic-ai/claude-code@latest"], Agent::Claude),
            (
                vec!["node", "/Users/me/.npm/_npx/1/node_modules/.bin/codex"],
                Agent::Codex,
            ),
            (
                vec!["node", "/usr/local/lib/node_modules/@openai/codex/bin/codex.js"],
                Agent::Codex,
            ),
            (vec!["bunx", "@google/gemini-cli"], Agent::Gemini),
            (
                vec!["/opt/homebrew/bin/bun", "/Users/me/.bun/bin/gemini"],
                Agent::Gemini,
            ),
            (vec!["npm", "exec", "--", "opencode-ai"], Agent::OpenCode),
        ];
        for (args, agent) in cases {
            assert_eq!(classify(&argv(&args)), Foreground::Agent(agent), "{args:?}");
        }
    }

    #[test]
    fn a_runtime_running_something_else_is_another_program() {
        assert_eq!(
            classify(&argv(&["node", "server.js", "--name", "claude"])),
            Foreground::Other("node".into())
        );
        assert_eq!(
            classify(&argv(&[
                "node",
                "/app/node_modules/@anthropic-ai/claude-code-tools/x.js"
            ])),
            Foreground::Other("node".into())
        );
        assert_eq!(classify(&argv(&["npx", "vite"])), Foreground::Other("npx".into()));
    }

    #[test]
    fn remote_sessions_shells_and_other_programs_are_told_apart() {
        assert_eq!(classify(&argv(&["ssh", "host"])), Foreground::Remote("ssh".into()));
        assert_eq!(
            classify(&argv(&["mosh-client", "-#", "host"])),
            Foreground::Remote("mosh-client".into())
        );
        assert_eq!(classify(&argv(&["et", "host:8080"])), Foreground::Remote("et".into()));
        assert_eq!(classify(&argv(&["-zsh"])), Foreground::Shell("zsh".into()));
        assert_eq!(
            classify(&argv(&["/bin/bash", "--login"])),
            Foreground::Shell("bash".into())
        );
        assert_eq!(classify(&argv(&["vim", "README.md"])), Foreground::Other("vim".into()));
        assert_eq!(classify(&[]), Foreground::Unknown);
    }

    #[test]
    fn in_a_group_an_agent_beats_its_launcher_and_a_program_beats_the_shell() {
        let group = [
            argv(&["npx", "-y", "something"]),
            argv(&["node", "/x/node_modules/@openai/codex/bin/codex.js"]),
        ];
        assert_eq!(classify_group(&group), Foreground::Agent(Agent::Codex));
        let job = [argv(&["sh", "-c", "make"]), argv(&["make"])];
        assert_eq!(classify_group(&job), Foreground::Other("make".into()));
        assert_eq!(classify_group(&[]), Foreground::Unknown);
    }

    #[test]
    fn procargs_are_read_past_the_executable_path_and_its_padding() {
        let mut buffer = 2i32.to_ne_bytes().to_vec();
        buffer.extend_from_slice(b"/usr/local/bin/node\0\0\0\0node\0/opt/claude\0PATH=/bin\0");
        assert_eq!(parse_procargs(&buffer), Some(argv(&["node", "/opt/claude"])));
        assert_eq!(parse_procargs(&[1, 0]), None);
        let mut short = 3i32.to_ne_bytes().to_vec();
        short.extend_from_slice(b"/bin/x\0x\0");
        assert_eq!(parse_procargs(&short), None);
    }
}
