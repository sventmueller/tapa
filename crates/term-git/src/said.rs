//! git's English words for the failures it gives no other sign of: no exit code, porcelain field or plumbing command
//! tells them apart. `run::prepared` sets `LC_ALL`, so git writes them in English.

use std::path::PathBuf;

use crate::path::{from_bytes, unquoted};

/// A failure git names only in words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Said {
    /// The `git` found is Apple's stand-in, and the developer tools are not installed.
    GitMissing,
    /// The folder is in no repository.
    NotARepository,
    /// The repository belongs to another user, so git refuses it.
    DubiousOwnership,
    /// The remote refused the credentials, or git could not ask for any.
    AuthFailed,
    /// The remote could not be reached.
    Network,
    /// The commit message was empty once git or a hook had cleaned it up.
    EmptyMessage,
    /// Local changes stand in the way of a checkout, merge, pull or stash.
    LocalChanges,
    /// A pull that may only fast-forward cannot.
    NotFastForward,
}

/// The words of each failure, in lower case.
const WORDS: &[(Said, &str)] = &[
    (Said::GitMissing, "xcrun: error"),
    (Said::GitMissing, "invalid active developer path"),
    (Said::GitMissing, "no developer tools were found"),
    (Said::NotARepository, "not a git repository"),
    (Said::DubiousOwnership, "detected dubious ownership"),
    (Said::AuthFailed, "authentication failed"),
    (Said::AuthFailed, "could not read username"),
    (Said::AuthFailed, "could not read password"),
    (Said::AuthFailed, "terminal prompts disabled"),
    (Said::AuthFailed, "permission denied (publickey"),
    (Said::AuthFailed, "invalid username or password"),
    (Said::AuthFailed, "http basic: access denied"),
    (Said::AuthFailed, "returned error: 401"),
    (Said::AuthFailed, "returned error: 403"),
    (Said::AuthFailed, "host key verification failed"),
    (Said::AuthFailed, "repository not found"),
    (Said::Network, "could not resolve host"),
    (Said::Network, "could not resolve hostname"),
    (Said::Network, "unable to access"),
    (Said::Network, "connection refused"),
    (Said::Network, "connection timed out"),
    (Said::Network, "operation timed out"),
    (Said::Network, "network is unreachable"),
    (Said::Network, "no route to host"),
    (Said::Network, "the remote end hung up unexpectedly"),
    (Said::Network, "could not read from remote repository"),
    (Said::Network, "failed to connect"),
    (Said::Network, "early eof"),
    (Said::EmptyMessage, "empty commit message"),
    (Said::LocalChanges, "would be overwritten"),
    (Said::LocalChanges, "please commit your changes or stash them"),
    (Said::LocalChanges, "already exists, no checkout"),
    (Said::LocalChanges, "you have unstaged changes"),
    (Said::LocalChanges, "your index contains uncommitted changes"),
    (Said::NotFastForward, "not possible to fast-forward"),
    (Said::NotFastForward, "diverging branches can't be fast-forwarded"),
];

impl Said {
    /// Whether `said`, what git printed, names this failure.
    pub(crate) fn in_text(self, said: &str) -> bool {
        let lower = said.to_lowercase();
        WORDS.iter().any(|(kind, words)| *kind == self && lower.contains(words))
    }
}

