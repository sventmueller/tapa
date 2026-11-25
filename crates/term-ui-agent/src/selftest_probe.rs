//! What the self-test reads from and does with the parts of `lib.rs`, kept out of its production code.

#[allow(
    unused_imports,
    reason = "the parts moved here use only some of their module's names"
)]
use super::*;

pub use super::activity::{activity_lines, activity_updated_in_place, activity_view, press_activity};

pub use super::acp::{set_test_installs, set_test_launch, set_test_register};
pub use super::codex::set_test_codex;
pub use super::opencode::set_test_opencode;

pub use super::cards::window_name;

pub use super::context_view::{
    click as click_context, close as close_context, cost_text, seen as context_seen, token_text,
};

pub use super::figures::{asked_for as asked_for_figure, is_dark};

pub use super::format::percent;

pub use super::source::set_test_program as set_test_claude;

pub use super::tab::{INPUT_INSET, INPUT_WIDTH, SIDE_MARGIN};

pub use super::tab_probe::TabMenu;

pub use super::usage::inject as inject_usage;

/// An answer's markdown as its card sets it, for the self-test.
pub fn answer_text(
    mtm: objc2::MainThreadMarker,
    markdown: &str,
) -> objc2::rc::Retained<objc2_foundation::NSAttributedString> {
    markdown::render_with_figures(mtm, markdown, style::BODY, markdown::Tone::Primary, 500.0)
}

/// The height an answer's markdown takes in its card at `width`.
pub fn answer_height(mtm: objc2::MainThreadMarker, markdown: &str, width: f64) -> f64 {
    use objc2_app_kit::NSView;
    let rendered = markdown::render_with_figures(mtm, markdown, style::BODY, markdown::Tone::Primary, width);
    let text = text::CardText::new(mtm, &rendered, width, true);
    let view: &NSView = &text;
    view.intrinsicContentSize().height
}

pub use super::cards::Part;

pub use super::cards::notice_text;

pub use super::cards::request::{ALLOW_ALWAYS, ALLOW_ONCE, DENY};

/// Whether `@terminal` finds a terminal's output in `controller`'s window.
pub fn terminal_context(controller: &(impl AsWorkspace + ?Sized)) -> bool {
    let controller = &controller.as_workspace();
    context::terminal(controller)
        .is_some_and(|a| matches!(a, term_thread::Attachment::External { text, .. } if !text.is_empty()))
}

/// How many notifications sessions posted, or would have where posting is not allowed.
pub fn notifications_posted() -> usize {
    notify::posted()
}

/// Opens an agent tab in `cwd` titled `title` that plays recordings instead of running `claude`, for the self-test.
pub fn open_replay_in(controller: &(impl AsWorkspace + ?Sized), cwd: std::path::PathBuf, title: &str) -> Rc<AgentTab> {
    let controller = &controller.as_workspace();
    let state = crate::host::state();
    let tab = AgentTab::open_replay_in(controller, &state, cwd);
    tab.set_custom_title(title);
    state.agent_tabs.tabs.borrow_mut().push(tab.clone());
    changed(controller);
    tab
}

/// Opens an agent tab that plays recordings instead of running `claude`, for the self-test.
pub fn open_replay(controller: &(impl AsWorkspace + ?Sized)) -> Option<Rc<AgentTab>> {
    let controller = &controller.as_workspace();
    let state = crate::host::state();
    let tab = AgentTab::open_replay(controller, &state)?;
    state.agent_tabs.tabs.borrow_mut().push(tab.clone());
    changed(controller);
    Some(tab)
}
