//! The quick terminal (F45): one window that drops in from an edge of the active screen on a global hotkey and
//! slides away on the next, keeping its sessions in between. The hotkey is every chord the key bindings give
//! `quick_terminal`, registered with Carbon, which needs no accessibility permission.

mod hotkey;

use std::cell::Cell;

use term_config::{Action, QuickPosition};

use crate::state::app;
use crate::window::Window;

thread_local! {
    /// The quick terminal's window, once opened.
    static WINDOW: Cell<Option<u64>> = const { Cell::new(None) };
    /// Counts the slides, so a slide out that finishes after a slide in began does not hide the window.
    static SLIDE: Cell<u64> = const { Cell::new(0) };
}

/// Registers the hotkey; again after the settings changed.
pub(crate) fn install() {
    let (bindings, _) = app().config().bindings();
    let chords: Vec<term_config::Chord> = bindings
        .into_iter()
        .filter(|binding| binding.action == Action::QuickTerminal)
        .map(|binding| binding.chord)
        .collect();
    hotkey::register(&chords);
}

/// Shows the quick terminal, or hides it when it is in front with the keyboard.
pub(crate) fn toggle() {
    let app = app();
    match WINDOW.get().and_then(|id| app.window(id)) {
        Some(window) if window.quick_in_front() => window.slide_out(),
        Some(window) => window.slide_in(),
        None => {
            let window = Window::open_quick();
            WINDOW.set(Some(window.id()));
            window.slide_in();
        }
    }
}

/// Whether the quick terminal is in front, so that its toggle hides it.
pub(crate) fn in_front() -> bool {
    WINDOW
        .get()
        .and_then(|id| app().window(id))
        .is_some_and(|window| window.quick_in_front())
}

/// Starts another slide and returns its number.
pub(crate) fn next_slide() -> u64 {
    let next = SLIDE.get() + 1;
    SLIDE.set(next);
    next
}

/// The number of the latest slide.
pub(crate) fn slide() -> u64 {
    SLIDE.get()
}

/// The edge the quick terminal slides in from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

impl From<QuickPosition> for Edge {
    fn from(position: QuickPosition) -> Edge {
        match position {
            QuickPosition::Top => Edge::Top,
            QuickPosition::Bottom => Edge::Bottom,
            QuickPosition::Left => Edge::Left,
            QuickPosition::Right => Edge::Right,
        }
    }
}

/// The frames, as x, y, width and height from the bottom left, of a quick terminal at `edge` of a screen whose
/// usable part is `visible` and whole is `full`: where it shows, `size` of the screen deep and `span` of the edge
/// wide, and where it waits just beyond the screen's edge.
pub(crate) fn frames(visible: [f64; 4], full: [f64; 4], edge: Edge, size: f64, span: f64) -> ([f64; 4], [f64; 4]) {
    let [vx, vy, vw, vh] = visible;
    let [fx, fy, fw, fh] = full;
    match edge {
        Edge::Top | Edge::Bottom => {
            let (width, height) = ((vw * span).round(), (vh * size).round());
            let x = vx + ((vw - width) / 2.0).round();
            if edge == Edge::Top {
                ([x, vy + vh - height, width, height], [x, fy + fh, width, height])
            } else {
                ([x, vy, width, height], [x, fy - height, width, height])
            }
        }
        Edge::Left | Edge::Right => {
            let (width, height) = ((vw * size).round(), (vh * span).round());
            let y = vy + ((vh - height) / 2.0).round();
            if edge == Edge::Left {
                ([vx, y, width, height], [fx - width, y, width, height])
            } else {
                ([vx + vw - width, y, width, height], [fx + fw, y, width, height])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FULL: [f64; 4] = [0.0, 0.0, 1440.0, 900.0];
    /// Below a 25 point menu bar, beside a 60 point Dock at the bottom.
    const VISIBLE: [f64; 4] = [0.0, 60.0, 1440.0, 815.0];

    #[test]
    fn from_the_top_it_hangs_below_the_menu_bar_and_waits_above_the_screen() {
        let (shown, hidden) = frames(VISIBLE, FULL, Edge::Top, 0.4, 1.0);
        assert_eq!(shown, [0.0, 875.0 - 326.0, 1440.0, 326.0]);
        assert_eq!(hidden, [0.0, 900.0, 1440.0, 326.0]);
    }

    #[test]
    fn a_partial_span_is_centred_on_its_edge() {
        let (shown, _) = frames(VISIBLE, FULL, Edge::Bottom, 0.5, 0.5);
        assert_eq!(shown, [360.0, 60.0, 720.0, 408.0]);
        let (shown, hidden) = frames(VISIBLE, FULL, Edge::Right, 0.25, 1.0);
        assert_eq!(shown, [1080.0, 60.0, 360.0, 815.0]);
        assert_eq!(hidden[0], 1440.0, "waits right of the screen");
        let (_, hidden) = frames(VISIBLE, FULL, Edge::Left, 0.25, 1.0);
        assert_eq!(hidden[0], -360.0);
    }

    #[test]
    fn a_second_screen_keeps_its_origin() {
        let full = [1440.0, -200.0, 1920.0, 1080.0];
        let (shown, hidden) = frames(full, full, Edge::Top, 0.5, 1.0);
        assert_eq!(shown, [1440.0, 340.0, 1920.0, 540.0]);
        assert_eq!(hidden[1], 880.0);
    }
}
