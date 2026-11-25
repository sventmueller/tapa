//! Vim's modes, keys, motions and operators (E104–E106) without AppKit: the only crate that knows `modalkit`. A
//! [`Vim`] holds a shadow copy of an editor's text with LF line ends only, feeds keys to `modalkit`'s Vim machine,
//! lets its `EditBuffer` carry out operators, registers, marks, visual mode and macros on the copy, and hands back what
//! changed as replacements by line and column in characters, which the editor applies to its own text. Undo and redo
//! go to the editor's own history, and `/`, `?`, `n` and `N` search with Rust's regular expressions.
//!
//! ```
//! use term_vim::{Mode, Vim};
//!
//! let mut vim = Vim::new("one two\nthree\n");
//! vim.keys("w");
//! assert_eq!(vim.cursor(), (0, 4));
//! let done = vim.keys("dd");
//! assert_eq!(vim.text(), "three\n");
//! assert_eq!(done.replacements.len(), 1);
//! vim.keys("i");
//! assert_eq!(vim.mode(), Mode::Insert);
//! ```

#![forbid(unsafe_code)]

mod diff;
mod note;
mod restore;
mod search;

use std::str::FromStr;

use modalkit::actions::{Action, CommandBarAction, EditAction, Editable, EditorAction, HistoryAction, Jumpable};
use modalkit::editing::application::EmptyInfo;
use modalkit::editing::buffer::{CursorGroupId, EditBuffer};
use modalkit::editing::context::{EditContext, Resolve};
use modalkit::editing::cursor::{Cursor, CursorState};
use modalkit::editing::key::KeyManager;
use modalkit::editing::rope::EditRope;
use modalkit::editing::store::Store;
use modalkit::env::vim::keybindings::default_vim_keys;
use modalkit::key::TerminalKey;
use modalkit::keybindings::BindingMachine;
use modalkit::prelude::{
    EditTarget, IndentChange, MoveDir1D, MoveDirMod, RepeatType, SearchType, Specifier, ViewportContext,
};

pub use note::Note;
use search::{Found, Search, Step};

/// The mode the keys are in, as the status bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Keys move and run operators.
    Normal,
    /// Keys type text.
    Insert,
    /// Keys replace the characters under the cursor.
    Replace,
    /// A stretch of characters is selected.
    Visual,
    /// Whole lines are selected.
    VisualLine,
    /// A block of columns is selected.
    VisualBlock,
    /// A search pattern or an Ex command is being typed after `/`, `?` or `:`.
    Prompt,
}

/// A change of the text: the characters from `start` to `end`, each a line and a column in characters counted from 0,
/// make way for `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Replacement {
    /// Where the replaced characters begin.
    pub start: (usize, usize),
    /// Where they end, not included.
    pub end: (usize, usize),
    /// What takes their place.
    pub text: String,
}

/// What the editor does for Vim beyond replacing text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Undo this many steps of the editor's history.
    Undo(usize),
    /// Redo this many steps.
    Redo(usize),
    /// Save the file, as `:w` asks.
    Save,
    /// Close the editor's document and ask about unsaved changes as ⌘W does, as `:q` asks.
    Close,
    /// Close the editor's document and give its unsaved changes up, as `:q!` asks.
    Discard,
    /// Save the file and close the document once it is written, as `:wq` asks; with `false` only when it has unsaved
    /// changes, and else close it as it is, as `:x` asks.
    SaveAndClose(bool),
    /// Say this in the status bar, such as a command Vim does not support here.
    Note(Note),
    /// Mark the match from the first place to the second, not included, each a line and a column in characters, or
    /// none after `:noh`.
    Highlight(Option<((usize, usize), (usize, usize))>),
    /// Put this text on the system clipboard, which a command wrote into `"+` or `"*`, or into the unnamed register
    /// while it is the clipboard.
    Copy(String),
    /// Indent these lines, counted from 0, by this many levels, or take as many away when negative, with the
    /// editor's own rule for a level; `modalkit` leaves indenting to its user.
    Indent(std::ops::Range<usize>, isize),
    /// Indent these lines anew by the editor's rule, as `=` asks.
    Reindent(std::ops::Range<usize>),
}

/// What a key or a run of keys did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// The changes of the text, in the order they apply.
    pub replacements: Vec<Replacement>,
    /// What else the editor does.
    pub effects: Vec<Effect>,
}

