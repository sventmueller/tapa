//! Command suggestions (E8, F88, F93, F94, F210), all on this Mac. The instant stage answers within the 2 ms a key may
//! take: every command the shell ran with its folder, branch, the command before it and its exit code, kept in SQLite
//! (A5) and in memory, ranked by how often and how lately a command ran, more so in the same folder and branch and
//! after the same command; the subcommands, options and values of the bundled Fig specs; and folder and file names.
//! The delayed stage works in a pool with a time limit: specs not parsed yet, dynamic sources such as git branches,
//! scripts, targets, hosts and containers, and the next command from a typo, a missing program or the project's state.

mod complete;
mod delayed;
mod files;
mod history;
mod next;
mod paths;
mod rank;
mod sources;
mod spec;
mod words;

pub use complete::{Candidate, Completion, Kind, Wanted, complete};
pub use delayed::{TIME_LIMIT, cancel, complete_later, complete_with_sources, next_later};
pub use history::{Entry, History};
pub use next::{Next, Reason, next_without_input};
pub use paths::complete_path;
pub use rank::Context;
pub use spec::{Arg, Opt, Spec, Suggestion};

/// Parses the spec of `command` ahead, as the field does on a pool thread once the first word is typed, so that the
/// instant stage finds it parsed.
pub fn warm_spec(command: &str) {
    if spec::cached(command).is_none() && spec::exists(command) {
        let command = command.to_string();
        let _ = std::thread::Builder::new().name("suggest-spec".into()).spawn(move || {
            spec::load(&command);
        });
    }
}
