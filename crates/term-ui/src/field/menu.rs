//! The completion menu of the input field (F93), as Warp opens it on ⌃Space, or on Tab while no suggestion shows:
//! the candidates for the word being typed with what each does, above the field where the word starts, or without
//! the field above the word at the shell's prompt, where taking one types it into the shell. Typing filters it, by scattered letters too; ↑ and ↓ or Tab
//! and ⇧Tab move, Return, → or a click take the selected one, Escape closes it. The selected one shows faint after the
//! text, as a suggestion does.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSColor, NSEvent, NSFont, NSLineBreakMode, NSTextField, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWorkspace,
};
use objc2_foundation::{NSObjectProtocol, NSPoint, NSRect, NSSize};
use term_suggest::{Candidate, Completion};

use super::suggest::LATER;
use crate::TerminalView;
use crate::kit::{guarded, ns};

/// How many candidates show at once.
const SHOWN: usize = 8;
const ROW_HEIGHT: f64 = 22.0;
/// The room around the rows and between a row's edge and its text.
const PADDING: f64 = 4.0;
const TEXT_X: f64 = 8.0;
/// The room between a candidate and what it does.
const GAP: f64 = 16.0;
const NAME_WIDEST: f64 = 280.0;
const ABOUT_WIDEST: f64 = 320.0;
const NARROWEST: f64 = 160.0;
/// The room between the menu and the field.
const ABOVE_FIELD: f64 = 4.0;

/// Which menu row a row view is.
pub(crate) struct RowIvars {
    view: u64,
    row: Cell<usize>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `MenuRow` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermCompletionRow"]
    #[ivars = RowIvars]
    struct MenuRow;

    unsafe impl NSObjectProtocol for MenuRow {}

    impl MenuRow {
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, _event: &NSEvent) {
            guarded((), || {
                if let Some(view) = crate::find(self.ivars().view) {
                    view.menu_choose_row(self.ivars().row.get());
                }
            });
        }

        #[unsafe(method(accessibilityPerformPress))]
        fn accessibility_press(&self) -> bool {
            guarded(false, || {
                if let Some(view) = crate::find(self.ivars().view) {
                    view.menu_choose_row(self.ivars().row.get());
                }
                true
            })
        }
    }
);

struct Row {
    view: Retained<MenuRow>,
    name: Retained<NSTextField>,
    about: Retained<NSTextField>,
}

/// The open menu of one field.
pub(crate) struct Menu {
    root: Retained<NSView>,
    background: Retained<NSVisualEffectView>,
    rows: Vec<Row>,
    /// It completes the line typed at the shell's prompt rather than the field's text.
    raw: bool,
    /// Where the word being completed starts in the line, in bytes.
    start: usize,
    /// Every candidate at that place, which the scattered letters choose from.
    every: Vec<Candidate>,
    /// What the menu lists, best first.
    found: Vec<Candidate>,
    /// The completion the candidates take the word's place with.
    completion: Completion,
    selected: usize,
    first: usize,
    /// It opened by itself while typing at the shell's prompt, so Return runs the line rather than choosing.
    auto: bool,
    /// The user moved the selection, so Tab takes the selected candidate rather than the suggestion.
    moved: bool,
}

fn row(mtm: MainThreadMarker, view: u64, index: usize, font: &NSFont) -> Row {
    let this = MenuRow::alloc(mtm).set_ivars(RowIvars {
        view,
        row: Cell::new(index),
    });
    // SAFETY: NSView's designated initializer, called once on the newly allocated object.
    let row_view: Retained<MenuRow> = unsafe { msg_send![super(this), initWithFrame: NSRect::ZERO] };
    row_view.setWantsLayer(true);
    if let Some(layer) = row_view.layer() {
        layer.setCornerRadius(5.0);
    }
    row_view.setAccessibilityElement(true);
    row_view.setAccessibilityRole(Some(term_render::constant!(objc2_app_kit::NSAccessibilityButtonRole)));
    let name = crate::kit::label(mtm, "");
    name.setFont(Some(font));
    name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    let about = crate::kit::label(mtm, "");
    about.setFont(Some(&NSFont::systemFontOfSize(11.0)));
    about.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
    for label in [&name, &about] {
        crate::kit::hide_from_voiceover(label);
        row_view.addSubview(label);
    }
    Row {
        view: row_view,
        name,
        about,
    }
}