/// The files git lists, indented by a tab, under a message about local changes.
pub(crate) fn overwritten_paths(said: &str) -> Vec<PathBuf> {
    said.lines()
        .filter_map(|line| line.strip_prefix('\t'))
        .map(|path| from_bytes(&unquoted(path.trim_end().as_bytes())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What git and its helpers print, one line for each of the words above.
    const PRINTED: &[(Said, &str)] = &[
        (
            Said::GitMissing,
            "xcrun: error: invalid active developer path (/Library/Developer/CommandLineTools), missing xcrun",
        ),
        (
            Said::GitMissing,
            "xcode-select: note: No developer tools were found, requesting install.",
        ),
        (
            Said::NotARepository,
            "fatal: not a git repository (or any of the parent directories): .git",
        ),
        (
            Said::DubiousOwnership,
            "fatal: detected dubious ownership in repository at '/Volumes/x/repo'",
        ),
        (
            Said::AuthFailed,
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://h/r.git/'",
        ),
        (
            Said::AuthFailed,
            "fatal: could not read Username for 'https://h': terminal prompts disabled",
        ),
        (
            Said::AuthFailed,
            "fatal: could not read Password for 'https://u@h': No such device",
        ),
        (Said::AuthFailed, "git@github.com: Permission denied (publickey)."),
        (Said::AuthFailed, "remote: HTTP Basic: Access denied"),
        (
            Said::AuthFailed,
            "fatal: unable to access 'https://h/r/': The requested URL returned error: 401",
        ),
        (
            Said::AuthFailed,
            "fatal: unable to access 'https://h/r/': The requested URL returned error: 403",
        ),
        (Said::AuthFailed, "Host key verification failed."),
        (
            Said::AuthFailed,
            "remote: Repository not found.\nfatal: repository 'https://h/r/' not found",
        ),
        (
            Said::Network,
            "fatal: unable to access 'https://h/': Could not resolve host: h",
        ),
        (
            Said::Network,
            "ssh: Could not resolve hostname h: nodename nor servname provided",
        ),
        (Said::Network, "ssh: connect to host h port 22: Connection refused"),
        (Said::Network, "ssh: connect to host h port 22: Connection timed out"),
        (Said::Network, "ssh: connect to host h port 22: Operation timed out"),
        (Said::Network, "ssh: connect to host h port 22: Network is unreachable"),
        (Said::Network, "ssh: connect to host h port 22: No route to host"),
        (Said::Network, "fatal: the remote end hung up unexpectedly"),
        (
            Said::Network,
            "fatal: Could not read from remote repository.\n\nPlease make sure you have the correct access rights",
        ),
        (
            Said::Network,
            "fatal: unable to access 'https://h/': Failed to connect to h port 443",
        ),
        (Said::Network, "fatal: early EOF"),
        (Said::EmptyMessage, "Aborting commit due to empty commit message."),
        (
            Said::LocalChanges,
            "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt",
        ),
        (
            Said::LocalChanges,
            "error: cannot cherry-pick: Please commit your changes or stash them.",
        ),
        (Said::LocalChanges, "new.txt already exists, no checkout"),
        (
            Said::LocalChanges,
            "error: cannot pull with rebase: You have unstaged changes.",
        ),
        (
            Said::LocalChanges,
            "error: cannot pull with rebase: Your index contains uncommitted changes.",
        ),
        (Said::NotFastForward, "fatal: Not possible to fast-forward, aborting."),
        (
            Said::NotFastForward,
            "hint: Diverging branches can't be fast-forwarded, you need to either merge or rebase.",
        ),
    ];

    #[test]
    fn every_line_git_prints_names_its_failure() {
        for (kind, printed) in PRINTED {
            assert!(kind.in_text(printed), "{kind:?}: {printed}");
        }
    }

    #[test]
    fn every_pattern_has_a_line_git_prints() {
        for (kind, words) in WORDS {
            assert!(
                PRINTED
                    .iter()
                    .any(|(k, printed)| k == kind && printed.to_lowercase().contains(words)),
                "{kind:?}: no printed line has {words:?}"
            );
        }
    }

    #[test]
    fn other_words_name_no_failure() {
        let kinds = [
            Said::GitMissing,
            Said::NotARepository,
            Said::DubiousOwnership,
            Said::AuthFailed,
            Said::Network,
            Said::EmptyMessage,
            Said::LocalChanges,
            Said::NotFastForward,
        ];
        assert!(kinds.iter().all(|k| !k.in_text("fatal: something else")));
    }

    #[test]
    fn files_git_would_overwrite_are_listed() {
        let said = "error: Your local changes to the following files would be overwritten by checkout:\n\
                    \ta.txt\n\t\"new\\nline.txt\"\nPlease commit your changes or stash them before you switch branches.\nAborting";
        assert_eq!(
            overwritten_paths(said),
            [PathBuf::from("a.txt"), PathBuf::from("new\nline.txt")]
        );
    }
}
