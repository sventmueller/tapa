//! git for the app, through the `git` program: repositories, status, diffs, staging down to single hunks, commits,
//! remotes, branches, the history as a graph with what each commit changed, the stash and worktrees.
//!
//! [`panel`] is what the app's Git panel decides without AppKit: its rows, the commit graph and its words for git's
//! errors.
//!
//! git runs code a repository controls: hooks, `core.fsmonitor`, filters, diff drivers, `core.sshCommand` and more
//! from its configuration. So every way into a repository, [`Git::discover`] and [`Git::init`], takes a [`Policy`]
//! the caller chooses per workspace (E21, E22): [`Policy::Untrusted`] runs every command neutralised, as [`neutral`]
//! describes; [`Policy::Trusted`] runs git as the user's own git would.
//!
//! Every call blocks until git ends and so belongs on a worker thread, never the main thread. Every process runs
//! through the supervisor as `Kind::Git` with the login shell's environment. Operations that throw work away for good
//! are reached through [`Repository::destructive`], so the app can ask for the user's approval before each.

#![forbid(unsafe_code)]

mod blame;
mod branch;
mod changed;
mod commit;
mod destructive;
mod diff;
mod error;
mod history;
mod hooks;
mod id;
pub mod neutral;
mod operation;
pub mod panel;
mod path;
mod remote;
mod repo;
mod run;
mod said;
mod staging;
mod stash;
mod status;
mod worktree;

pub use blame::{Blame, BlameCommit};
pub use branch::{Branch, Commit};
pub use changed::ChangedFile;
pub use commit::CommitOptions;
pub use destructive::Destructive;
pub use diff::{Change, DiffSource, FileDiff, Hunk, Line, LineKind};
pub use error::{Error, Result};
pub use history::CommitChanges;
pub use id::CommitId;
pub use neutral::Policy;
pub use operation::Operation;
pub use remote::{
    FetchOptions, Progress, PullMode, PullOptions, PushFlag, PushOptions, PushOutcome, PushedRef, Remote,
};
pub use repo::Repository;
pub use run::{Cancel, Git};
pub use stash::{Applied, Stash, StashApply, StashOptions, Unstash};
pub use status::{
    BranchStatus, Conflict, EntryKind, FileState, Status, StatusEntry, StatusOptions, Submodule, Untracked,
};
pub use worktree::Worktree;
