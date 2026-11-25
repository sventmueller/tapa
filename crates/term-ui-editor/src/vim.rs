//! The Vim mode of the editor (B26, E105, E106), off by default, from Editor ▸ Vim-Modus: `term_vim` decides what a
//! key does on its copy of the text, and this module feeds it the keys and applies what it hands back. Every key goes
//! through the input context, so that dead keys of a German keyboard work; in Normal and Visual mode
//! `insertText:` and `doCommandBySelector:` give their result to Vim instead of the text. Insert mode types as the
//! editor always does, but through Vim after `I` or `A` in Visual block mode, and a pass from `i` to Esc is one undo
//! step. ⌃-keys go to Vim before the menu bar while the editor has the focus. The caret is a block outside Insert mode,
//! drawn by the overlay.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{NSColor, NSEvent, NSEventModifierFlags, NSMenuItem, NSTextField};
use objc2_foundation::{NSRange, NSString};
use splat_i18n::{t, tf};
use term_document::{Edit, Unit};
use term_vim::{Effect, Mode, Note, Outcome, Vim};

use super::view::EditorTextView;
use crate::util::OnDrop;

mod closing;
mod keyboard;
mod mark;

/// Whether editors use Vim's keys, as the user last chose; read from the settings once.
#[derive(Default)]
pub(crate) struct Enabled(Cell<Option<bool>>);

impl Enabled {
    /// Reads the setting again on next use.
    pub(crate) fn forget(&self) {
        self.0.set(None);
    }
}

/// What Vim holds for one editor.
#[derive(Default)]
pub(crate) struct State {
    vim: Option<Vim>,
    /// Vim's own edits are being applied, which its copy already has.
    applying: bool,
    /// An undo group of an insert pass is open.
    grouping: bool,
    /// The mode shown last, to announce a change.
    shown: Option<Mode>,
    /// The mode in the status bar.
    label: Option<Retained<NSTextField>>,
    /// The system clipboard's change count Vim's registers last took.
    clipboard: isize,
    /// The document's lines end in CRLF, as its whole text showed when Vim last took it.
    crlf: bool,
    /// How the document closes once the key that typed `:q`, `:q!`, `:wq` or `:x` is handled.
    closing: Option<closing::Closing>,
    /// The UTF-16 range of the last search's match, marked until `:noh`.
    marked: Option<std::ops::Range<usize>>,
}

pub(crate) use closing::due as close_due;

/// Whether editors use Vim's keys.
pub(crate) fn enabled() -> bool {
    let state = crate::state();
    let cell = &state.vim.0;
    cell.get().unwrap_or_else(|| {
        let value = crate::host::settings().vim_mode;
        cell.set(Some(value));
        value
    })
}

/// Turns the Vim mode on or off in every editor and remembers the choice.
pub(crate) fn toggle() {
    let value = !enabled();
    crate::state().vim.0.set(Some(value));
    crate::host::host().set_vim_mode(value);
    for view in super::open_editors() {
        attach(&view);
    }
}

/// Whether `item` is the menu item of the Vim mode, which then gets its check mark.
pub(crate) fn validate(item: &NSMenuItem) -> Option<bool> {
    if item.action() != Some(sel!(termToggleVimMode:)) {
        return None;
    }
    item.setState(if enabled() {
        objc2_app_kit::NSControlStateValueOn
    } else {
        objc2_app_kit::NSControlStateValueOff
    });
    Some(true)
}

/// Gives `view` Vim's state while the mode is on, in Normal mode at its caret, or takes it away.
pub(crate) fn attach(view: &EditorTextView) {
    close_group(view);
    let vim = enabled().then(|| {
        let text = view.document().text().to_string();
        let mut vim = Vim::new(&text);
        let (line, column) = line_column(view, view.selectedRange().location);
        vim.set_cursor(line, column);
        (vim, text.contains("\r\n"))
    });
    let mut state = view.ivars().vim.borrow_mut();
    state.crlf = vim.as_ref().is_some_and(|(_, crlf)| *crlf);
    state.vim = vim.map(|(vim, _)| vim);
    drop(state);
    show(view);
}

/// The mode of `view`'s Vim, or `None` while the mode is off.
pub(crate) fn mode(view: &EditorTextView) -> Option<Mode> {
    view.ivars().vim.borrow().vim.as_ref().map(Vim::mode)
}

