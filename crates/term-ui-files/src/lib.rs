//! The file list of a workspace's sidebar entry (E22): the folder tree of the workspace's base path, read one folder
//! at a time as the user opens it, with new files and folders, renaming, the Trash and the Finder in its context
//! menu. A click opens a file in the workspace's preview tab, a double click in a tab of its own; the model in
//! `term_project::files_panel` decides, the view draws, the controller carries out.
//!
//! The view is laid out at the sidebar's width, and the app shows it in the sidebar in place of the workspaces
//! ([`panel_view`]) with the buttons of its title row ([`panel_accessory`]). The
//! FSEvents watcher runs only while the list exists, from the first [`panel_view`] until [`detach`]; it never watches
//! `/` or the home folder itself.

mod controller;
mod host;
mod hover;
mod panel;
mod prewarm;
mod project;
mod view;

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use term_project::TreeOptions;
use term_ui::workspace_host::AsWorkspace;

pub use host::{Host, install};

/// The view of the workspace's file list, built for its base path the first time, which starts its watcher, and the
/// same view after; `None` once the workspace closed or its folder cannot be read.
pub fn panel_view(workspace: &(impl AsWorkspace + ?Sized)) -> Option<Retained<NSView>> {
    panel::attach(workspace.as_workspace())?.page()
}

/// The buttons of the title row of the workspace's file list, once [`panel_view`] built it.
pub fn panel_accessory(workspace: &(impl AsWorkspace + ?Sized)) -> Option<Retained<NSView>> {
    panel::of(workspace.as_workspace())?.buttons()
}

/// Lets go of the workspace's file list and stops its watcher, as when the list closes or the workspace does; the
/// next [`panel_view`] builds it anew.
pub fn detach(workspace: &(impl AsWorkspace + ?Sized)) {
    panel::detach(workspace.as_workspace());
}

/// Whether the workspace's file list is built, which is while its watcher runs.
pub fn is_open(workspace: &(impl AsWorkspace + ?Sized)) -> bool {
    panel::of(workspace.as_workspace()).is_some()
}

/// The user changed whether ignored and hidden files show: every list reads its open folders again.
pub fn show_ignored_changed() {
    let show_ignored = host::installed().is_some_and(|host| host.show_ignored());
    for panel in panel::all() {
        panel.ivars().project.set_options(TreeOptions { show_ignored });
    }
}

/// What the self-test reads from and does with a workspace's file list.
#[cfg(feature = "selftest")]
pub mod selftest;
