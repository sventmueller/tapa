//! One driver over every agent session: it starts a session of any of the four paths, sends prompts, answers
//! requests by a rule it is handed, fans the session's events out to any number of views and ends every turn with a
//! typed result. It knows no AppKit; the app hands it the hop to the main thread.
//!
//! # Starting
//!
//! A [`Launch`] names the [`AgentPath`] (Claude with its Remote Control and a rewind of the session it resumes,
//! Codex, OpenCode, an ACP preset found on the PATH, or, for the tests, the scripted session and a played recording
//! of Claude), the folder, the login shell's environment, model, effort, the session to resume, the user's
//! [`UserChoices`] and whether the workspace is trusted. [`Registry::start`] starts it under a [`SessionId`] the app
//! keeps in its context, with a [`RequestRule`], normally [`UserRule`].
//!
//! A workspace that is not trusted (E16) starts each path without the project's configuration, hooks and MCP
//! servers:
//!
//! | Path | Untrusted |
//! |---|---|
//! | Claude | `--setting-sources=user --strict-mcp-config` |
//! | Codex | the folder's `projects.<folder>.trust_level` set to `untrusted`, so no `.codex/` configuration loads |
//! | OpenCode | `OPENCODE_DISABLE_PROJECT_CONFIG` and `OPENCODE_DISABLE_CLAUDE_CODE` |
//! | ACP | no MCP servers handed over; OpenCode as above; every other agent is refused with [`LaunchError::Untrusted`] |
//!
//! # Driving
//!
//! [`Driver::send`] queues a [`Prompt`]; it goes out when no turn runs. [`Driver::stop_turn`],
//! [`Driver::stop_background`], [`Driver::clear_queue`] and [`Driver::stop`] stop what they say;
//! [`Driver::stop_task`] stops one subagent or background item and [`Driver::set_model`] switches the model where
//! the path can while it runs. Requests go to the rule once: [`Verdict::Allow`], [`Verdict::Deny`] with a reason the
//! agent reads where the path carries one, or [`Verdict::User`], after which the user answers through
//! [`Driver::answer_permission`], [`Driver::answer_questions`] and [`Driver::answer_plan`]. [`Driver::steer`] hands
//! a message to the turn that runs at once, [`Driver::answer_permission_as`] takes Claude's answer with an edited
//! input and suggestions to keep as rules, never one for destructive git, [`Driver::stop_turn`] also stops a turn
//! the agent began by itself, and [`Driver::claude`] reaches what only Claude Code offers, such as its overlays,
//! Remote Control and its usage. [`Driver::codex`], [`Driver::opencode`] and [`Driver::acp`] do the same for the
//! other paths; [`Driver::answer_option`] answers with any option a request offers, such as allowing for the whole
//! session. A gap in OpenCode's stream holds neither prompts nor the end of a turn: the session goes on and is
//! reconciled once the stream is back.
//!
//! # Events
//!
//! Sessions report on their own threads into the driver's inbox, which asks [`Wake`] once for a hop to the main
//! thread; there the app calls [`Registry::flush`]. Each subscriber ([`Driver::subscribe`]) has a feed: control
//! events ([`Signal`]) complete and in order, invalidations coalesced, a redraw from the snapshot after a backlog of
//! more than 4096; a new subscriber's first [`Batch`] is the snapshot. `flush` returns the subscribers whose feed
//! just filled, which [`Driver::take`] empties; [`Driver::unsubscribe`] ends a feed. Once nobody follows a session,
//! the next flush stops it and the registry lets it go.
//!
//! [`Signal::Delivered`] says that a prompt went out, with the agent's id of the message, which rewinding names.
//! No checkpoints are taken: [`Checkpoint`] has no value. [`Signal::ClaudeFailed`], [`Signal::CodexFailed`],
//! [`Signal::OpenCodeFailed`] and [`Signal::AcpFailed`] follow each `Failed` of their path with the typed error;
//! [`Signal::RemoteControl`] tells that Claude's Remote Control changed.
//!
//! Every turn ends with [`Signal::TurnEnded`]: its [`TurnId`], an [`Outcome`] (succeeded, failed with the path's
//! words, cancelled, connection lost) and how much work still runs. Every path ends a turn in `Idle`, failed ones
//! too; the outcome comes from the last interruption or error in the turn that no output of the agent follows, and
//! from the process's end.
//!
//! When OpenCode's event stream breaks off, [`Signal::StreamGap`] follows; once it is back, the driver reads
//! messages, waiting requests and usage from the server again ([`Signal::Resynced`]), or, when that fails, reports
//! [`Signal::NeedsPause`], after which [`Driver::resync`] tries again.
//!
//! The scripted session (feature `selftest`) plays a [`Script`] per prompt, writes files in the folder and asks for
//! permissions, so self-tests can run without an agent's CLI. A played recording of Claude (`AgentPath::Replay`)
//! takes no prompts; `Driver::feed` plays lines into it and hands what changed to the subscribers at once, and
//! `Driver::replay_plan` answers its plans.

#![forbid(unsafe_code)]

mod backend;
mod checkpoint;
mod driver;
mod feed;
mod id;
mod inbox;
pub mod launch;
mod registry;
pub mod rule;
#[cfg(any(test, feature = "selftest"))]
pub mod script;
mod start;
pub mod turn;

pub use backend::PlanReply;
pub use checkpoint::{Checkpoint, CommitId};
pub use driver::{AnswerError, Driver, Prompt, SendError, StreamState};
pub use feed::{Batch, Signal};
pub use id::{SessionId, SubscriberId, TurnId};
pub use launch::{AgentPath, Launch, LaunchError, PathKind, UserChoices};
pub use registry::{Registry, Wake};
pub use rule::{Request, RequestKind, RequestRule, UserRule, Verdict};
#[cfg(any(test, feature = "selftest"))]
pub use script::{End, Script, ScriptTurn, Step};
pub use turn::{Outcome, TurnEnd};