/// Text from the input context: Vim's in Normal and Visual mode, else the editor's, which Vim follows for `.`;
/// whether Vim took it. Insert mode after `I` or `A` in Visual block mode types through Vim, which types in every line
/// of the block.
pub(crate) fn insert_text(view: &EditorTextView, text: &AnyObject) -> bool {
    let Some(mode) = mode(view) else { return false };
    let typed = if let Some(s) = text.downcast_ref::<NSString>() {
        s.to_string()
    } else if let Some(s) = text.downcast_ref::<objc2_foundation::NSAttributedString>() {
        s.string().to_string()
    } else {
        return false;
    };
    if mode == Mode::Insert && !in_block(view) {
        follow_typing(view, |vim| typed.chars().for_each(|c| drop(vim.char(c))));
        return false;
    }
    let latin = typed.is_ascii() || keyboard::types_ascii();
    run(view, |vim| {
        let mut outcome = Outcome::default();
        for c in typed.chars() {
            let one = vim.char(key_of(c, vim.mode(), latin));
            outcome.replacements.extend(one.replacements);
            outcome.effects.extend(one.effects);
        }
        outcome
    });
    true
}

/// Whether Vim types at several places at once, after `I` or `A` in Visual block mode.
fn in_block(view: &EditorTextView) -> bool {
    view.ivars()
        .vim
        .borrow()
        .vim
        .as_ref()
        .is_some_and(|vim| vim.mode() == Mode::Insert && vim.carets() > 1)
}

/// What the character `c`, typed in `mode` on a layout that types ASCII with `latin`, is to Vim: in Normal and Visual
/// mode on a layout that does not, the key at its place on the US layout (E105); else as typed, such as `π` on a German
/// layout or a pattern after `/`.
fn key_of(c: char, mode: Mode, latin: bool) -> char {
    match mode {
        Mode::Normal | Mode::Visual | Mode::VisualLine | Mode::VisualBlock if !latin => ascii_key(c),
        _ => c,
    }
}

/// A command of the input context, such as Esc or Return: Vim's outside Insert mode, and Esc there too; whether Vim
/// took it. Esc closes further carets first, which their own handler before this one does.
pub(crate) fn command(view: &EditorTextView, command: Sel) -> bool {
    let Some(mode) = mode(view) else { return false };
    let key = if command == sel!(cancelOperation:) {
        "<Esc>"
    } else if command == sel!(insertNewline:) {
        "<Enter>"
    } else if command == sel!(deleteBackward:) {
        "<BS>"
    } else if command == sel!(insertTab:) {
        "<Tab>"
    } else if command == sel!(moveLeft:) {
        "<Left>"
    } else if command == sel!(moveRight:) {
        "<Right>"
    } else if command == sel!(moveUp:) {
        "<Up>"
    } else if command == sel!(moveDown:) {
        "<Down>"
    } else {
        return mode != Mode::Insert;
    };
    if mode == Mode::Insert {
        if key == "<Esc>" {
            leave_insert(view);
        } else if in_block(view) {
            run(view, |vim| vim.keys(key));
        } else {
            follow_typing(view, |vim| drop(vim.keys(key)));
            return false;
        }
        return true;
    }
    run(view, |vim| vim.keys(key));
    true
}

/// The letters Vim takes with ⌃ in Normal and Visual mode (E105); the others stay the app's.
const NORMAL_CONTROL: &str = "rvodufbeyaxg";
/// The letters Vim takes with ⌃ in Insert mode.
const INSERT_CONTROL: &str = "orwu";