impl Menu {
    /// Puts the `shown` candidates from the first on into the rows and marks the selected one; how wide the widest
    /// candidate and description are.
    fn fill(&self, shown: usize) -> (f64, f64) {
        let (mut name_width, mut about_width) = (0.0_f64, 0.0_f64);
        for (at, row) in self.rows.iter().enumerate() {
            let Some(candidate) = self.found.get(self.first + at).filter(|_| at < shown) else {
                row.view.setHidden(true);
                continue;
            };
            row.view.setHidden(false);
            row.view.ivars().row.set(at);
            row.name.setStringValue(&ns(&candidate.text));
            let about = candidate.description.as_deref().unwrap_or_default();
            row.about.setStringValue(&ns(about));
            let spoken = [candidate.text.as_str(), about]
                .iter()
                .filter(|part| !part.is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", ");
            row.view.setAccessibilityLabel(Some(&ns(&spoken)));
            let selected = self.first + at == self.selected;
            row.view.setAccessibilitySelected(selected);
            let (name_colour, about_colour, ground) = if selected {
                (
                    NSColor::alternateSelectedControlTextColor(),
                    NSColor::alternateSelectedControlTextColor(),
                    NSColor::selectedContentBackgroundColor(),
                )
            } else {
                (
                    NSColor::labelColor(),
                    NSColor::secondaryLabelColor(),
                    NSColor::clearColor(),
                )
            };
            row.name.setTextColor(Some(&name_colour));
            row.about.setTextColor(Some(&about_colour));
            if let Some(layer) = row.view.layer() {
                layer.setBackgroundColor(Some(&ground.CGColor()));
            }
            for label in [&row.name, &row.about] {
                label.sizeToFit();
            }
            name_width = name_width.max(row.name.frame().size.width.min(NAME_WIDEST));
            about_width = about_width.max(row.about.frame().size.width.min(ABOUT_WIDEST));
        }
        (name_width, about_width)
    }

    /// Places the `shown` rows top down in a menu of `size`, each description at `about_x` and each candidate
    /// `name_width` wide.
    fn lay_out(&self, shown: usize, size: NSSize, about_x: f64, name_width: f64) {
        let inner = size.width - 2.0 * PADDING;
        for (at, row) in self.rows.iter().enumerate().take(shown) {
            let top = size.height - PADDING - (at + 1) as f64 * ROW_HEIGHT;
            row.view
                .setFrame(NSRect::new(NSPoint::new(PADDING, top), NSSize::new(inner, ROW_HEIGHT)));
            let place = |label: &NSTextField, x: f64, wide: f64| {
                let tall = label.intrinsicContentSize().height;
                label.setFrame(NSRect::new(
                    NSPoint::new(x, ((ROW_HEIGHT - tall) / 2.0).round()),
                    NSSize::new(wide.max(0.0), tall),
                ));
            };
            place(&row.name, TEXT_X, name_width);
            place(&row.about, about_x, inner - about_x - TEXT_X);
        }
    }
}

/// The candidates of `completion` first, then those of `every` the word matches by scattered letters, each once.
fn merge(completion: &Completion, every: &[Candidate]) -> Vec<Candidate> {
    let mut found = completion.candidates.clone();
    let titles: Vec<(&str, &str)> = every.iter().map(|candidate| (candidate.text.as_str(), "")).collect();
    for index in crate::palette::rank(&completion.word, &titles) {
        let candidate = &every[index];
        if !found.iter().any(|known| known.text == candidate.text) {
            found.push(candidate.clone());
        }
    }
    found
}