/// Vim's state for one editor.
pub struct Vim {
    keys: KeyManager<TerminalKey, Action<EmptyInfo>, RepeatType>,
    buffer: EditBuffer<EmptyInfo>,
    store: Store<EmptyInfo>,
    group: CursorGroupId,
    viewport: ViewportContext<Cursor>,
    search: Search,
    /// Whether the unnamed register is the system clipboard too (`vimClipboard`).
    shared_clipboard: bool,
    /// The copy ends with a line break that the editor's text lacks: `modalkit` needs one there and reads it as the
    /// end of the last line, as Vim does (E104).
    added_break: bool,
    /// The keys the machine took since it last acted, such as `d` or `"a` while their command waits for more.
    pending: Vec<String>,
    /// The selection Visual mode had last and the mode, for `gv`.
    last_selection: Option<(CursorState, Mode)>,
    /// The line the latest changes went to and its text before the first of them, for `U`.
    line_undo: Option<(usize, String)>,
}

impl Vim {
    /// Vim on a copy of `text`, in Normal mode at its start.
    pub fn new(text: &str) -> Vim {
        let (copy, added_break) = shadow(text);
        let mut buffer = EditBuffer::from_str(String::from("editor"), &copy);
        let group = buffer.create_group();
        Vim {
            keys: KeyManager::new(default_vim_keys::<EmptyInfo>()),
            buffer,
            store: Store::default(),
            group,
            viewport: ViewportContext::default(),
            search: Search::default(),
            shared_clipboard: false,
            added_break,
            pending: Vec::new(),
            last_selection: None,
            line_undo: None,
        }
    }

    /// Makes the unnamed register the system clipboard too, or Vim's own again (E106).
    pub fn share_clipboard(&mut self, shared: bool) {
        self.shared_clipboard = shared;
    }

    /// Takes the system clipboard's `text` into `"+` and `"*`, and into the unnamed register while it is shared;
    /// the editor calls it before a key that may paste. Text that ends with a line break pastes as whole lines.
    pub fn set_clipboard(&mut self, text: &str) {
        let text = text.replace("\r\n", "\n");
        self.store.registers.set_system_text(&text);
        if self.shared_clipboard && self.store.registers.unnamed_text() != text {
            self.store.registers.set_unnamed_text(&text);
        }
    }

    /// Takes `text` as the editor's text again after a change Vim did not make, such as the agent's or the file's,
    /// keeping the cursor where it was as far as the text reaches; where the first change begins, if any.
    pub fn resync(&mut self, text: &str) -> Option<(usize, usize)> {
        let (copy, added_break) = shadow(text);
        self.take_copy(copy.into(), added_break)
    }

    /// [`Vim::resync`] from the editor's rope, whose leaves the copy shares unless the text holds a CRLF.
    pub fn resync_rope(&mut self, text: &ropey::Rope) -> Option<(usize, usize)> {
        let (copy, added_break) = shadow_rope(text);
        self.take_copy(copy.into(), added_break)
    }

    fn take_copy(&mut self, copy: EditRope, added_break: bool) -> Option<(usize, usize)> {
        let cursor = self.cursor();
        let before = visible(self.buffer.get().as_rope(), self.added_break);
        self.added_break = added_break;
        self.buffer.set_text(copy);
        self.set_cursor(cursor.0, cursor.1);
        let after = visible(self.buffer.get().as_rope(), self.added_break);
        let change = diff::difference(&before, &after)?;
        self.note_line_change(&before, &change);
        Some(change.start)
    }

    /// The editor's text as Vim's copy has it, with LF line ends.
    pub fn text(&self) -> String {
        visible(self.buffer.get().as_rope(), self.added_break).to_string()
    }

    /// The cursor as a line and a column in characters, counted from 0.
    pub fn cursor(&mut self) -> (usize, usize) {
        let cursor = self.buffer.get_leader(self.group);
        (cursor.y, cursor.x)
    }

    /// Puts the cursor at `line` and `column`, as a click or the editor's own move does.
    pub fn set_cursor(&mut self, line: usize, column: usize) {
        let lines = self.buffer.get_lines().max(1);
        let line = line.min(lines - 1);
        let column = column.min(self.buffer.get_columns(line));
        self.buffer.set_leader(self.group, Cursor::new(line, column));
    }

    /// How many places typing goes to: more than one while Insert mode goes on from `I` or `A` in Visual block mode,
    /// which the editor's own typing would put in one line only.
    pub fn carets(&self) -> usize {
        1 + self.buffer.get_followers(self.group).len()
    }

