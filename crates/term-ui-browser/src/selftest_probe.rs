//! What the self-test reads from and does with the parts of `lib.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

pub use super::dialogs::answer_asking;

pub use super::tabs::{Seen, address_field, fill_login, front_view, inspect, proceed, read_front_page};

pub use super::web::open_popups_unasked;
