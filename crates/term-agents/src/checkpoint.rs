//! Checkpoints of the work folder before each prompt, which this app does not take (E21): the types keep the
//! signals' shape and have no value.

/// A checkpoint taken before a prompt; there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checkpoint {}

/// The commit of a checkpoint; there is none.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitId {}
