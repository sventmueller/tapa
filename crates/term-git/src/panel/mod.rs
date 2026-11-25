//! What the app's Git panel shows, without AppKit: its rows, the commit graph, and its words for git's errors
//! and for ages.

mod graph;
mod rows;
mod text;

pub use graph::{GraphRow, Segment};
pub use rows::{FileRow, Mark, Model, RowKey, Section};
pub use text::{Problem, ago, problem};
