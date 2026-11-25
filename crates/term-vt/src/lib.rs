//! The terminal's own VT core (E4), which `term-session` keeps each session's screen with: the parser, the screens, and
//! the second look at the output for shell integration marks (OSC 133), the working directory (OSC 7) and local web
//! servers.

pub mod cell;
pub mod color;
mod devserver;
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
pub mod grid;
pub mod history;
mod host;
pub mod parser;
pub mod reference;
pub mod resize;
pub mod rows;
pub mod scan;
pub mod search;
pub mod selection;
pub mod style;
pub mod term;

pub use scan::{Mark, Scanner, ShellReport, WorkingDirectory};
pub use term::{Event, Size, Terminal};
