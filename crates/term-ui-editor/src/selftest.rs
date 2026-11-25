//! What the self-test reads from and does with editors, through the code their commands run; an editor is named by
//! its serial, a workspace's editor by its file.

use std::ops::Range;
use std::path::Path;

use objc2::DefinedClass;
use objc2::rc::Retained;
use objc2_app_kit::{NSTextInputClient, NSTextView};
use objc2_foundation::NSRange;
use term_ui::workspace_host::{AsWorkspace, TabId};

use crate::{EditorSerial, EditorTextView, find_editor};

/// Has the next save panel offer `path` instead of the document's own name and folder.
pub fn offer_in_save_panel(path: &Path) {
    crate::saving::offer_next(path.to_path_buf());
}

/// Saves the editors of `path` for Splat as "In Splat öffnen" does and waits; whether nothing is left unsaved.
pub fn saved_for_splat(path: &Path) -> bool {
    crate::markdown::selftest::saved_for_splat(path)
}

/// A question's message, informative text and buttons with their keys.
pub type Question = (String, String, Vec<(String, String)>);

/// A change mark's kind and its lines.
pub type Mark = (String, Range<usize>);

fn view(editor: EditorSerial) -> Option<Retained<EditorTextView>> {
    find_editor(editor)
}

/// The editor of `path` in `workspace`.
pub fn editor_of(workspace: &(impl AsWorkspace + ?Sized), path: &Path) -> Option<EditorSerial> {
    let path = crate::canonical(path);
    crate::editors_in(&workspace.as_workspace())
        .into_iter()
        .find(|view| view.document().path() == Some(path.as_path()))
        .map(|view| view.ivars().serial)
}

/// The editors of `workspace` with their tabs.
pub fn editors_of(workspace: &(impl AsWorkspace + ?Sized)) -> Vec<(TabId, EditorSerial)> {
    crate::editors_in(&workspace.as_workspace())
        .into_iter()
        .filter_map(|view| Some((view.ivars().tab.get()?, view.ivars().serial)))
        .collect()
}

/// What `editor` shows, and what its document holds, which must be the same.
pub fn texts(editor: EditorSerial) -> Option<(String, String)> {
    let view = view(editor)?;
    let shown = (*view).string().to_string();
    let held = view.document().text().to_string();
    Some((shown, held))
}

/// Whether `editor` has unsaved changes.
pub fn is_dirty(editor: EditorSerial) -> bool {
    view(editor).is_some_and(|view| view.document().is_dirty())
}

/// Types `text` into `editor` as the keyboard would, through the view's input path.
pub fn type_text(editor: EditorSerial, text: &str) {
    let Some(view) = view(editor) else { return };
    // SAFETY: The inserted object is an NSString, one of the two types this method accepts.
    unsafe {
        view.insertText_replacementRange(
            &crate::util::ns(text),
            NSRange::new(objc2_foundation::NSNotFound.cast_unsigned(), 0),
        );
    }
}

/// Presses Return in `editor`.
pub fn type_newline(editor: EditorSerial) {
    if let Some(view) = view(editor) {
        crate::editing::newline(&view);
    }
}

/// Selects the UTF-16 range `range` of `editor`.
pub fn select(editor: EditorSerial, range: Range<usize>) {
    if let Some(view) = view(editor) {
        view.setSelectedRange(NSRange::new(range.start, range.len()));
    }
}

/// Saves `editor` and waits until the file is written; `save_as` asks for a file. Whether it was written.
pub fn save(editor: EditorSerial, save_as: bool) -> bool {
    view(editor).is_some_and(|view| crate::saving::save(&view, save_as))
}

/// Saves every editor with unsaved changes, those of gone files aside, and waits; whether all were saved.
pub fn save_all() -> bool {
    crate::closing::save_all()
}

/// How many open editors have unsaved changes.
pub fn dirty_count() -> usize {
    crate::closing::dirty_count()
}

/// What the bar above `editor` says and offers while it shows.
pub fn conflict_bar(editor: EditorSerial) -> Option<(String, Vec<String>)> {
    crate::conflict::inspect(&*view(editor)?)
}

/// Clicks the button titled `title` of the bar above `editor`; whether there was one.
pub fn press_conflict_button(editor: EditorSerial, title: &str) -> bool {
    view(editor).is_some_and(|view| crate::conflict::press(&view, title))
}

/// The message, informative text and buttons with their keys of the question before the text of a gone file goes.
pub fn discard_question(editor: EditorSerial) -> Option<Question> {
    Some(crate::conflict::inspect_question(&*view(editor)?))
}

/// The text of the tab with the differences between the disk and the editor for `path`.
pub fn compared(path: &Path) -> Option<String> {
    crate::compare::shown(path)
}

/// Turns Vim's keys on or off in every editor.
pub fn toggle_vim() {
    crate::vim::toggle();
}