    /// The mode the keys are in.
    pub fn mode(&self) -> Mode {
        if self.search.is_open() {
            return Mode::Prompt;
        }
        mode_of(self.keys.show_mode().as_deref())
    }

    /// Feeds the character `c` as typed, `<` and spaces included, which [`Vim::keys`] would read as key names.
    pub fn char(&mut self, c: char) -> Outcome {
        use modalkit::key::event::{KeyCode, KeyEvent, KeyModifiers};
        self.key(TerminalKey::from(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
    }

    /// The selection of Visual mode as its first and last character, each a line and a column; `None` outside it.
    pub fn selection(&mut self) -> Option<((usize, usize), (usize, usize))> {
        if !matches!(self.mode(), Mode::Visual | Mode::VisualLine | Mode::VisualBlock) {
            return None;
        }
        let (a, b, _) = self.buffer.get_leader_selection(self.group)?;
        let (first, last) = if (a.y, a.x) <= (b.y, b.x) { (a, b) } else { (b, a) };
        Some(((first.y, first.x), (last.y, last.x)))
    }

    /// What the prompt after `/`, `?` or `:` shows while it is open.
    pub fn prompt(&self) -> Option<String> {
        self.search.shown()
    }

    /// Feeds the keys written as Vim writes them, such as `dd`, `ciw<Esc>` or `<C-R>`, and says what they did.
    pub fn keys(&mut self, keys: &str) -> Outcome {
        let mut outcome = Outcome::default();
        for key in parse(keys) {
            let one = self.key(key);
            outcome.replacements.extend(one.replacements);
            outcome.effects.extend(one.effects);
        }
        outcome
    }

    /// Feeds one key.
    pub(crate) fn key(&mut self, key: TerminalKey) -> Outcome {
        let name = key.to_string();
        let normal = self.mode() == Mode::Normal;
        let idle = normal && self.pending.is_empty();
        // A clone of the rope shares its nodes; it becomes text only when an action may have changed it.
        let (before, before_break) = (self.buffer.get().clone(), self.added_break);
        let unnamed = self.store.registers.unnamed_text();
        let mut effects = Vec::new();
        let mut undo_line = false;
        let changes = if self.search.is_open() {
            self.prompt_key(&key, &mut effects)
        } else if idle && matches!(name.as_str(), "/" | "?" | ":") {
            // Opened here rather than by the machine, which would record the key in a macro without the pattern.
            self.search.open(name.chars().next().unwrap_or(':'));
            false
        } else if idle && name == "U" {
            undo_line = true;
            self.undo_line()
        } else if normal && self.is_reselect(&name) {
            self.reselect(&mut effects);
            false
        } else {
            self.feed(key, &mut effects)
        };
        if let Some(copied) = self.store.registers.take_system_written() {
            effects.push(Effect::Copy(copied));
        } else if self.shared_clipboard {
            let now = self.store.registers.unnamed_text();
            if now != unnamed {
                effects.push(Effect::Copy(now));
            }
        }
        let replacement = if changes {
            self.changed_since(&before, before_break, !undo_line)
        } else {
            None
        };
        if normal && self.mode() == Mode::Normal {
            self.settle_cursor(&name, replacement.as_ref());
        }
        self.remember_selection();
        Outcome {
            replacements: replacement.into_iter().collect(),
            effects,
        }
    }

    /// Feeds `key` to `modalkit`'s machine and carries out what it does; whether that may have changed the text.
    fn feed(&mut self, key: TerminalKey, effects: &mut Vec<Effect>) -> bool {
        let name = key.to_string();
        self.pending.push(name.clone());
        self.keys.input_key(key);
        let (mut changes, mut acted) = (false, false);
        while let Some((action, context)) = self.keys.pop() {
            acted = true;
            changes |= may_change_text(&action, &context);
            effects.extend(self.act(&action, &context));
        }
        if acted || name == "<Esc>" {
            self.pending.clear();
        }
        changes
    }

    /// The replacement that turns the editor's text of `before`, a state of the copy that ended in a line break of
    /// its own with `before_break`, into the one of the copy now; with `line`, `U` takes note of it.
    fn changed_since(&mut self, before: &EditRope, before_break: bool, line: bool) -> Option<Replacement> {
        let old = visible(before.as_rope(), before_break);
        let new = visible(self.buffer.get().as_rope(), self.added_break);
        let copy = self.buffer.get().as_rope();
        if copy.len_chars() == 0 || copy.char(copy.len_chars() - 1) != '\n' {
            // `modalkit` reads its text as ending in a line break; one it adds is not the editor's.
            self.buffer.append_text("");
            self.added_break = true;
        }
        let replacement = diff::difference(&old, &new)?;
        if line {
            self.note_line_change(&old, &replacement);
        }
        Some(replacement)
    }

    /// Puts the cursor where Vim leaves it after `key` in Normal mode, where `modalkit` leaves it elsewhere: one
    /// further after `~`, on the last character typed after `.` repeated an insertion.
    fn settle_cursor(&mut self, key: &str, replacement: Option<&Replacement>) {
        let (line, column) = self.cursor();
        let last = self.buffer.get_columns(line).saturating_sub(1);
        match (key, replacement) {
            ("~", Some(_)) => self.set_cursor(line, (column + 1).min(last)),
            (".", Some(replaced)) if !replaced.text.is_empty() && !replaced.text.contains('\n') => {
                let end = replaced.start.1 + replaced.text.chars().count();
                self.set_cursor(replaced.start.0, end.saturating_sub(1));
            }
            _ => {}
        }
    }

    /// Carries out one action of the machine on the copy, or says what the editor does for it.
    fn act(&mut self, action: &Action<EmptyInfo>, context: &EditContext) -> Vec<Effect> {
        let ctx = (self.group, &self.viewport, context);
        match action {
            Action::NoOp => Vec::new(),
            Action::Editor(EditorAction::History(HistoryAction::Undo(count))) => {
                vec![Effect::Undo(context.resolve(count))]
            }
            Action::Editor(EditorAction::History(HistoryAction::Redo(count))) => {
                vec![Effect::Redo(context.resolve(count))]
            }
            // Undo is the editor's (E106); `modalkit`'s own history keeps a copy of the text per checkpoint, which
            // costs half a second in a 10 MB file.
            Action::Editor(EditorAction::History(HistoryAction::Checkpoint)) => Vec::new(),
            Action::Editor(EditorAction::Edit(edit, target))
                if matches!(context.resolve(edit), EditAction::Indent(_)) =>
            {
                self.indent(context.resolve(edit), target, context)
            }
            Action::CommandBar(_) | Action::Search(..) => self.search_action(action, context),
            Action::Editor(EditorAction::Edit(_, EditTarget::Search(kind, ..)))
                if !matches!(kind, SearchType::Char(_)) =>
            {
                self.search_action(action, context)
            }
            Action::Editor(editor) => match self.buffer.editor_command(editor, &ctx, &mut self.store) {
                Ok(_) => Vec::new(),
                Err(error) => vec![Effect::Note(Note::of_edit(&error))],
            },
            Action::Macro(act) => match self.keys.macro_command(act, context, &mut self.store) {
                Ok(_) => Vec::new(),
                Err(error) => vec![Effect::Note(Note::of_edit(&error))],
            },
            Action::Repeat(kind) => {
                self.keys.repeat(kind.clone(), Some(context.clone()));
                Vec::new()
            }
            Action::Jump(list, dir, count) => match self.buffer.jump(*list, *dir, context.resolve(count), &ctx) {
                Ok(_) => Vec::new(),
                Err(error) => vec![Effect::Note(Note::of_ui(&error))],
            },
            _ => Vec::new(),
        }
    }

    /// The prompt after `/`, `?` and `:`, and `n`, `N`, `*` and `#`, which search with Rust's regular expressions
    /// here; after an operator, such as `dn`, they are not supported.
    fn search_action(&mut self, action: &Action<EmptyInfo>, context: &EditContext) -> Vec<Effect> {
        let operation: EditAction = context.resolve(&Specifier::Contextual);
        let motion = matches!(operation, EditAction::Motion);
        match action {
            Action::CommandBar(CommandBarAction::Focus(prompt, ..)) => {
                // The prompt is this crate's own; the machine would wait in its command mode.
                self.keys.reset_mode();
                if !motion {
                    return vec![self.unsupported()];
                }
                self.search.open(prompt.chars().next().unwrap_or(':'));
                Vec::new()
            }
            Action::Search(flip, count) => self.search_again(flip, context.resolve(count)),
            Action::Editor(EditorAction::Edit(_, EditTarget::Search(kind, flip, count))) => {
                if !motion {
                    return vec![self.unsupported()];
                }
                let count = context.resolve(count);
                let SearchType::Word(_, whole) = kind else {
                    return self.search_again(flip, count);
                };
                let (cursor, text) = (self.cursor(), self.buffer.get_text());
                let forward = context.get_search_regex_dir() == MoveDir1D::Next;
                let found = self.search.word(forward, *whole, count, &text, cursor);
                self.jump(found)
            }
            _ => Vec::new(),
        }
    }

    /// `n` or `N` `count` times.
    fn search_again(&mut self, flip: &MoveDirMod, count: usize) -> Vec<Effect> {
        let (cursor, text) = (self.cursor(), self.buffer.get_text());
        let found = self
            .search
            .again(matches!(flip, MoveDirMod::Flip), count, &text, cursor);
        self.jump(found)
    }

    /// Moves the cursor to the match a search `found` and marks it, or says why there is none.
    fn jump(&mut self, found: Result<Found, Note>) -> Vec<Effect> {
        match found {
            Ok(found) => {
                self.set_cursor(found.start.0, found.start.1);
                vec![Effect::Highlight(Some((found.start, found.end)))]
            }
            Err(note) => vec![Effect::Note(note)],
        }
    }

    /// The keys of a command the Vim mode does not support, as typed.
    fn unsupported(&self) -> Effect {
        Effect::Note(Note::UnsupportedCommand(self.pending.concat()))
    }

    /// A key typed into the prompt; whether an Ex command it ran may have changed the text.
    fn prompt_key(&mut self, key: &TerminalKey, effects: &mut Vec<Effect>) -> bool {
        let cursor = self.cursor();
        let buffer = &self.buffer;
        match self.search.key(key, || buffer.get_text(), cursor) {
            Step::Typing => false,
            Step::Jump(found) => {
                effects.extend(self.jump(Ok(found)));
                false
            }
            Step::Command(command) => {
                effects.extend(self.ex(&command));
                true
            }
            Step::Failed(note) => {
                effects.push(Effect::Note(note));
                false
            }
        }
    }

    /// `>`, `<` and `=` with `target`: the lines they reach go to the editor, which knows its indentation.
    fn indent(&mut self, edit: EditAction, target: &EditTarget, context: &EditContext) -> Vec<Effect> {
        let EditAction::Indent(change) = edit else {
            return Vec::new();
        };
        let lines = self.indented_lines(target, context);
        match change {
            IndentChange::Increase(count) => vec![Effect::Indent(lines, context.resolve(&count).cast_signed())],
            IndentChange::Decrease(count) => vec![Effect::Indent(lines, -context.resolve(&count).cast_signed())],
            IndentChange::Auto => vec![Effect::Reindent(lines)],
        }
    }

    /// The lines an indent with `target` reaches: the selection's in Visual mode, the lines a motion passes over, or
    /// the count of lines from the cursor's.
    fn indented_lines(&mut self, target: &EditTarget, context: &EditContext) -> std::ops::Range<usize> {
        let line = self.cursor().0;
        if let Some(selection) = self.buffer.get_leader_selection(self.group) {
            let (a, b, _) = selection;
            let (first, last) = (a.y.min(b.y), a.y.max(b.y));
            self.buffer.set_leader(self.group, Cursor::new(first, 0));
            return first..last + 1;
        }
        let reached = match target {
            EditTarget::Motion(..) => {
                let cursor = self.buffer.get_leader(self.group);
                let motion = EditorAction::Edit(Specifier::Exact(EditAction::Motion), target.clone());
                let ctx = (self.group, &self.viewport, context);
                let moved = self.buffer.editor_command(&motion, &ctx, &mut self.store).is_ok();
                let reached = if moved { self.cursor().0 } else { line };
                self.buffer.set_leader(self.group, cursor);
                reached
            }
            EditTarget::Range(_, _, count) => line + context.resolve(count).saturating_sub(1),
            _ => line,
        };
        let last = reached.max(line).min(self.buffer.get_lines().saturating_sub(1));
        let first = reached.min(line);
        self.buffer.set_leader(self.group, Cursor::new(first, 0));
        first..last + 1
    }

    /// Runs the Ex command `command`, typed after `:`.
    fn ex(&mut self, command: &str) -> Vec<Effect> {
        let command = command.trim();
        match command {
            "w" => vec![Effect::Save],
            "q" => vec![Effect::Close],
            "q!" => vec![Effect::Discard],
            "wq" => vec![Effect::SaveAndClose(true)],
            "x" => vec![Effect::SaveAndClose(false)],
            "noh" | "nohlsearch" => vec![Effect::Highlight(None)],
            line if line.parse::<usize>().is_ok() => {
                let line = line.parse::<usize>().unwrap_or(1).saturating_sub(1);
                self.set_cursor(line, 0);
                Vec::new()
            }
            _ => match search::substitute(command, &self.text(), self.cursor().0) {
                Some(Ok(text)) => {
                    let cursor = self.cursor();
                    let (copy, added_break) = shadow(&text);
                    self.added_break = added_break;
                    self.buffer.set_text(copy);
                    self.set_cursor(cursor.0, 0);
                    Vec::new()
                }
                Some(Err(note)) => vec![Effect::Note(note)],
                None => vec![Effect::Note(Note::UnsupportedCommand(command.to_string()))],
            },
        }
    }
}

/// Whether `action` can change the text, so that only those compare the text before and after.
fn may_change_text(action: &Action<EmptyInfo>, context: &EditContext) -> bool {
    match action {
        Action::Editor(EditorAction::Edit(edit, _)) => {
            !matches!(context.resolve(edit), EditAction::Motion | EditAction::Yank(..))
        }
        Action::Editor(EditorAction::InsertText(_) | EditorAction::Selection(_) | EditorAction::Complete(..)) => true,
        Action::Macro(_) | Action::Repeat(_) => true,
        _ => false,
    }
}

/// `text` with LF line ends only, which the copy keeps so that `$` and `x` never reach a CR (E104), and with a line
/// break at its end, which `modalkit` needs; whether that break was added.
fn shadow(text: &str) -> (String, bool) {
    let mut copy = text.replace("\r\n", "\n");
    let added = !copy.ends_with('\n');
    if added {
        copy.push('\n');
    }
    (copy, added)
}

/// Whether `text` holds a CRLF, which the copy keeps as an LF.
pub fn has_crlf(text: &ropey::Rope) -> bool {
    let mut after_cr = false;
    text.chunks().any(|chunk| {
        let found = (after_cr && chunk.starts_with('\n')) || chunk.contains("\r\n");
        after_cr = chunk.ends_with('\r');
        found
    })
}

/// [`shadow`] of a rope: a clone of it, which takes constant time, unless it holds a CRLF.
fn shadow_rope(text: &ropey::Rope) -> (ropey::Rope, bool) {
    if has_crlf(text) {
        let (copy, added) = shadow(&text.to_string());
        return (ropey::Rope::from_str(&copy), added);
    }
    let mut copy = text.clone();
    let len = copy.len_chars();
    let added = len == 0 || copy.char(len - 1) != '\n';
    if added {
        copy.insert_char(len, '\n');
    }
    (copy, added)
}

/// The editor's text of `rope`, a state of the copy: without the line break at its end when the copy `added` it.
fn visible(rope: &ropey::Rope, added: bool) -> ropey::Rope {
    let mut rope = rope.clone();
    let len = rope.len_chars();
    if added && len > 0 && rope.char(len - 1) == '\n' {
        rope.remove(len - 1..);
    }
    rope
}

/// The keys of `keys`, written as Vim writes them: single characters and names in angle brackets such as `<Esc>`.
pub(crate) fn parse(keys: &str) -> Vec<TerminalKey> {
    let mut out = Vec::new();
    let mut rest = keys;
    while let Some(first) = rest.chars().next() {
        if first == '<'
            && let Some(end) = rest.find('>')
            && let Ok(key) = TerminalKey::from_str(&rest[..=end])
        {
            out.push(key);
            rest = &rest[end + 1..];
            continue;
        }
        if let Ok(key) = TerminalKey::from_str(&first.to_string()) {
            out.push(key);
        }
        rest = &rest[first.len_utf8()..];
    }
    out
}

/// The mode a `show_mode` text such as `-- INSERT --` names.
fn mode_of(shown: Option<&str>) -> Mode {
    let shown = shown.unwrap_or_default();
    if shown.contains("VISUAL LINE") {
        Mode::VisualLine
    } else if shown.contains("VISUAL BLOCK") {
        Mode::VisualBlock
    } else if shown.contains("VISUAL") || shown.contains("SELECT") {
        Mode::Visual
    } else if shown.contains("REPLACE") {
        Mode::Replace
    } else if shown.contains("INSERT") {
        Mode::Insert
    } else {
        Mode::Normal
    }
}

#[cfg(test)]
mod editor_tests;
#[cfg(test)]
mod property_tests;
#[cfg(test)]
mod tests;
