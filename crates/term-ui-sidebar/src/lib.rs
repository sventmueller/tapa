//! The sidebar of workspaces (E9, E20, F40, F211, F212); a workspace's tabs are native window tabs, which the app
//! keeps. Each row is a workspace: its name, base folder and branch, a dot while its agents work or wait for the user as
//! in Dango, and the switches of its panels, which take the list's place. Display only; no AI integration.

mod cell;
pub use cell::mark_color;
mod footer;
pub use term_ui::git;
pub mod model;
mod panel_view;
mod rename;
mod reorder;
mod table;
mod view;

pub use model::{Connection, Entry, Kind, Mark, Panel, Remote, Status, Workspace};
pub use view::{PanelView, RowAction, Sidebar};