/// The mode label of `editor`'s status bar and whether the system caret is hidden for Vim's block.
pub fn vim_shown(editor: EditorSerial) -> Option<(String, bool)> {
    Some(crate::vim::shown(&*view(editor)?))
}

/// The UTF-16 range of the search match Vim marks in `editor`.
pub fn vim_marked(editor: EditorSerial) -> Option<Range<usize>> {
    crate::vim::marked(&*view(editor)?)
}

/// Whether the keyboard layout in use types ASCII.
pub fn vim_types_ascii() -> bool {
    crate::vim::types_ascii()
}

/// How many places Vim and the editor type at in `editor`.
pub fn carets(editor: EditorSerial) -> usize {
    view(editor).map_or(0, |view| crate::vim::carets(&view))
}

/// Gives `editor` carets or selections at the UTF-16 ranges `at`.
pub fn set_carets(editor: EditorSerial, at: Vec<Range<usize>>) {
    if let Some(view) = view(editor) {
        crate::multi::set(&view, at);
    }
}

/// The carets and selections of `editor` beyond the first.
pub fn extra_carets(editor: EditorSerial) -> Vec<Range<usize>> {
    view(editor).map_or_else(Vec::new, |view| crate::multi::carets(&view))
}

/// Folds or unfolds every range of `editor`.
pub fn fold_all(editor: EditorSerial, fold: bool) {
    if let Some(view) = view(editor) {
        if fold {
            crate::fold::fold_all(&view);
        } else {
            crate::fold::unfold_all(&view);
        }
    }
}

/// How many folds of `editor` are folded, and the lines TextKit lays out, folded ones missing.
pub fn folds(editor: EditorSerial) -> Option<(usize, Vec<usize>)> {
    let view = view(editor)?;
    Some((crate::fold::folded(&view), crate::fold::laid_out_lines(&view)))
}

/// Whether `editor` shows its whitespace.
pub fn whitespace_visible(editor: EditorSerial) -> bool {
    view(editor).is_some_and(|view| crate::whitespace::visible(&view))
}

/// The indentation `editor`'s status bar shows, and its menu's checked entry.
pub fn indent_shown(editor: EditorSerial) -> Option<(String, Option<String>)> {
    Some(crate::indent::shown(&*view(editor)?))
}

/// Whether the lines on screen of `editor` have their colours.
pub fn colours_painted(editor: EditorSerial) -> bool {
    view(editor).is_some_and(|view| crate::highlight::colours_painted(&view))
}

/// Whether the file of `editor` in the last commit is known, and its change marks as kind and lines.
pub fn change_marks(editor: EditorSerial) -> Option<(bool, Vec<Mark>)> {
    let seen = crate::changes::seen(&*view(editor)?);
    let hunks = seen
        .hunks
        .into_iter()
        .map(|(mark, lines)| (format!("{mark:?}"), lines))
        .collect();
    Some((seen.head, hunks))
}

/// The committed lines the popover of a change mark of `editor` shows, while it shows.
pub fn change_popover(editor: EditorSerial) -> Option<String> {
    crate::changes::seen(&*view(editor)?).popover
}

/// Clicks the change mark of `line`, counted from 0, in the line number column of `editor`; whether it opened.
pub fn click_change(editor: EditorSerial, line: usize) -> bool {
    let Some(view) = view(editor) else { return false };
    let Some(gutter) = view
        .ivars()
        .around
        .gutter
        .borrow()
        .as_ref()
        .and_then(objc2::rc::Weak::load)
    else {
        return false;
    };
    let Some(start) = view.line_start(line) else {
        return false;
    };
    let Some(row) = crate::text_view::rect_of(&view, start..start + 1) else {
        return false;
    };
    let row = gutter.convertRect_fromView(row, Some(&view));
    let width = gutter.bounds().size.width;
    let at = objc2_foundation::NSRect::new(
        objc2_foundation::NSPoint::new(width - 8.0, row.origin.y),
        objc2_foundation::NSSize::new(8.0, row.size.height),
    );
    crate::changes::clicked(&view, line, &gutter, at)
}

/// Clicks the button of the popover of a change mark, which puts the committed lines back; whether there was one.
pub fn press_change_popover(editor: EditorSerial) -> bool {
    let Some((_, button)) = view(editor).and_then(|view| crate::changes::popover_parts(&view)) else {
        return false;
    };
    // SAFETY: A click on the popover's button, whose target is the editor, as a person's click would be.
    unsafe { button.performClick(None) };
    true
}

/// Saves the editors of files below `root` and waits; whether none is left unsaved.
pub fn save_below_now(root: &Path) -> bool {
    crate::closing::save_below_now(root)
}

/// The lines of `editor` that can fold and those folded, both counted from 0.
pub fn fold_gutter(editor: EditorSerial) -> Option<(Vec<usize>, Vec<usize>)> {
    let view = view(editor)?;
    let gutter = crate::fold::gutter(&view);
    let lines = view.document().text().line_count();
    let foldable = (0..lines).filter(|&line| gutter.get(line).is_some()).collect();
    Some((foldable, gutter.folded().collect()))
}