/// A key with ⌃ while the editor has the focus: Vim's in Normal and Visual mode, and ⌃O, ⌃R, ⌃W and ⌃U in Insert
/// mode, before the menu bar sees it (A6); ⌃[ and ⌃C are Esc. Whether Vim took it.
pub(crate) fn control_key(view: &EditorTextView, event: &NSEvent) -> bool {
    let Some(mode) = mode(view) else { return false };
    let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
    if !flags.contains(NSEventModifierFlags::Control) || flags.contains(NSEventModifierFlags::Command) {
        return false;
    }
    let focused = view.window().and_then(|w| w.firstResponder()).is_some_and(|r| {
        std::ptr::eq(
            objc2::rc::Retained::as_ptr(&r).cast::<u8>(),
            std::ptr::from_ref(view).cast::<u8>(),
        )
    });
    if !focused {
        return false;
    }
    let Some(c) = event
        .charactersIgnoringModifiers()
        .and_then(|s| s.to_string().chars().next())
        .map(|c| ascii_key(c).to_ascii_lowercase())
    else {
        return false;
    };
    if c == '[' || c == 'c' {
        if mode == Mode::Insert {
            leave_insert(view);
        } else {
            run(view, |vim| vim.keys("<Esc>"));
        }
        return true;
    }
    let taken = if mode == Mode::Insert {
        INSERT_CONTROL
    } else {
        NORMAL_CONTROL
    };
    if !taken.contains(c) {
        return false;
    }
    if mode == Mode::Insert {
        sync_copy(view);
    }
    let key = format!("<C-{}>", c.to_ascii_uppercase());
    run(view, |vim| vim.keys(&key));
    true
}

/// The editor's text changed by something other than Vim, such as the agent, the file or the user typing in Insert
/// mode: Vim's copy follows, but not keystroke by keystroke while it types along in Insert mode.
pub(crate) fn changed(view: &EditorTextView) {
    mark::clear(view);
    let (applying, inserting) = {
        let state = view.ivars().vim.borrow();
        (
            state.applying,
            state.vim.as_ref().is_some_and(|v| v.mode() == Mode::Insert),
        )
    };
    if applying || inserting {
        return;
    }
    if view.ivars().vim.borrow().vim.is_some() {
        sync_copy(view);
    }
}

/// The editor lost the focus or its document: an open insert pass ends as one undo step.
pub(crate) fn focus_lost(view: &EditorTextView) {
    close_group(view);
}

/// A change from outside, which `apply_model_changes` makes, never joins the undo step of an insert pass (E24).
pub(crate) fn before_model_change(view: &EditorTextView) {
    close_group(view);
}

fn follow_typing(view: &EditorTextView, f: impl FnOnce(&mut Vim)) {
    if let Some(vim) = view.ivars().vim.borrow_mut().vim.as_mut() {
        f(vim);
    }
}

/// Esc in Insert mode: the copy takes what the editor's own typing made, such as auto-closed brackets, then Vim
/// leaves Insert mode and the pass becomes one undo step.
fn leave_insert(view: &EditorTextView) {
    sync_copy(view);
    run(view, |vim| vim.keys("<Esc>"));
    close_group(view);
}

/// Puts the editor's text into Vim's copy, its caret with it.
fn sync_copy(view: &EditorTextView) {
    let text = view.document().text().rope().clone();
    let (line, column) = line_column(view, view.selectedRange().location);
    let mut state = view.ivars().vim.borrow_mut();
    state.crlf = term_vim::has_crlf(&text);
    if let Some(vim) = state.vim.as_mut() {
        let _ = vim.resync_rope(&text);
        vim.set_cursor(line, column);
    }
}

/// Gives Vim's `"+`, `"*` and, with `vimClipboard`, its unnamed register the system clipboard, when it changed.
fn load_clipboard(view: &EditorTextView) {
    let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
    let count = pasteboard.changeCount();
    let mut state = view.ivars().vim.borrow_mut();
    if state.clipboard == count {
        return;
    }
    state.clipboard = count;
    // SAFETY: AppKit's constant for plain text, a valid pasteboard type.
    let kind = unsafe { objc2_app_kit::NSPasteboardTypeString };
    let text = pasteboard
        .stringForType(kind)
        .map(|s| s.to_string())
        .unwrap_or_default();
    let shared = crate::host::settings().vim_clipboard;
    if let Some(vim) = state.vim.as_mut() {
        vim.share_clipboard(shared);
        vim.set_clipboard(&text);
    }
}

/// Runs `f` on `view`'s Vim and applies what it did to the editor.
fn run(view: &EditorTextView, f: impl FnOnce(&mut Vim) -> Outcome) {
    load_clipboard(view);
    let before = mode(view);
    let outcome = {
        let mut state = view.ivars().vim.borrow_mut();
        let Some(vim) = state.vim.as_mut() else { return };
        f(vim)
    };
    let after = mode(view);
    if before != Some(Mode::Insert) && after == Some(Mode::Insert) {
        open_group(view);
    }
    apply(view, &outcome);
    for effect in outcome.effects {
        perform(view, effect);
    }
    show(view);
}

