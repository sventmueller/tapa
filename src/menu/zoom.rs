//! ⌘+, ⌘− and ⌘0 on what has the keyboard, each from its own size under Settings ▸ General: the terminal pane in
//! front (F189), the editor, the agent tabs or the sidebar with its panels. The zoom lasts until the settings change and
//! never changes them; ⌘0 goes back to the size in the settings.

use objc2::rc::Retained;
use objc2_app_kit::NSView;
use term_config::Action;
use term_ui::kit::ui_size;

use crate::state::App;
use crate::window::Window;

/// What a change of the text size acts on.
enum Target {
    Terminal(Retained<term_ui::TerminalView>),
    Editor(Retained<NSView>),
    Agent,
    Interface,
}

/// What has the keyboard in `window`, the session in front where nothing else claims it.
fn target(window: &Window) -> Option<Target> {
    let responder = window
        .front_ns_window()
        .and_then(|ns| ns.firstResponder())
        .and_then(|responder| responder.downcast::<NSView>().ok());
    if let Some(view) = responder {
        if term_ui_editor::holds_keyboard(&view) {
            return Some(Target::Editor(view));
        }
        if term_ui_agent::all().iter().any(|tab| view.isDescendantOf(&tab.root())) {
            return Some(Target::Agent);
        }
        if window.sidebar_holds(&view) {
            return Some(Target::Interface);
        }
    }
    window.focused_view().map(Target::Terminal)
}

/// Whether one of the three actions has something to act on in `window`.
pub(super) fn can_run(window: &Window) -> bool {
    target(window).is_some()
}

/// Runs `action`, one of ⌘+, ⌘− and ⌘0, on what has the keyboard in `window`.
pub(super) fn run(app: &App, window: &Window, action: Action) {
    let change = match action {
        Action::FontBigger => 1.0,
        Action::FontSmaller => -1.0,
        _ => 0.0,
    };
    let config = app.config();
    match target(window) {
        Some(Target::Terminal(view)) => {
            view.set_font_size(zoomed(view.font_size(), change, config.face().size, (6.0, 72.0)));
        }
        Some(Target::Editor(view)) => term_ui_editor::zoom(&view, change),
        Some(Target::Agent) => term_ui_agent::set_text_size(zoomed(
            term_ui_agent::text_size(),
            change,
            config.agent.font_size,
            (term_ui_agent::CHAT_FONT_MIN, term_ui_agent::CHAT_FONT_MAX),
        )),
        Some(Target::Interface) => ui_size::set(zoomed(
            ui_size::current(),
            change,
            config.window.interface_font_size,
            (ui_size::MIN, ui_size::MAX),
        )),
        None => {}
    }
}

/// The size after `change` points from `shown`, within `range`; `setting` for a change of 0.
fn zoomed(shown: f64, change: f64, setting: f64, (min, max): (f64, f64)) -> f64 {
    if change == 0.0 {
        setting
    } else {
        (shown + change).clamp(min, max)
    }
}

#[cfg(test)]
mod tests {
    use super::zoomed;

    #[test]
    fn a_step_stays_in_the_range_and_zero_goes_back_to_the_setting() {
        assert_eq!(zoomed(13.0, 1.0, 12.0, (6.0, 72.0)), 14.0);
        assert_eq!(zoomed(10.0, -1.0, 13.0, (10.0, 24.0)), 10.0);
        assert_eq!(zoomed(18.0, 1.0, 13.0, (10.0, 18.0)), 18.0);
        assert_eq!(zoomed(20.0, 0.0, 13.0, (10.0, 18.0)), 13.0);
    }
}
