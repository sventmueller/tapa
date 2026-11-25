//! The frame and sidebar width of the window the user last resized or arranged, which a new window opens with while
//! the settings' columns and rows are those it was taken under. Kept beside the restored state but apart from it and
//! also with restoring off, as macOS apps keep their window size when they reopen no windows; sizes are no secret
//! worth sealing.

use std::cell::RefCell;
use std::path::Path;

use objc2_foundation::NSRect;
use serde::{Deserialize, Serialize};
use term_config::Config;

use super::frame::{Look, TabWindow};
use super::geometry;
use crate::state::app;

const FILE: &str = "window.json";

/// The window last resized or arranged.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Last {
    /// Origin and size in screen points.
    pub(crate) frame: [f64; 4],
    #[serde(default = "default_width")]
    pub(crate) sidebar_width: f64,
    /// The settings' columns and rows of a new window when the frame was taken.
    #[serde(default)]
    pub(crate) columns: u16,
    #[serde(default)]
    pub(crate) rows: u16,
}

fn default_width() -> f64 {
    geometry::SIDEBAR_WIDTH
}

impl Last {
    /// The frame for a new window under `config`: none once the settings' columns or rows changed, as the user then
    /// asked for that size.
    fn frame_for(&self, config: &Config) -> Option<[f64; 4]> {
        let [_, _, width, height] = self.frame;
        let same = self.columns == config.window.columns && self.rows == config.window.rows;
        (same && width > 0.0 && height > 0.0).then_some(self.frame)
    }

    /// The sidebar for a new window under `config`: shown as the settings say, as wide as it was last.
    fn look_for(this: Option<&Last>, config: &Config) -> Look {
        let width = this.map_or(geometry::SIDEBAR_WIDTH, |last| last.sidebar_width);
        Look {
            collapsed: !config.window.sidebar,
            width: width.clamp(geometry::SIDEBAR_MIN, geometry::SIDEBAR_MAX),
        }
    }
}

/// What is known, read from the file the first time, and whether it changed since it was written.
#[derive(Default)]
struct Kept {
    read: bool,
    last: Option<Last>,
    changed: bool,
}

thread_local! {
    static KEPT: RefCell<Kept> = RefCell::new(Kept::default());
}

fn with<R>(f: impl FnOnce(&mut Kept) -> R) -> R {
    KEPT.with(|kept| {
        let mut kept = kept.borrow_mut();
        if !kept.read {
            kept.read = true;
            kept.last = read(&crate::restore::folder().join(FILE));
        }
        f(&mut kept)
    })
}

fn read(path: &Path) -> Option<Last> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Takes `frame`'s window as the last one, unless it fills the screen in full-screen mode.
pub(super) fn remember(frame: &TabWindow, look: Look) {
    let full_screen = frame
        .ns
        .styleMask()
        .contains(objc2_app_kit::NSWindowStyleMask::FullScreen);
    if full_screen {
        return;
    }
    let config = app().config();
    let rect = frame.ns.frame();
    let last = Last {
        frame: [rect.origin.x, rect.origin.y, rect.size.width, rect.size.height],
        sidebar_width: look.width,
        columns: config.window.columns,
        rows: config.window.rows,
    };
    with(|kept| {
        if kept.last != Some(last) {
            kept.last = Some(last);
            kept.changed = true;
        }
    });
}

/// Takes the window in front as the last one and writes what changed since it was last written.
pub(crate) fn store() {
    if let Some(window) = crate::state::try_app()
        .and_then(|app| app.front())
        .filter(|window| !window.is_quick())
        && let Some(front) = window.front_frame().filter(|front| front.ns.isVisible())
    {
        remember(&front, front.look());
    }
    let Some(last) = with(|kept| kept.last.filter(|_| std::mem::take(&mut kept.changed))) else {
        return;
    };
    let dir = crate::restore::folder();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(json) = serde_json::to_vec(&last)
        && let Err(error) = term_base::atomic_write::write(&dir.join(FILE), &json)
    {
        term_base::diag::report("storing the window size", &error);
    }
}

/// The frame a new window takes, if the last one's still fits the settings.
pub(super) fn frame(config: &Config) -> Option<NSRect> {
    let [x, y, width, height] = with(|kept| kept.last)?.frame_for(config)?;
    Some(NSRect::new(
        objc2_foundation::NSPoint::new(x, y),
        objc2_foundation::NSSize::new(width, height),
    ))
}

/// The sidebar a new window shows.
pub(super) fn look(config: &Config) -> Look {
    Last::look_for(with(|kept| kept.last).as_ref(), config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn last() -> Last {
        Last {
            frame: [100.0, 200.0, 1100.0, 700.0],
            sidebar_width: 280.0,
            columns: Config::default().window.columns,
            rows: Config::default().window.rows,
        }
    }

    #[test]
    fn a_new_window_takes_the_last_frame_until_the_settings_size_changes() {
        let mut config = Config::default();
        assert_eq!(last().frame_for(&config), Some([100.0, 200.0, 1100.0, 700.0]));
        config.window.columns += 10;
        assert_eq!(last().frame_for(&config), None, "the new columns win");
        config.window.columns -= 10;
        config.window.rows += 1;
        assert_eq!(last().frame_for(&config), None, "the new rows win");
    }

    #[test]
    fn the_sidebar_follows_the_setting_and_keeps_its_width() {
        let mut config = Config::default();
        config.window.sidebar = false;
        let look = Last::look_for(Some(&last()), &config);
        assert!(look.collapsed);
        assert!((look.width - 280.0).abs() < f64::EPSILON);
        config.window.sidebar = true;
        let look = Last::look_for(None, &config);
        assert!(!look.collapsed);
        assert!((look.width - geometry::SIDEBAR_WIDTH).abs() < f64::EPSILON);
        let wide = Last {
            sidebar_width: 9000.0,
            ..last()
        };
        assert!((Last::look_for(Some(&wide), &config).width - geometry::SIDEBAR_MAX).abs() < f64::EPSILON);
    }

    #[test]
    fn a_file_with_only_the_frame_still_reads() {
        let back: Last = serde_json::from_str(r#"{"frame":[1.0,2.0,800.0,600.0]}"#).unwrap();
        assert!((back.sidebar_width - geometry::SIDEBAR_WIDTH).abs() < f64::EPSILON);
        assert_eq!(back.frame_for(&Config::default()), None, "taken under other settings");
        let json = serde_json::to_string(&last()).unwrap();
        assert_eq!(serde_json::from_str::<Last>(&json).unwrap(), last());
    }
}