/// Makes Vim's replacements in the editor's document, each an undo step unless an insert pass groups them.
fn apply(view: &EditorTextView, outcome: &Outcome) {
    if outcome.replacements.is_empty() {
        return;
    }
    let ending = if view.ivars().vim.borrow().crlf { "\r\n" } else { "\n" };
    view.ivars().vim.borrow_mut().applying = true;
    let _applying = OnDrop::new(|| view.ivars().vim.borrow_mut().applying = false);
    for replacement in &outcome.replacements {
        let start = utf16_of(view, replacement.start);
        let end = utf16_of(view, replacement.end);
        let text = replacement.text.replace('\n', ending);
        let change = view.document_mut().apply(Edit {
            range_utf16: start..end,
            text,
        });
        match change {
            Ok(change) => view.apply_vim_change(&change),
            Err(error) => term_base::diag::report("applying a Vim edit", error),
        }
    }
}

fn perform(view: &EditorTextView, effect: Effect) {
    match effect {
        Effect::Undo(count) | Effect::Redo(count) => {
            let undo = matches!(effect, Effect::Undo(_));
            if let Some(manager) = view.undoManager() {
                for _ in 0..count {
                    if undo { manager.undo() } else { manager.redo() }
                }
            }
            // Vim puts the cursor where the undone change began, AppKit after it.
            let text = view.document().text().rope().clone();
            let mut state = view.ivars().vim.borrow_mut();
            state.crlf = term_vim::has_crlf(&text);
            if let Some(vim) = state.vim.as_mut()
                && let Some((line, column)) = vim.resync_rope(&text)
            {
                vim.set_cursor(line, column);
            }
        }
        Effect::Save => crate::save_soon(view, false),
        Effect::Close => closing::soon(view, closing::Closing::Ask),
        Effect::Discard => closing::soon(view, closing::Closing::Discard),
        Effect::SaveAndClose(always) => closing::soon(view, closing::Closing::Save(always)),
        Effect::Note(note) => crate::status::note(view, &say(&note)),
        Effect::Copy(text) => {
            let pasteboard = objc2_app_kit::NSPasteboard::generalPasteboard();
            pasteboard.clearContents();
            // SAFETY: AppKit's constant for plain text, a valid pasteboard type.
            let kind = unsafe { objc2_app_kit::NSPasteboardTypeString };
            pasteboard.setString_forType(&crate::util::ns(&text), kind);
            view.ivars().vim.borrow_mut().clipboard = pasteboard.changeCount();
        }
        Effect::Highlight(found) => mark::show(view, found),
        Effect::Indent(lines, levels) => super::indent::shift_lines(view, lines, levels),
        Effect::Reindent(lines) => super::indent::reindent_lines(view, lines),
    }
}

/// What `note` says in the user's language: Vim's words, with Vim's error number where Vim has the message so that
/// `:help` finds it.
fn say(note: &Note) -> String {
    match note {
        Note::PatternNotFound(pattern) => tf("E486: Muster nicht gefunden: {}", &[pattern.as_str()]),
        Note::NoPreviousPattern => t("E35: Kein vorheriger regulärer Ausdruck").to_string(),
        Note::InvalidPattern { pattern, detail } => {
            term_base::diag::report("a Vim pattern", detail);
            tf("E383: Unzulässiges Suchmuster: {}", &[pattern.as_str()])
        }
        Note::UnsupportedCommand(command) => tf("Befehl nicht unterstützt: {}", &[command.as_str()]),
        Note::NoWordUnderCursor => t("E348: Keine Zeichenkette unter dem Cursor").to_string(),
        Note::NoSelection => t("Keine Auswahl").to_string(),
        Note::MarkNotSet => t("E20: Markierung nicht gesetzt").to_string(),
        Note::ReadOnly => t("E21: Kann keine Änderungen machen").to_string(),
        Note::NoPreviousMacro => t("E748: Kein bereits verwendetes Register").to_string(),
        Note::UnsupportedRegister => t("E354: Ungültiger Registername").to_string(),
        Note::InvalidDigraph(first, second) => tf("Unbekannter Digraph: {}", &[&format!("{first}{second}")]),
        Note::Other(detail) => {
            term_base::diag::report("a Vim command", detail);
            t("Das kann der Vim-Modus nicht").to_string()
        }
    }
}