/// The longest start all of `names` share.
fn shared_start(names: &[&str]) -> String {
    let Some(first) = names.first() else {
        return String::new();
    };
    names.iter().fold(first.to_string(), |common, name| {
        common
            .chars()
            .zip(name.chars())
            .take_while(|(a, b)| a == b)
            .map(|(a, _)| a)
            .collect()
    })
}

impl TerminalView {
    /// The candidates for the word being typed in `text`: the delayed stage's where it answered for this text, which
    /// has the dynamic sources, else the spec's and the paths.
    fn completion_for(&self, text: &str) -> Completion {
        let id = self.id();
        let later = LATER.with(|later| {
            later
                .borrow()
                .get(&id)
                .filter(|(line, _)| *line == text)
                .map(|(_, completion)| completion.clone())
        });
        later.unwrap_or_else(|| term_suggest::complete(text, &self.cwd(), &term_base::home(), true))
    }

    /// Every candidate at the place of the word being typed in `text`, which scattered letters choose from.
    fn every_for(&self, text: &str, completion: &Completion) -> Vec<Candidate> {
        let mut head = text.get(..completion.start).unwrap_or_default().to_string();
        if completion.word.starts_with('-') {
            head.push('-');
        }
        self.completion_for(&head).candidates
    }

    /// The line the menu completes: the field's text, or with `raw` the line typed at the shell's prompt.
    fn menu_line(&self, raw: bool) -> Option<String> {
        if raw {
            return self
                .with_session(|session| session.typed_line())
                .flatten()
                .map(|(typed, _)| typed.text);
        }
        self.field_view().map(|field| field.text())
    }

    /// Makes `old` read `new`: in the field as its text, at the shell's prompt as typed keys, deleting the word from
    /// byte `start` on first where `new` does not just continue it.
    fn menu_put(&self, raw: bool, old: &str, new: &str, start: usize) {
        if !raw {
            if let Some(field) = self.field_view() {
                field.set_text(new);
            }
            return;
        }
        let old_word = old.get(start..).unwrap_or_default();
        let new_word = new.get(start..).unwrap_or_default();
        let keys = match new_word.strip_prefix(old_word) {
            Some(rest) => rest.to_string(),
            None => format!("{}{new_word}", "\x7f".repeat(old_word.chars().count())),
        };
        self.with_session(|session| {
            session.scroll(term_session::Scroll::Bottom);
            session.write(keys.into_bytes());
        });
    }

    /// ⌃Space, or Tab while no suggestion shows: one candidate takes the word's place; several complete it as far as
    /// those starting with it agree and open the menu, which also lists those the word matches by scattered letters.
    /// With `raw` it completes the line typed at the shell's prompt; whether it found anything, as the shell's own
    /// completion takes Tab otherwise.
    pub(crate) fn complete_word(&self, raw: bool) -> bool {
        let Some(text) = self.menu_line(raw) else {
            return false;
        };
        let completion = self.completion_for(&text);
        let found = merge(&completion, &self.every_for(&text, &completion));
        match found.as_slice() {
            [] => return false,
            [only] => {
                let line = completion.apply(&text, &only.text);
                self.menu_put(raw, &text, &line, completion.start);
            }
            _ => {
                let names: Vec<&str> = completion.candidates.iter().map(|c| c.text.as_str()).collect();
                let common = shared_start(&names);
                let mut shown = text.clone();
                if common.chars().count() > completion.word.chars().count() {
                    let line = completion.apply(&text, &common);
                    shown = if common.ends_with(['/', '=']) {
                        line
                    } else {
                        line.trim_end().to_string()
                    };
                    self.menu_put(raw, &text, &shown, completion.start);
                }
                self.menu_open(raw, &shown, false);
            }
        }
        true
    }

    /// Tab or ⌃Space in the field; a beep when nothing fits.
    pub(crate) fn field_complete(&self) {
        if !self.complete_word(false) {
            objc2_app_kit::NSBeep();
        }
    }

