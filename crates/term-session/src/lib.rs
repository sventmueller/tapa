//! Terminal sessions without a window: the user's shell in a PTY with its screen kept by `term-vt` (E4), keys,
//! mouse and paste turned into bytes, shell integration for prompts and the working directory, the program in front
//! with coding agents recognised, and what pasting an image does.
//!
//! The view draws `Session::snapshot` and sends input through the session; events arrive on the session's reader
//! thread.

#![deny(unsafe_code)]

pub mod event;
mod fair;
pub mod foreground;
pub mod frames;
pub use term_vt::search::Options as FindOptions;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
mod gather;
pub mod input;
pub mod integration;
mod io;
mod own;
pub mod paste;
#[allow(unsafe_code, reason = "opening the PTY and starting the program in it")]
mod pty;
pub mod session;
pub mod snapshot;
#[allow(unsafe_code, reason = "libc calls and the poller registration on the PTY")]
mod sys;

pub use event::{Clipboard, Event, ShellReport, WorkingDirectory};
pub use foreground::{Agent, Foreground};
pub use frames::{FrameSink, FrameSource};
pub use input::{InputModes, Key, KeyAction, KeyCode, Modifiers, MouseAction, MouseButton, MouseEvent, NamedKey};
pub use paste::{PasteAction, Refusal};
pub use session::{CellSide, Found, LockTimes, Options, Program, Scroll, SelectionKind, Session, Size, TypedLine};
pub use snapshot::{Cell, CellFlags, Cursor, CursorShape, Damage, Highlight, Palette, Point, Rgb, Snapshot};