/// Shows Vim's caret, selection and mode in the editor and its status bar, and says a new mode for VoiceOver.
fn show(view: &EditorTextView) {
    let (mode, cursor, selection, prompt) = {
        let mut state = view.ivars().vim.borrow_mut();
        match state.vim.as_mut() {
            Some(vim) => (Some(vim.mode()), Some(vim.cursor()), vim.selection(), vim.prompt()),
            None => (None, None, None, None),
        }
    };
    let insert = matches!(mode, None | Some(Mode::Insert));
    let color = if insert {
        NSColor::textColor()
    } else {
        NSColor::clearColor()
    };
    view.setInsertionPointColor(Some(&color));
    if mode.is_some() {
        let range = match (selection, cursor) {
            (Some((first, last)), _) => {
                let start = utf16_of(view, first);
                let end = utf16_of(view, (last.0, last.1 + 1));
                NSRange::new(start, end.saturating_sub(start))
            }
            (None, Some(at)) => NSRange::new(utf16_of(view, at), 0),
            (None, None) => view.selectedRange(),
        };
        view.setSelectedRange(range);
        view.scrollRangeToVisible(NSRange::new(range.location, 0));
    }
    let text = match (mode, prompt) {
        (_, Some(prompt)) => prompt,
        (Some(mode), None) => mode_name(mode).to_string(),
        (None, None) => String::new(),
    };
    let label = view.ivars().vim.borrow().label.clone();
    if let Some(label) = label {
        label.setStringValue(&crate::util::ns(&text));
        label.setHidden(mode.is_none());
    }
    let changed = view.ivars().vim.borrow().shown != mode;
    view.ivars().vim.borrow_mut().shown = mode;
    if changed && let Some(mode) = mode {
        term_ui::kit::panel::announce(view.mtm(), mode_name(mode));
    }
    super::overlay::redraw(view);
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Normal => t("NORMAL"),
        Mode::Insert => t("EINFÜGEN"),
        Mode::Replace => t("ERSETZEN"),
        Mode::Visual => t("VISUELL"),
        Mode::VisualLine => t("VISUELL ZEILE"),
        Mode::VisualBlock => t("VISUELL BLOCK"),
        Mode::Prompt => "",
    }
}

/// Adds the mode's label to the editor's status bar `stack`, hidden while the Vim mode is off.
pub(crate) fn status_items(mtm: MainThreadMarker, view: &EditorTextView, stack: &objc2_app_kit::NSStackView) {
    let label = crate::util::label(mtm, "");
    let weight = crate::util::constant!(objc2_app_kit::NSFontWeightSemibold);
    label.setFont(Some(&objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(
        term_ui::kit::style::SMALL,
        weight,
    )));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label.setHidden(true);
    stack.addView_inGravity(&label, objc2_app_kit::NSStackViewGravity::Leading);
    view.ivars().vim.borrow_mut().label = Some(label);
}

/// Draws the block caret outside Insert mode, over the character under the caret.
pub(crate) fn draw_block(view: &EditorTextView) -> Option<objc2_foundation::NSRect> {
    let rect = block(view)?;
    NSColor::textColor().colorWithAlphaComponent(0.45).setFill();
    objc2_app_kit::NSBezierPath::fillRect(rect);
    Some(rect)
}

/// Where the block caret of `view` goes, in the editor's coordinates.
pub(crate) fn block(view: &EditorTextView) -> Option<objc2_foundation::NSRect> {
    if matches!(mode(view), None | Some(Mode::Insert)) {
        return None;
    }
    let at = view.selectedRange();
    if at.length > 0 {
        return None;
    }
    let length = view.document().text().len_utf16();
    let rect = if at.location < length {
        crate::text_view::rect_of(view, at.location..at.location + 1)
    } else {
        crate::text_view::rect_of(view, at.location..at.location)
    };
    let mut rect = rect?;
    if rect.size.width < 2.0 {
        rect.size.width = view.ivars().font_size.get() * 0.6;
    }
    Some(rect)
}

/// Opens the undo group of an insert pass, which closes on Esc, a lost focus, another document or a change from
/// outside (E24).
fn open_group(view: &EditorTextView) {
    if view.ivars().vim.borrow().grouping {
        return;
    }
    if let Some(manager) = view.undoManager() {
        manager.setGroupsByEvent(false);
        manager.beginUndoGrouping();
        view.ivars().vim.borrow_mut().grouping = true;
    }
}