    /// Opens the menu for `text`, the field's or with `raw` the line at the prompt, with `auto` as one that opened by
    /// itself while typing; nothing with fewer than two candidates.
    pub(super) fn menu_open(&self, raw: bool, text: &str, auto: bool) {
        let Some(font) = self.field_view().map(|field| field.font()) else {
            return;
        };
        let completion = self.completion_for(text);
        let every = self.every_for(text, &completion);
        let found = merge(&completion, &every);
        if found.len() < 2 {
            return;
        }
        let start = completion.start;
        let mtm = self.mtm();
        let background = NSVisualEffectView::new(mtm);
        background.setMaterial(NSVisualEffectMaterial::Menu);
        background.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        background.setState(NSVisualEffectState::Active);
        background.setWantsLayer(true);
        let increase_contrast = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
        if let Some(layer) = background.layer() {
            layer.setCornerRadius(8.0);
            layer.setMasksToBounds(true);
            layer.setBorderWidth(if increase_contrast { 1.0 } else { 0.5 });
            let border = if increase_contrast {
                NSColor::secondaryLabelColor()
            } else {
                NSColor::separatorColor()
            };
            layer.setBorderColor(Some(&border.CGColor()));
        }
        let root = NSView::new(mtm);
        root.setWantsLayer(true);
        if let Some(layer) = root.layer() {
            layer.setShadowOpacity(0.25);
            layer.setShadowRadius(10.0);
            layer.setShadowOffset(NSSize::new(0.0, -4.0));
        }
        root.addSubview(&background);
        let rows: Vec<Row> = (0..SHOWN).map(|index| row(mtm, self.id(), index, &font)).collect();
        for row in &rows {
            background.addSubview(&row.view);
        }
        root.setAccessibilityElement(true);
        root.setAccessibilityRole(Some(term_render::constant!(objc2_app_kit::NSAccessibilityListRole)));
        self.addSubview(&root);
        *self.ivars().menu.borrow_mut() = Some(Menu {
            root,
            background,
            rows,
            raw,
            start,
            every,
            found,
            completion,
            selected: 0,
            first: 0,
            auto,
            moved: false,
        });
        self.menu_show(text);
    }

    /// Whether the menu at the shell's prompt opened by itself while typing, and whether the user moved its selection.
    pub(super) fn raw_menu_auto(&self) -> Option<(bool, bool)> {
        let menu = self.ivars().menu.try_borrow().ok()?;
        menu.as_ref()
            .filter(|menu| menu.raw)
            .map(|menu| (menu.auto, menu.moved))
    }

    /// Takes the selected candidate into the line and closes the menu.
    pub(super) fn menu_take_selected(&self) {
        self.menu_choose(None);
    }

    /// Whether the menu is open.
    pub(crate) fn menu_is_open(&self) -> bool {
        self.ivars().menu.try_borrow().is_ok_and(|menu| menu.is_some())
    }

    /// Whether the menu is open for the line at the shell's prompt.
    pub(crate) fn raw_menu_is_open(&self) -> bool {
        self.ivars()
            .menu
            .try_borrow()
            .is_ok_and(|menu| menu.as_ref().is_some_and(|menu| menu.raw))
    }

    /// Closes the menu without looking at the line again, as something else changes it next.
    pub(crate) fn menu_close_quietly(&self) {
        if let Ok(mut menu) = self.ivars().menu.try_borrow_mut()
            && let Some(menu) = menu.take()
        {
            menu.root.removeFromSuperview();
        }
    }

    /// Closes the menu; the suggestion of the field or of the prompt shows again.
    pub(crate) fn menu_close(&self) {
        let Some(menu) = self.ivars().menu.borrow_mut().take() else {
            return;
        };
        menu.root.removeFromSuperview();
        if menu.raw {
            self.raw_suggest();
        } else {
            self.field_text_changed();
        }
    }

