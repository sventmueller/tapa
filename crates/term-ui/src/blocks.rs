//! Command blocks in a terminal view (F78, F79, F81 to F84, F87): the overlay that follows the screen, and what the
//! block actions, the bookmarks and a click into the line typed at a prompt do. They need the own core's shell marks;
//! without them the view shows no blocks and the actions find none.

pub(crate) mod model;
mod overlay;

use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};

use block2::RcBlock;
use objc2::rc::Weak;
use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::{NSBeep, NSColor, NSFont, NSModalResponseOK, NSSavePanel};
use objc2_foundation::{NSDate, NSDateFormatter, NSDateFormatterStyle};
use splat_i18n::tf;
use term_session::session::BlockPart;
use term_session::{Point, Rgb};

use self::overlay::BlockOverlay;
use super::view::TerminalView;
use crate::kit::{main_after, ns};

/// The overlay follows the screen at most this often while output streams, as each look takes the session's lock.
const GAP: Duration = Duration::from_millis(40);

/// The block overlay of a view and when it last followed the screen.
#[derive(Default)]
struct Follow {
    overlay: Option<Weak<BlockOverlay>>,
    last: Option<Instant>,
    pending: bool,
}

thread_local! {
    static FOLLOW: RefCell<HashMap<u64, Follow>> = RefCell::new(HashMap::new());
}

fn colour(c: Rgb) -> objc2::rc::Retained<NSColor> {
    NSColor::colorWithSRGBRed_green_blue_alpha(
        f64::from(c.r) / 255.0,
        f64::from(c.g) / 255.0,
        f64::from(c.b) / 255.0,
        1.0,
    )
}

/// The time of day of `at`, as the user's locale writes it.
pub(crate) fn clock(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let date = NSDate::dateWithTimeIntervalSince1970(seconds);
    NSDateFormatter::localizedStringFromDate_dateStyle_timeStyle(
        &date,
        NSDateFormatterStyle::NoStyle,
        NSDateFormatterStyle::MediumStyle,
    )
    .to_string()
}

impl TerminalView {
    /// Has the block overlay follow the screen after a frame, at once or, while frames come quickly, a little later.
    pub(crate) fn blocks_follow(&self) {
        let id = self.id();
        let wait = FOLLOW.with(|follow| {
            let mut follow = follow.borrow_mut();
            follow.retain(|known, _| *known == id || crate::find(*known).is_some());
            let entry = follow.entry(id).or_default();
            if entry.pending {
                return None;
            }
            let since = entry.last.map(|last| last.elapsed());
            match since {
                Some(since) if since < GAP => {
                    entry.pending = true;
                    Some(GAP - since)
                }
                _ => Some(Duration::ZERO),
            }
        });
        match wait {
            None => {}
            Some(Duration::ZERO) => self.blocks_update(),
            Some(wait) => main_after(wait, move || {
                FOLLOW.with(|follow| {
                    if let Some(entry) = follow.borrow_mut().get_mut(&id) {
                        entry.pending = false;
                    }
                });
                if let Some(view) = crate::find(id) {
                    view.blocks_update();
                }
            }),
        }
    }

    fn overlay(&self) -> Option<objc2::rc::Retained<BlockOverlay>> {
        FOLLOW.with(|follow| {
            follow
                .borrow()
                .get(&self.id())
                .and_then(|entry| entry.overlay.as_ref())
                .and_then(Weak::load)
        })
    }

