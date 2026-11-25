//! The mark on links of the preview that lead nowhere, as splat sets it: to a file that is not there, a note no folder
//! below holds, or a heading the document lacks.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;

use splat_editor_state::links::{Destination, destination, missing_on_disk};
use splat_editor_state::state::links_of;
use splat_preview::layout::Layout;

/// The links of a layout that can lead nowhere, by `Link::attribute`, and the anchors among them without a place.
#[derive(Default)]
pub(super) struct Wanted {
    shown: HashMap<String, Destination>,
    missing_anchors: HashSet<String>,
}

/// The links of `laid`, the layout of `text`, that can lead nowhere, read off the main thread.
pub(super) fn wanted(laid: &Layout, doc: &splat_markdown::doc::Doc, text: &str, doc_dir: Option<&Path>) -> Wanted {
    let mut shown = HashMap::new();
    for link in laid.blocks.iter().flat_map(|block| links_of(block)) {
        if let Some(wanted) = destination(link, doc_dir) {
            shown.entry(link.attribute()).or_insert(wanted);
        }
    }
    let anchors: Vec<&String> = shown
        .values()
        .filter_map(|d| match d {
            Destination::Anchor(anchor) => Some(anchor),
            _ => None,
        })
        .collect();
    let missing_anchors = if anchors.is_empty() {
        HashSet::new()
    } else {
        let headings: HashSet<String> = splat_markdown::commands::heading_anchors(doc)
            .into_iter()
            .map(|h| h.anchor)
            .collect();
        anchors
            .into_iter()
            .filter(|anchor| {
                let anchor = decoded_anchor(anchor);
                !headings.contains(&anchor) && !names_an_element(text, &anchor)
            })
            .cloned()
            .collect()
    };
    Wanted { shown, missing_anchors }
}

/// The file watcher's looks between two searches for a note that was missing: each walks the folder below the document.
const LOOKS_PER_SEARCH: u32 = 5;

/// What a preview knows about where its links lead.
#[derive(Default)]
pub(super) struct LinkTargets {
    nowhere: RefCell<Rc<HashSet<String>>>,
    wanted: RefCell<Wanted>,
    /// Whether nothing lies at each destination the disk was asked about.
    on_disk: RefCell<HashMap<Destination, bool>>,
    /// A look at the disk is under way; the next one waits for its answer.
    looking: Cell<bool>,
    looks: Cell<u32>,
}

impl LinkTargets {
    /// The links shown that lead nowhere, for the renderer.
    pub(super) fn nowhere(&self) -> Rc<HashSet<String>> {
        self.nowhere.borrow().clone()
    }

    pub(super) fn take(&self, wanted: Wanted) {
        self.wanted.replace(wanted);
    }

    /// Whether a look at the disk is under way.
    pub(super) fn looking(&self) -> bool {
        self.looking.get()
    }

    /// The destinations to ask the disk about: those it has not answered for, and with `again` also those it has,
    /// except notes it found missing, which only every few looks; `None` when nothing is to ask.
    pub(super) fn to_ask(&self, again: bool) -> Option<HashSet<Destination>> {
        if again {
            self.looks.set(self.looks.get().wrapping_add(1));
        }
        let search = self.looks.get().is_multiple_of(LOOKS_PER_SEARCH);
        let on_disk = self.on_disk.borrow();
        let wanted: HashSet<Destination> = self
            .wanted
            .borrow()
            .shown
            .values()
            .filter(|d| !matches!(d, Destination::Anchor(_)))
            .filter(|d| match on_disk.get(*d) {
                None => true,
                Some(_) if !again => false,
                Some(true) => search || !matches!(d, Destination::Note { .. }),
                Some(false) => true,
            })
            .cloned()
            .collect();
        if wanted.is_empty() {
            return None;
        }
        self.looking.set(true);
        Some(wanted)
    }

    pub(super) fn answered(&self, found: Vec<(Destination, Option<bool>)>) {
        self.looking.set(false);
        let mut on_disk = self.on_disk.borrow_mut();
        for (d, missing) in found {
            on_disk.insert(d, missing.unwrap_or(false));
        }
    }

    /// Takes over what is known now; the links that gained or lost their mark.
    pub(super) fn settle(&self) -> HashSet<String> {
        let nowhere: HashSet<String> = {
            let on_disk = self.on_disk.borrow();
            let wanted = self.wanted.borrow();
            wanted
                .shown
                .iter()
                .filter(|(_, d)| match d {
                    Destination::Anchor(anchor) => wanted.missing_anchors.contains(anchor),
                    other => on_disk.get(*other).copied().unwrap_or(false),
                })
                .map(|(key, _)| key.clone())
                .collect()
        };
        let changed: HashSet<String> = nowhere.symmetric_difference(&self.nowhere.borrow()).cloned().collect();
        if !changed.is_empty() {
            self.nowhere.replace(Rc::new(nowhere));
        }
        changed
    }
}

/// Whether nothing lies at each of `wanted`; it reads the disk.
pub(super) fn look(wanted: HashSet<Destination>) -> Vec<(Destination, Option<bool>)> {
    wanted
        .into_iter()
        .map(|d| {
            let missing = missing_on_disk(&d);
            (d, missing)
        })
        .collect()
}

/// An anchor as the heading has it, or as it stands where it is no valid percent encoding.
fn decoded_anchor(anchor: &str) -> String {
    term_ui::kit::ns(anchor)
        .stringByRemovingPercentEncoding()
        .map_or_else(|| anchor.to_string(), |s| s.to_string())
}

/// Whether HTML in `text` gives an element the `id` or `name` `anchor`, which a link may point to as well as to a
/// heading.
fn names_an_element(text: &str, anchor: &str) -> bool {
    ["id=\"", "id='", "name=\"", "name='"].iter().any(|attribute| {
        let quote = &attribute[attribute.len() - 1..];
        text.contains(&format!("{attribute}{anchor}{quote}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted_in(text: &str, dir: &Path) -> Wanted {
        let doc = splat_markdown::reader::parse(text);
        let laid = splat_preview::layout::layout(&doc, &splat_preview::layout::NoImages, None);
        wanted(&laid, &doc, text, Some(dir))
    }

    #[test]
    fn an_anchor_of_an_html_element_counts_as_a_place() {
        let text = "<a id=\"oben\"></a>\n<span name='mitte'>x</span>\n";
        assert!(names_an_element(text, "oben"));
        assert!(names_an_element(text, "mitte"));
        assert!(!names_an_element(text, "ob"));
        assert!(!names_an_element(text, "unten"));
    }

    #[test]
    fn anchors_without_a_heading_and_missing_files_lead_nowhere() {
        let dir = std::env::temp_dir().join(format!("term-preview-links-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("da.md"), "# Da").unwrap();
        let text = "# Oben\n\n[a](#oben) [b](#unten) [c](da.md) [d](fehlt.md) [e](https://example.com)\n";
        let targets = LinkTargets::default();
        targets.take(wanted_in(text, &dir));
        let asked = targets.to_ask(false).unwrap();
        assert_eq!(asked.len(), 2, "the two files, not the anchors or the web");
        targets.answered(look(asked));
        let changed = targets.settle();
        let mut nowhere: Vec<String> = targets.nowhere().iter().cloned().collect();
        nowhere.sort();
        assert_eq!(nowhere.len(), 2);
        assert_eq!(changed.len(), 2);
        assert!(nowhere.iter().any(|n| n.ends_with("unten")));
        assert!(nowhere.iter().any(|n| n.contains("fehlt.md")));
        assert!(targets.to_ask(false).is_none(), "nothing new to ask");
        assert!(targets.settle().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