    /// The line changed while the menu is open: it lists what still fits, or closes once nothing does or another word
    /// is being typed.
    pub(crate) fn menu_follow(&self, text: &str) {
        if !self.menu_is_open() {
            return;
        }
        let completion = self.completion_for(text);
        let keep = self.ivars().menu.borrow_mut().as_mut().is_some_and(|menu| {
            if completion.start != menu.start {
                return false;
            }
            menu.found = merge(&completion, &menu.every);
            menu.completion = completion;
            menu.selected = 0;
            menu.first = 0;
            !menu.found.is_empty()
        });
        if keep {
            self.menu_show(text);
        } else {
            self.menu_close_quietly();
        }
    }

    /// A key while the menu is open: whether the menu took it, or none when it is not open or leaves the key alone.
    pub(crate) fn menu_key(&self, code: u16, shift: bool, at_end: bool) -> Option<bool> {
        if !self.menu_is_open() {
            return None;
        }
        let step: i8 = match code {
            125 => 1,
            126 => -1,
            48 if shift => -1,
            48 => 1,
            36 | 76 => {
                self.menu_choose(None);
                return Some(true);
            }
            124 if at_end => {
                self.menu_choose(None);
                return Some(true);
            }
            53 => {
                self.menu_close();
                return Some(true);
            }
            _ => return None,
        };
        let raw = self.ivars().menu.borrow_mut().as_mut().map(|menu| {
            menu.moved = true;
            let count = menu.found.len().max(1);
            menu.selected = if step > 0 {
                (menu.selected + 1) % count
            } else {
                (menu.selected + count - 1) % count
            };
            menu.raw
        })?;
        if let Some(text) = self.menu_line(raw) {
            self.menu_show(&text);
        }
        Some(true)
    }

    /// A click on row `row`.
    fn menu_choose_row(&self, row: usize) {
        let first = self.ivars().menu.borrow().as_ref().map(|menu| menu.first);
        if let Some(first) = first {
            self.menu_choose(Some(first + row));
        }
    }

    /// Takes the candidate at `index`, else the selected one, into the line and closes the menu.
    fn menu_choose(&self, index: Option<usize>) {
        let Some(menu) = self.ivars().menu.borrow_mut().take() else {
            return;
        };
        menu.root.removeFromSuperview();
        let Some(candidate) = menu.found.get(index.unwrap_or(menu.selected)) else {
            return;
        };
        if let Some(text) = self.menu_line(menu.raw) {
            let line = menu.completion.apply(&text, &candidate.text);
            self.menu_put(menu.raw, &text, &line, menu.completion.start);
        }
    }

    /// Where the word being completed starts on the screen: in the field, or with `raw` the cell at the prompt.
    fn menu_anchor(&self, raw: bool, text: &str, start: usize) -> Option<NSRect> {
        if !raw {
            return self.field_view().and_then(|field| field.rect_of(start, self));
        }
        let (_, cursor) = self.with_session(|session| session.typed_line()).flatten()?;
        let word = text.get(start..).unwrap_or_default().chars().count();
        let point = term_session::Point {
            line: cursor.line,
            column: cursor.column.saturating_sub(word),
        };
        Some(self.metrics().cell_rect(point, 1))
    }

