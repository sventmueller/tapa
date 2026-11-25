//! Formulas and diagrams in agent answers, drawn by the hidden renderer the Markdown preview uses (E1): an answer shows
//! them as code until they are drawn, then its card is built again.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2_app_kit::NSApplication;
use splat_file_state::Stamp;
use splat_markdown::palette::Appearance;
use splat_render::Viewer;
use splat_render::images::ImageFile;
use splat_render::js::{self, Key, Owner};

use crate::common::util::main_soon;

/// The most figures the answers keep asked for; the oldest give way.
const KEPT: usize = 400;

struct Asked {
    viewer: Rc<dyn Viewer>,
    owner: Option<Owner>,
    keys: Vec<Key>,
    ready: HashSet<Key>,
}

/// The figures the AI tabs asked the renderer for.
#[derive(Default)]
pub(crate) struct Figures {
    asked: RefCell<Option<Asked>>,
    asking: Cell<bool>,
    collect: RefCell<Option<Vec<Key>>>,
}

fn with_asked<R>(f: impl FnOnce(&RefCell<Option<Asked>>) -> R) -> R {
    f(&crate::host::state().figures.asked)
}

fn with_collect<R>(f: impl FnOnce(&RefCell<Option<Vec<Key>>>) -> R) -> R {
    f(&crate::host::state().figures.collect)
}

/// Records the figures a card uses, including those it finds in the renderer's cache.
pub(crate) fn collect<R>(build: impl FnOnce() -> R) -> (R, Vec<Key>) {
    struct Capture(Option<Vec<Key>>);
    impl Drop for Capture {
        fn drop(&mut self) {
            with_collect(|slot| *slot.borrow_mut() = self.0.take());
        }
    }
    let capture = Capture(with_collect(|slot| slot.replace(Some(Vec::new()))));
    let result = build();
    let keys = with_collect(|slot| slot.borrow_mut().take().unwrap_or_default());
    drop(capture);
    (result, keys)
}

/// Whether the app shows itself dark, which the pictures of formulas and diagrams follow.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub fn is_dark(mtm: MainThreadMarker) -> bool {
    let appearance = NSApplication::sharedApplication(mtm).effectiveAppearance();
    matches!(
        splat_render::style::appearance_of(&appearance),
        Appearance::Dark | Appearance::DarkHighContrast
    )
}

/// Records `all` as what a card uses and asks for `new` on a later turn, as a card is built while the table lays out.
pub(crate) fn want_among(all: &[Key], new: Vec<Key>) {
    with_collect(|slot| {
        if let Some(collected) = slot.borrow_mut().as_mut() {
            collected.extend(all.iter().cloned());
        }
    });
    ask_for(new);
}

fn ask_for(keys: Vec<Key>) {
    if keys.is_empty() {
        return;
    }
    let added = with_asked(|asked| {
        let mut asked = asked.borrow_mut();
        let asked = asked.get_or_insert_with(|| Asked {
            viewer: Rc::new(Answers),
            owner: None,
            keys: Vec::new(),
            ready: HashSet::new(),
        });
        let before = asked.keys.len();
        for key in keys {
            if !asked.keys.contains(&key) {
                asked.keys.push(key);
            }
        }
        let excess = asked.keys.len().saturating_sub(KEPT);
        asked.keys.drain(..excess);
        let kept: HashSet<_> = asked.keys.iter().collect();
        asked.ready.retain(|key| kept.contains(key));
        asked.keys.len() != before || excess > 0
    });
    if added && !crate::host::state().figures.asking.replace(true) {
        main_soon(move || {
            crate::host::state().figures.asking.set(false);
            if let Some(mtm) = MainThreadMarker::new() {
                ask(mtm);
            }
        });
    }
}

fn ask(mtm: MainThreadMarker) {
    let Some((viewer, owner, keys)) = with_asked(|asked| {
        let asked = asked.borrow();
        let asked = asked.as_ref()?;
        Some((asked.viewer.clone(), asked.owner, asked.keys.clone()))
    }) else {
        return;
    };
    let renderer = js::renderer(mtm);
    let owner = owner.unwrap_or_else(|| renderer.listen(&viewer));
    with_asked(|asked| {
        if let Some(asked) = asked.borrow_mut().as_mut() {
            asked.owner = Some(owner);
        }
    });
    renderer.request(owner, keys);
}

/// Builds the cards anew when more of the answers' figures are drawn than when they were last built; the renderer
/// tells every document when it has run empty, also when it drew only for a preview.
fn arrived() {
    let changed = with_asked(|asked| {
        let mut asked = asked.borrow_mut();
        let Some(asked) = asked.as_mut() else { return Vec::new() };
        let ready: HashSet<_> = asked.keys.iter().filter(|key| js::ready(key)).cloned().collect();
        let changed = ready.symmetric_difference(&asked.ready).cloned().collect();
        asked.ready = ready;
        changed
    });
    if !changed.is_empty() {
        for tab in super::all() {
            tab.timeline().figures_arrived(&changed);
        }
    }
}

/// The answers of all AI tabs as one document of the renderer.
struct Answers;

impl Viewer for Answers {
    fn renders_arrived(&self) {
        main_soon(arrived);
    }

    fn image_arrived(&self, _url: String) {}

    fn image_loaded(&self, _path: PathBuf, _file: ImageFile) {}

    fn images_changed(&self, _changed: Vec<(PathBuf, Stamp)>) {}

    fn web_images_shown(&self) -> Vec<String> {
        Vec::new()
    }

    fn has_closed(&self) -> bool {
        false
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub use selftest_probe::*;
