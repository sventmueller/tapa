//! What can go wrong, sorted by what the app does about it.

use std::fmt;
use std::io;
use std::path::PathBuf;

use crate::remote::PushedRef;

/// The result of a git operation.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a git operation failed. `output` fields hold the end of what git and its hooks printed.
#[derive(Debug)]
pub enum Error {
    /// No `git` program was found, or macOS's `/usr/bin/git` stands in for Command Line Tools that are not installed.
    GitMissing,
    /// The folder is not inside a git repository.
    NotARepository {
        /// The folder.
        path: PathBuf,
    },
    /// The repository has no working tree.
    BareRepository {
        /// The repository.
        path: PathBuf,
    },
    /// git refuses the repository because another user owns it (`safe.directory`).
    UnsafeRepository {
        /// The repository.
        path: PathBuf,
        /// What git said.
        output: String,
    },
    /// The remote did not accept the credentials, or there were none and prompting is off.
    AuthFailed {
        /// What git said.
        output: String,
    },
    /// The remote could not be reached.
    Network {
        /// What git said.
        output: String,
    },
    /// The operation stopped at merge conflicts in these files; the repository is left mid-merge or mid-rebase.
    Conflicts {
        /// The files with conflicts, relative to the repository's root.
        paths: Vec<PathBuf>,
        /// What git said.
        output: String,
    },
    /// Local changes would be overwritten, so git did nothing.
    LocalChanges {
        /// The files git named, relative to the repository's root; empty when git named none.
        paths: Vec<PathBuf>,
        /// What git said.
        output: String,
    },
    /// Nothing is staged, so a commit would be empty.
    NothingToCommit,
    /// The commit message is empty.
    EmptyMessage,
    /// A hook refused the operation.
    HookFailed {
        /// The hook's name, such as `pre-commit`.
        hook: String,
        /// What the hook printed; git sends a hook's standard output to its standard error.
        output: String,
    },
    /// The branch and its upstream have diverged and only a fast-forward was allowed.
    NotFastForward {
        /// What git said.
        output: String,
    },
    /// The branch has no upstream to pull from or push to.
    NoUpstream {
        /// What git said.
        output: String,
    },
    /// The remote rejected some of the refs.
    PushRejected {
        /// Every ref of the push, including the rejected ones.
        refs: Vec<PushedRef>,
        /// What git said.
        output: String,
    },
    /// The branch holds commits no other branch has, so deleting it would lose them.
    NotMerged {
        /// The branch.
        branch: String,
        /// What git said.
        output: String,
    },
    /// The file changed since its diff was read, so the hunk no longer fits.
    PatchDoesNotApply {
        /// What git said.
        output: String,
    },
    /// The file is binary, so it has no hunks.
    Binary {
        /// The file.
        path: PathBuf,
    },
    /// These files are untracked, so discarding them deletes them; the app moves them to the Trash instead.
    Untracked {
        /// The untracked files, relative to the repository's root.
        paths: Vec<PathBuf>,
    },
    /// A name that git does not take for a branch, or a thread name that cannot be part of a ref.
    InvalidName {
        /// The name.
        name: String,
    },
    /// A path outside the repository or folder.
    OutsideFolder {
        /// The path.
        path: PathBuf,
    },
    /// A stash entry is no longer at the place it was listed at, as after a stash made or dropped elsewhere.
    StashMoved,
    /// The caller cancelled the operation, or the process was stopped.
    Cancelled,
    /// The operation took longer than its limit and was stopped.
    TimedOut,
    /// git printed more than this crate keeps.
    OutputTooLarge,
    /// git failed for a reason without a variant of its own.
    Failed {
        /// The git command, such as `git commit`.
        command: String,
        /// The exit code; `None` when a signal ended git.
        code: Option<i32>,
        /// What git said.
        output: String,
    },
    /// git printed something this crate does not understand.
    Unexpected {
        /// What it was.
        what: String,
    },
    /// In an untrusted workspace, a configuration file git would read could not be checked, so git did not run.
    UncheckedConfig {
        /// The file.
        path: PathBuf,
        /// Why it could not be checked.
        why: &'static str,
    },
    /// Reading or writing a file failed.
    Io(io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::GitMissing => write!(f, "git is not installed"),
            Error::NotARepository { path } => write!(f, "{} is not in a git repository", path.display()),
            Error::BareRepository { path } => write!(f, "{} is a bare repository", path.display()),
            Error::UnsafeRepository { path, .. } => write!(f, "git does not trust {}", path.display()),
            Error::AuthFailed { .. } => write!(f, "the remote did not accept the credentials"),
            Error::Network { .. } => write!(f, "the remote could not be reached"),
            Error::Conflicts { paths, .. } => write!(f, "conflicts in {} files", paths.len()),
            Error::LocalChanges { .. } => write!(f, "local changes would be overwritten"),
            Error::NothingToCommit => write!(f, "nothing to commit"),
            Error::EmptyMessage => write!(f, "the commit message is empty"),
            Error::HookFailed { hook, .. } => write!(f, "the {hook} hook failed"),
            Error::NotFastForward { .. } => write!(f, "the branch cannot be fast-forwarded"),
            Error::NoUpstream { .. } => write!(f, "the branch has no upstream"),
            Error::PushRejected { .. } => write!(f, "the remote rejected the push"),
            Error::NotMerged { branch, .. } => write!(f, "{branch} is not merged"),
            Error::PatchDoesNotApply { .. } => write!(f, "the change no longer applies"),
            Error::Binary { path } => write!(f, "{} is binary", path.display()),
            Error::Untracked { paths } => write!(f, "{} files are untracked", paths.len()),
            Error::InvalidName { name } => write!(f, "{name:?} is not a valid name"),
            Error::OutsideFolder { path } => write!(f, "{} is outside the folder", path.display()),
            Error::StashMoved => write!(f, "the stash changed since it was read"),
            Error::Cancelled => write!(f, "cancelled"),
            Error::TimedOut => write!(f, "git took too long"),
            Error::OutputTooLarge => write!(f, "git printed too much"),
            Error::Failed { command, code, output } => match code {
                Some(code) => write!(f, "{command} failed ({code}): {}", output.trim()),
                None => write!(f, "{command} was ended: {}", output.trim()),
            },
            Error::Unexpected { what } => write!(f, "unexpected output from git: {what}"),
            Error::UncheckedConfig { path, why } => write!(f, "{}: {why}", path.display()),
            Error::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Error {
        Error::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as _;

    use super::*;
    use crate::remote::PushFlag;

    #[test]
    fn each_failure_of_the_repository_or_the_remote_names_what_it_concerns() {
        let path = || PathBuf::from("src/main.rs");
        let output = || "fatal: details\n".to_string();
        let cases = [
            (Error::GitMissing, "git is not installed"),
            (
                Error::NotARepository { path: path() },
                "src/main.rs is not in a git repository",
            ),
            (
                Error::BareRepository { path: path() },
                "src/main.rs is a bare repository",
            ),
            (
                Error::UnsafeRepository {
                    path: path(),
                    output: output(),
                },
                "git does not trust src/main.rs",
            ),
            (
                Error::AuthFailed { output: output() },
                "the remote did not accept the credentials",
            ),
            (Error::Network { output: output() }, "the remote could not be reached"),
            (
                Error::Conflicts {
                    paths: vec![path(), path()],
                    output: output(),
                },
                "conflicts in 2 files",
            ),
            (
                Error::LocalChanges {
                    paths: Vec::new(),
                    output: output(),
                },
                "local changes would be overwritten",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected, "{error:?}");
            assert!(error.source().is_none(), "{error:?}");
        }
    }

    #[test]
    fn each_refusal_of_the_working_tree_or_a_command_says_what_was_refused() {
        let path = || PathBuf::from("src/main.rs");
        let output = || "fatal: details\n".to_string();
        let rejected = PushedRef {
            flag: PushFlag::Rejected,
            from: "refs/heads/main".into(),
            to: "refs/heads/main".into(),
            summary: "[rejected] (fetch first)".into(),
        };
        let cases = [
            (Error::NothingToCommit, "nothing to commit"),
            (Error::EmptyMessage, "the commit message is empty"),
            (
                Error::HookFailed {
                    hook: "pre-commit".into(),
                    output: output(),
                },
                "the pre-commit hook failed",
            ),
            (
                Error::NotFastForward { output: output() },
                "the branch cannot be fast-forwarded",
            ),
            (Error::NoUpstream { output: output() }, "the branch has no upstream"),
            (
                Error::PushRejected {
                    refs: vec![rejected],
                    output: output(),
                },
                "the remote rejected the push",
            ),
            (
                Error::NotMerged {
                    branch: "topic".into(),
                    output: output(),
                },
                "topic is not merged",
            ),
            (
                Error::PatchDoesNotApply { output: output() },
                "the change no longer applies",
            ),
            (Error::Binary { path: path() }, "src/main.rs is binary"),
            (Error::Untracked { paths: vec![path()] }, "1 files are untracked"),
            (
                Error::InvalidName { name: "a..b".into() },
                "\"a..b\" is not a valid name",
            ),
            (
                Error::OutsideFolder { path: path() },
                "src/main.rs is outside the folder",
            ),
            (Error::StashMoved, "the stash changed since it was read"),
            (Error::Cancelled, "cancelled"),
            (Error::TimedOut, "git took too long"),
            (Error::OutputTooLarge, "git printed too much"),
            (
                Error::Failed {
                    command: "git commit".into(),
                    code: Some(128),
                    output: output(),
                },
                "git commit failed (128): fatal: details",
            ),
            (
                Error::Failed {
                    command: "git push".into(),
                    code: None,
                    output: output(),
                },
                "git push was ended: fatal: details",
            ),
            (
                Error::Unexpected { what: "a line".into() },
                "unexpected output from git: a line",
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.to_string(), expected, "{error:?}");
            assert!(error.source().is_none(), "{error:?}");
        }
    }

    #[test]
    fn a_file_error_keeps_the_io_error_as_its_source() {
        let error = Error::from(io::Error::new(io::ErrorKind::PermissionDenied, "no access"));
        assert!(matches!(error, Error::Io(_)));
        assert_eq!(error.to_string(), "no access");
        let source = error.source().and_then(|s| s.downcast_ref::<io::Error>());
        assert_eq!(source.map(io::Error::kind), Some(io::ErrorKind::PermissionDenied));
    }
}