    /// Fills the rows around the selected candidate and places the menu: over the field where the word starts, or
    /// above the prompt's word, below it where there is no room above. In the field the selected candidate shows
    /// faint after the text.
    fn menu_show(&self, text: &str) {
        let Some((raw, start)) = self.ivars().menu.borrow().as_ref().map(|menu| (menu.raw, menu.start)) else {
            return;
        };
        let anchor = self.menu_anchor(raw, text, start);
        let bounds = self.bounds();
        let room = if raw {
            bounds.size.height
        } else {
            bounds.size.height - ABOVE_FIELD
        };
        let fit = (1..=SHOWN)
            .take_while(|rows| 2.0 * PADDING + *rows as f64 * ROW_HEIGHT <= room)
            .last()
            .unwrap_or(1);
        let Some(shown) = self.ivars().menu.borrow_mut().as_mut().map(|menu| {
            let shown = menu.found.len().min(fit);
            menu.first = crate::palette::first_shown(menu.first, menu.selected, shown, menu.found.len());
            shown
        }) else {
            return;
        };
        let guard = self.ivars().menu.borrow();
        let Some(menu) = guard.as_ref() else { return };
        let (name_width, about_width) = menu.fill(shown);
        let gap = if about_width > 0.0 { GAP } else { 0.0 };
        let width = (2.0 * (PADDING + TEXT_X) + name_width + gap + about_width)
            .max(NARROWEST)
            .min(bounds.size.width - 2.0 * PADDING);
        let height = 2.0 * PADDING + shown as f64 * ROW_HEIGHT;
        let left = anchor.map_or(PADDING, |rect| rect.origin.x - PADDING - TEXT_X);
        let x = left.min(bounds.size.width - width - PADDING).max(PADDING);
        let y = match anchor.filter(|_| raw) {
            Some(cell) if cell.origin.y - height - ABOVE_FIELD >= 0.0 => cell.origin.y - height - ABOVE_FIELD,
            Some(cell) => cell.origin.y + cell.size.height + ABOVE_FIELD,
            None => bounds.size.height - height - ABOVE_FIELD,
        };
        menu.root
            .setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(width, height)));
        menu.background
            .setFrame(NSRect::new(NSPoint::ZERO, NSSize::new(width, height)));
        menu.lay_out(shown, NSSize::new(width, height), TEXT_X + name_width + gap, name_width);
        let preview = menu.found.get(menu.selected).map(|candidate| {
            let line = menu.completion.apply(text, &candidate.text);
            line.trim_end().to_string()
        });
        drop(guard);
        if raw {
            return;
        }
        let rest = preview
            .as_deref()
            .and_then(|line| line.strip_prefix(text))
            .filter(|rest| !rest.is_empty());
        *self.ivars().suggestion.borrow_mut() = preview.clone().filter(|_| rest.is_some());
        self.ivars().suggestion_sure.set(false);
        if let Some(field) = self.field_view() {
            field.set_ghost(rest);
        }
    }

    /// What the open menu lists, the selected one first, for the self-test.
    #[cfg(feature = "selftest")]
    pub fn menu_entries(&self) -> Option<Vec<String>> {
        let menu = self.ivars().menu.borrow();
        let menu = menu.as_ref()?;
        let mut entries: Vec<String> = menu.found.iter().map(|candidate| candidate.text.clone()).collect();
        if menu.selected < entries.len() {
            let selected = entries.remove(menu.selected);
            entries.insert(0, selected);
        }
        Some(entries)
    }

    /// Opens the field's menu as Tab does unless a menu is open, and moves `steps` down in it, for the self-test.
    #[cfg(feature = "selftest")]
    pub fn menu_drive(&self, steps: usize) {
        if !self.menu_is_open() {
            self.field_complete();
        }
        for _ in 0..steps {
            let _ = self.menu_key(125, false, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_suggest::Kind;

    fn candidate(text: &str) -> Candidate {
        Candidate {
            text: text.into(),
            description: None,
            kind: Kind::Subcommand,
        }
    }

    #[test]
    fn the_word_finds_its_starts_first_and_scattered_letters_after() {
        let every = vec![candidate("checkout"), candidate("cherry-pick"), candidate("commit")];
        let completion = Completion {
            start: 4,
            word: "cmt".into(),
            candidates: Vec::new(),
            ..Completion::default()
        };
        assert_eq!(
            merge(&completion, &every)
                .iter()
                .map(|c| c.text.as_str())
                .collect::<Vec<_>>(),
            ["commit"]
        );
        let typed = Completion {
            word: "ch".into(),
            candidates: vec![candidate("checkout"), candidate("cherry-pick")],
            ..completion
        };
        assert_eq!(merge(&typed, &every).len(), 2);
        assert_eq!(shared_start(&["checkout", "cherry-pick"]), "che");
    }
}
