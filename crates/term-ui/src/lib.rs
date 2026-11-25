//! A terminal session's view (E5, E12): the layer-backed view that hands a session keys, mouse, pastes and drops and
//! shows notices above it, draws its screen through `term-render`, finds what a ⌘-click points at, turns Mac keys
//! into terminal keys and decides what it can without AppKit. Tabs, panes and the sidebar stay in the app, which a
//! view reaches through `Host`, naming its window by id; the AppKit shorthands of all UI crates are in `kit`.

mod access;
mod banner;
mod blocks;
mod clip;
mod copy;
mod field;
mod find;
mod frame;
pub mod git;
mod host;
mod input;
#[cfg(feature = "selftest")]
pub mod inspect;
pub mod keys;
pub mod kit;
mod link_policy;
pub mod links;
pub mod model;
mod mouse;
pub mod palette;
mod paste_guard;
mod scrollbar;
pub mod secure;
mod session;
mod size_hint;
mod smart;
mod view;
pub mod workspace_host;

pub use field::toggle::Look as FieldLook;
#[cfg(feature = "selftest")]
use host::latest_id;
pub use host::{Change, Host, Opened, State, all_views, find, install, set_option_as_meta};
use host::{forget, host, listener, next_id, option_as_meta, register};
pub use input::chord_of;
pub use view::TerminalView;