fn close_group(view: &EditorTextView) {
    if !view.ivars().vim.borrow().grouping {
        return;
    }
    view.ivars().vim.borrow_mut().grouping = false;
    if let Some(manager) = view.undoManager() {
        if manager.groupingLevel() > 0 {
            manager.endUndoGrouping();
        }
        manager.setGroupsByEvent(true);
    }
}

/// The line and column in characters of UTF-16 offset `at` of `view`'s text.
fn line_column(view: &EditorTextView, at: usize) -> (usize, usize) {
    let document = view.document();
    let text = document.text();
    let byte = text.utf16_to_byte(at.min(text.len_utf16())).unwrap_or(0);
    text.point(byte).map_or((0, 0), |point| (point.line, point.utf32))
}

/// The UTF-16 offset of `(line, column)` in characters of `view`'s text, kept within the line.
fn utf16_of(view: &EditorTextView, (line, column): (usize, usize)) -> usize {
    let document = view.document();
    let text = document.text();
    let line = line.min(text.line_count().saturating_sub(1));
    let byte = text
        .offset(line, column, Unit::Utf32)
        .or_else(|_| text.offset(line, 0, Unit::Utf32))
        .unwrap_or(0);
    text.byte_to_utf16(byte).unwrap_or(0)
}

/// The key of the US layout at the place of `c` where a keyboard's layout is not Latin, so that `j` is `j` on a
/// Russian, Greek or Hebrew keyboard too; an ASCII character stays what it is, whichever layout typed it.
fn ascii_key(c: char) -> char {
    const RUSSIAN: &str = "йцукенгшщзхъфывапролджэячсмитьбю";
    const US: &str = "qwertyuiop[]asdfghjkl;'zxcvbnm,.";
    const GREEK: &str = ";ςερτυθιοπ[]ασδφγηξκλ΄'ζχψωβνμ,.";
    const HEBREW: &str = "/'קראטוןםפ][שדגכעיחלךף,זסבהנמצתץ";
    if c.is_ascii() {
        return c;
    }
    let lower: Vec<char> = c.to_lowercase().collect();
    let Some(&lower) = lower.first() else { return c };
    for layout in [RUSSIAN, GREEK, HEBREW] {
        if let Some(index) = layout.chars().position(|k| k == lower) {
            let mapped = US.chars().nth(index).unwrap_or(c);
            return if c.is_uppercase() {
                mapped.to_ascii_uppercase()
            } else {
                mapped
            };
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::{Mode, ascii_key, key_of};

    #[test]
    fn a_non_latin_layout_reads_as_the_us_keys_at_the_same_place() {
        assert_eq!(ascii_key('о'), 'j');
        assert_eq!(ascii_key('л'), 'k');
        assert_eq!(ascii_key('Ш'), 'I');
        assert_eq!(ascii_key('ξ'), 'j');
        assert_eq!(ascii_key('ח'), 'j');
        assert_eq!(ascii_key('j'), 'j');
        assert_eq!(ascii_key('^'), '^');
    }

    #[test]
    fn a_layout_that_types_ascii_hands_every_character_to_vim_as_typed() {
        for mode in [
            Mode::Normal,
            Mode::Visual,
            Mode::VisualBlock,
            Mode::Prompt,
            Mode::Insert,
        ] {
            for c in "/;',.[]:%?nN*\"`πüçξо".chars() {
                assert_eq!(key_of(c, mode, true), c, "{c} in {mode:?}");
            }
        }
    }

    #[test]
    fn a_layout_without_ascii_maps_keys_only_outside_the_prompt_and_insert_mode() {
        for c in "/;',.[]:%?nN*\"`".chars() {
            assert_eq!(key_of(c, Mode::Normal, false), c, "{c}");
        }
        assert_eq!(key_of('о', Mode::Normal, false), 'j');
        assert_eq!(key_of('ξ', Mode::VisualBlock, false), 'j');
        assert_eq!(key_of('ח', Mode::Visual, false), 'j');
        assert_eq!(key_of('о', Mode::Prompt, false), 'о');
        assert_eq!(key_of('ח', Mode::Prompt, false), 'ח');
        assert_eq!(key_of('ξ', Mode::Insert, false), 'ξ');
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