/// Whether the first `fn ` of the text of `editor` shows in the keyword colour.
pub fn keyword_coloured(editor: EditorSerial) -> bool {
    let Some(view) = view(editor) else { return false };
    let text = view.document().text().to_string();
    let Some(at) = text.find("fn ") else { return false };
    let at = text[..at].encode_utf16().count();
    let expected: Retained<objc2::runtime::AnyObject> =
        crate::style::token_color(splat_markdown::code::CodeToken::Keyword).into();
    crate::highlight::shown_color(&view, at).is_some_and(|shown| std::ptr::eq(&*shown, &*expected))
}

/// Clicks the format bar's button of the Markdown command named `title`, as the bar shows it; whether there was one.
pub fn press_format_button(editor: EditorSerial, title: &str) -> bool {
    let Some(view) = view(editor) else { return false };
    let button = {
        let format = view.ivars().around.format.borrow();
        let Some(bar) = format.as_ref() else { return false };
        crate::markdown::Command::named(title).and_then(|command| bar.button_of(command))
    };
    button.is_some_and(|button| {
        // SAFETY: A click on a button of the bar, whose target is the bar's, as a person's click would be.
        unsafe { button.performClick(None) };
        true
    })
}

/// The title the format bar of `editor` shows, `None` while it is hidden.
pub fn format_bar_title(editor: EditorSerial) -> Option<String> {
    let view = view(editor)?;
    let around = &view.ivars().around;
    around
        .format
        .borrow()
        .as_ref()
        .and_then(crate::markdown::FormatBar::shown_title)
}

/// The `editor-undo` scenario of Dango (E96): `steps` pseudo-random replacements, each an undo step of its own, over
/// text with characters outside the BMP and all three line breaks; then undo walks back through every text and redo
/// forward to the last, with the document always holding what the view shows. What went wrong, else how many steps.
pub fn undo_walk(editor: EditorSerial, steps: usize) -> Result<String, String> {
    let view = view(editor).ok_or("the editor is open")?;
    let undo = view.undoManager().ok_or("the editor has an undo manager")?;
    let text_view: &NSTextView = &view;
    let mut texts = vec![view.document().text().to_string()];
    let mut random = Random(0x00da_6e0d_1234_5678);
    for _ in 0..steps {
        let before = text_view.string().to_string();
        let places = boundaries(&before);
        let (a, b) = (places[random.next(places.len())], places[random.next(places.len())]);
        let range = a.min(b)..a.max(b);
        let new: String = (0..random.next(4)).map(|_| PIECES[random.next(PIECES.len())]).collect();
        if range.is_empty() && new.is_empty() {
            continue;
        }
        crate::util::undo_step(&view, || {
            view.setSelectedRange(NSRange::new(range.start, range.len()));
            type_text(editor, &new);
        });
        texts.push(text_view.string().to_string());
        if text_view.string().to_string() != view.document().text().to_string() {
            return Err(format!(
                "after edit {} the document holds other text than the view",
                texts.len() - 1
            ));
        }
    }
    let mut back = Vec::new();
    for _ in 1..texts.len() {
        undo.undo();
        back.push(view.document().text().to_string());
    }
    let expected: Vec<String> = texts.iter().rev().skip(1).cloned().collect();
    if let Some(at) = back.iter().zip(&expected).position(|(got, want)| got != want) {
        return Err(format!(
            "undo step {at} gives {:?} instead of {:?}",
            back.get(at),
            expected.get(at)
        ));
    }
    for _ in 1..texts.len() {
        undo.redo();
    }
    if Some(&view.document().text().to_string()) != texts.last() {
        return Err(String::from("redo does not come forward to the last text"));
    }
    while undo.canUndo() {
        undo.undo();
    }
    Ok(format!("{} steps", texts.len() - 1))
}

/// The text the `editor-undo` scenario starts from: characters outside the BMP and all three line breaks.
pub const UNDO_START: &str = "fn main() {\r\n    let ä = \"😀\";\n}\rtail\n";
/// What the replacements are made of; no brackets or quotes, which the editor would pair.
const PIECES: [&str; 8] = ["a", "xyz", "ä", "😀", "e\u{301}", "\n", "\r\n", " "];

/// xorshift64, so that every run makes the same edits.
struct Random(u64);

impl Random {
    fn next(&mut self, below: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        usize::try_from(self.0 % u64::try_from(below.max(1)).unwrap_or(1)).unwrap_or(0)
    }
}

/// The UTF-16 offsets of the character boundaries of `text`.
fn boundaries(text: &str) -> Vec<usize> {
    let mut out = vec![0];
    let mut utf16 = 0;
    for c in text.chars() {
        utf16 += c.len_utf16();
        out.push(utf16);
    }
    out
}