    /// Shows the blocks of the screen as it is now.
    fn blocks_update(&self) {
        let id = self.id();
        if !crate::host().config().terminal.blocks {
            if let Some(overlay) = self.overlay() {
                overlay.removeFromSuperview();
                FOLLOW.with(|follow| follow.borrow_mut().entry(id).or_default().overlay = None);
            }
            return;
        }
        FOLLOW.with(|follow| follow.borrow_mut().entry(id).or_default().last = Some(Instant::now()));
        let Some(view) = self.with_session(term_session::Session::blocks_view) else {
            return;
        };
        let overlay = match self.overlay() {
            Some(overlay) => overlay,
            None if view.blocks.is_empty() && view.bookmarks.is_empty() => return,
            None => {
                let overlay = BlockOverlay::new(self.mtm(), id, self.bounds());
                self.addSubview(&overlay);
                FOLLOW.with(|follow| {
                    follow.borrow_mut().entry(id).or_default().overlay = Some(Weak::from(&*overlay));
                });
                overlay
            }
        };
        let metrics = self.metrics();
        let palette = self.ivars().theme.borrow().palette.clone();
        let face = crate::host().config().face();
        let size = self.font_size();
        let font = NSFont::fontWithName_size(&ns(&face.family), size)
            .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(size, 0.0));
        overlay.show(
            view,
            metrics.cell_height,
            &font,
            (&colour(palette.foreground), &colour(palette.background)),
        );
    }

    /// Block `id`, else the one the block actions act on.
    fn block_or_current(&self, id: Option<u64>) -> Option<u64> {
        id.or_else(|| self.with_session(term_session::Session::current_block).flatten())
    }

    /// Selects block `id`, else the current one, from its prompt to the end of its output (F78); whether there was one.
    pub fn select_block(&self, id: Option<u64>) -> bool {
        let selected = self
            .block_or_current(id)
            .and_then(|id| self.with_session(|session| session.select_block(id)))
            .unwrap_or(false);
        if selected {
            self.refresh(true);
            self.blocks_update();
        } else {
            NSBeep();
        }
        selected
    }

    fn output_of(&self, id: Option<u64>) -> Option<(String, Option<String>)> {
        let id = self.block_or_current(id)?;
        self.with_session(|session| {
            let output = session.block_text(id, BlockPart::Output)?;
            Some((output, session.block_text(id, BlockPart::Command)))
        })
        .flatten()
        .filter(|(output, _)| !output.is_empty())
    }

    /// Copies the output of block `id`, else of the current one (F81); whether there was any.
    pub fn copy_block_output(&self, id: Option<u64>) -> bool {
        match self.output_of(id) {
            Some((output, _)) => {
                super::clip::store(&output);
                true
            }
            None => {
                NSBeep();
                false
            }
        }
    }

    /// Asks where to save the output of block `id`, else of the current one, and writes it there (F81); whether
    /// there was any.
    pub fn save_block_output(&self, id: Option<u64>) -> bool {
        let (Some((output, command)), Some(window)) = (self.output_of(id), self.window()) else {
            NSBeep();
            return false;
        };
        let panel = NSSavePanel::savePanel(self.mtm());
        panel.setNameFieldStringValue(&ns(&model::file_name(command.as_deref())));
        let chosen = panel.clone();
        let view = self.id();
        let handler = RcBlock::new(move |response| {
            crate::kit::guarded((), || {
                if response != NSModalResponseOK {
                    return;
                }
                let Some(path) = chosen.URL().and_then(|url| url.path()) else {
                    return;
                };
                if let Err(error) = std::fs::write(path.to_string(), output.as_bytes())
                    && let Some(view) = crate::find(view)
                {
                    view.notify(&tf(
                        "Die Ausgabe ließ sich nicht sichern: {}",
                        &[&term_base::explain::Explain::explain(&error)],
                    ));
                }
            });
        });
        panel.beginSheetModalForWindow_completionHandler(&window, &handler);
        true
    }

    /// Collapses the output of block `id`, else of the current one, or shows it again (F78); whether it is collapsed.
    pub fn toggle_block(&self, id: Option<u64>) -> bool {
        let shown = crate::host().config().terminal.blocks;
        let Some(id) = self.block_or_current(id).filter(|_| shown) else {
            NSBeep();
            return false;
        };
        let folded = self.with_session(|session| session.toggle_block(id)).unwrap_or(false);
        self.refresh(true);
        self.blocks_update();
        folded
    }

    /// Sets a bookmark on the selection's first line, the top line of a view scrolled back or the cursor's line, or
    /// removes the one there (F87); whether there is one now.
    pub fn toggle_bookmark(&self) -> bool {
        let set = self.with_session(term_session::Session::toggle_bookmark);
        match set {
            Some(set) => {
                self.refresh(true);
                self.blocks_update();
                set
            }
            None => {
                NSBeep();
                false
            }
        }
    }

    /// Scrolls to the bookmark `step` bookmarks away (F87); whether the view moved.
    pub fn jump_to_bookmark(&self, step: isize) -> bool {
        let moved = self
            .with_session(|session| session.jump_to_bookmark(step))
            .unwrap_or(false);
        if moved {
            self.refresh(true);
        }
        moved
    }

    /// A plain click at `point`: moves the shell's cursor there while it waits at a prompt for input (F83); whether
    /// it did.
    pub(crate) fn click_in_prompt(&self, point: Point) -> bool {
        self.with_session(|session| {
            let modes = session.input_modes();
            modes.mouse == term_session::input::MouseTracking::Off
                && !modes.alternate_screen
                && session.click_in_prompt(point)
        })
        .unwrap_or(false)
    }

    /// Clicks the cell at `point` as a plain click does, for the self-test; whether the shell's cursor moved.
    #[cfg(feature = "selftest")]
    pub fn click_at(&self, point: Point) -> bool {
        self.click_in_prompt(point)
    }

    /// The blocks the view shows, for the self-test: each with its command, state, exit code and lines, then the
    /// sticky header, the bookmarks and the current block.
    #[cfg(feature = "selftest")]
    pub fn blocks_report(&self) -> String {
        let Some(view) = self.with_session(term_session::Session::blocks_view) else {
            return "no session".into();
        };
        let blocks: Vec<String> = view
            .blocks
            .iter()
            .map(|shown| {
                let block = &shown.block;
                format!(
                    "{:?} {:?} exit {:?} lines {:?}{}",
                    block.command.as_deref().unwrap_or(""),
                    block.state,
                    block.exit_code,
                    shown.lines,
                    if shown.folded { " folded" } else { "" }
                )
            })
            .collect();
        format!(
            "blocks {blocks:?} sticky {:?} bookmarks {:?} current {:?}",
            view.sticky.as_ref().map(|sticky| sticky.heading.clone()),
            view.bookmarks,
            view.current,
        )
    }
}
