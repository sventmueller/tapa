//! Vim as the editor drives it: its replacements go into the editor's own text one undo step per key, `u` takes the
//! editor's history back and Vim follows, and the system clipboard comes in before a key when it changed.

use super::tests::apply;
use super::*;

/// The editor around a Vim, as `src/editor/vim.rs` drives it.
struct Editor {
    vim: Vim,
    text: String,
    undo: Vec<String>,
    clipboard: String,
    /// How often the clipboard changed, and the change Vim took last.
    changes: usize,
    taken: usize,
}

impl Editor {
    fn new(text: &str, shared: bool) -> Editor {
        let mut vim = Vim::new(text);
        vim.share_clipboard(shared);
        Editor {
            vim,
            text: text.to_string(),
            undo: Vec::new(),
            clipboard: String::new(),
            changes: 0,
            taken: 0,
        }
    }

    /// Something else puts `text` on the clipboard.
    fn copy(&mut self, text: &str) {
        self.clipboard = text.to_string();
        self.changes += 1;
    }

    /// Types `keys` and says what else the editor did for them.
    fn keys(&mut self, keys: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        for key in parse(keys) {
            if self.taken != self.changes {
                self.vim.set_clipboard(&self.clipboard);
                self.taken = self.changes;
            }
            let done = self.vim.key(key);
            if !done.replacements.is_empty() {
                self.undo.push(self.text.clone());
                self.text = apply(&self.text, false, &done.replacements).unwrap_or_else(|wrong| panic!("{wrong}"));
            }
            for effect in done.effects {
                match &effect {
                    Effect::Undo(count) => {
                        for _ in 0..*count {
                            self.text = self.undo.pop().unwrap_or_else(|| self.text.clone());
                        }
                        if let Some((line, column)) = self.vim.resync(&self.text) {
                            self.vim.set_cursor(line, column);
                        }
                    }
                    Effect::Copy(text) => {
                        self.copy(text);
                        self.taken = self.changes;
                    }
                    _ => {}
                }
                effects.push(effect);
            }
            assert_eq!(
                self.vim.text(),
                self.text,
                "Vim's copy and the editor's text part after {keys:?}"
            );
        }
        effects
    }
}

#[test]
fn a_substitution_reaches_the_editor_as_one_undo_step_and_the_copy_stays_in_step() {
    let mut editor = Editor::new("x x\ny x", false);
    editor.keys(":s#x#z#<Enter>");
    assert_eq!(editor.text, "z x\ny x");
    editor.keys("x");
    assert_eq!(editor.text, " x\ny x", "x deletes the character under Vim's cursor");
    let mut whole = Editor::new("x x\ny x", false);
    whole.keys(":%s#x#z#g<Enter>");
    assert_eq!(whole.text, "z z\ny z");
    whole.keys("u");
    assert_eq!(whole.text, "x x\ny x");
    let mut none = Editor::new("x x\ny x", false);
    assert_eq!(
        none.keys(":s/q/z/<Enter>"),
        [Effect::Note(Note::PatternNotFound("q".into()))]
    );
    assert_eq!(none.text, "x x\ny x");
}

#[test]
fn a_search_key_after_an_operator_is_not_supported_and_leaves_the_text() {
    for keys in ["dn", "dN", "d*", "d/", "d?"] {
        let mut editor = Editor::new("one two\n", false);
        assert_eq!(
            editor.keys(keys),
            [Effect::Note(Note::UnsupportedCommand(keys.into()))],
            "{keys}"
        );
        editor.keys("x");
        assert_eq!(editor.text, "ne two\n", "after {keys}");
    }
}

#[test]
fn a_search_opens_the_prompt_and_starts_no_macro() {
    let mut vim = Vim::new("one\ntwo");
    vim.keys("/");
    assert_eq!(vim.prompt().as_deref(), Some("/"));
    vim.keys("two<Enter>");
    assert_eq!(vim.cursor(), (1, 0));
    assert_eq!(vim.keys.show_mode(), None, "no recording");
    vim.keys(":/;");
    assert_eq!(vim.prompt().as_deref(), Some(":/;"));
}

#[test]
fn the_match_of_a_search_is_marked_until_noh() {
    let mut editor = Editor::new("alpha\nbeta\ngamma beta\n", false);
    assert_eq!(editor.keys("/beta<Enter>"), [Effect::Highlight(Some(((1, 0), (1, 4))))]);
    assert_eq!(editor.keys("n"), [Effect::Highlight(Some(((2, 6), (2, 10))))]);
    assert_eq!(editor.keys(":noh<Enter>"), [Effect::Highlight(None)]);
    assert_eq!(editor.keys(":nohlsearch<Enter>"), [Effect::Highlight(None)]);
}

#[test]
fn u_takes_back_what_undoes_a_line_and_u_after_it_takes_that_back() {
    let mut editor = Editor::new("one two", false);
    editor.keys("xxU");
    assert_eq!(editor.text, "one two");
    editor.keys("u");
    assert_eq!(editor.text, "e two", "u takes U back");
    let mut twice = Editor::new("one two", false);
    twice.keys("xxUU");
    assert_eq!(twice.text, "e two");
}

#[test]
fn a_yanked_line_pastes_after_a_visual_delete_and_undo_with_the_clipboard_shared_or_not() {
    for shared in [false, true] {
        let mut editor = Editor::new("one two\nthree\n", shared);
        editor.copy("older");
        editor.keys("vlldu");
        assert_eq!(editor.text, "one two\nthree\n");
        editor.keys("yyp");
        assert_eq!(editor.text, "one two\none two\nthree\n", "shared {shared}");
    }
}

#[test]
fn clipboard_text_with_a_last_line_break_pastes_as_whole_lines() {
    for (keys, shared) in [("\"+p", false), ("\"*p", false), ("p", true)] {
        let mut editor = Editor::new("one\ntwo", shared);
        editor.copy("x\n");
        editor.keys(keys);
        assert_eq!(editor.text, "one\nx\ntwo", "{keys}");
        let mut chars = Editor::new("one\ntwo", shared);
        chars.copy("x");
        chars.keys(keys);
        assert_eq!(chars.text, "oxne\ntwo", "{keys}");
    }
}

#[test]
fn the_star_register_is_the_clipboard() {
    let mut editor = Editor::new("one\ntwo\nthree", false);
    assert_eq!(editor.keys("\"*yy"), [Effect::Copy("one\n".into())]);
    editor.keys("j\"*p");
    assert_eq!(editor.text, "one\ntwo\none\nthree");
    assert_eq!(editor.clipboard, "one\n");
}

#[test]
fn insert_in_a_visual_block_types_in_every_line_and_esc_leaves_one_cursor() {
    let mut vim = Vim::new("abc\ndef\nghi\n");
    vim.keys("<C-V>jI");
    assert_eq!((vim.mode(), vim.carets()), (Mode::Insert, 2));
    vim.keys("X<Esc>");
    assert_eq!(
        (vim.text().as_str(), vim.mode(), vim.carets()),
        ("Xabc\nXdef\nghi\n", Mode::Normal, 1)
    );
}

#[test]
fn gv_selects_lines_and_blocks_again_in_their_mode() {
    let mut vim = Vim::new("abc\ndef\n");
    vim.keys("Vj<Esc>gv");
    assert_eq!(
        (vim.mode(), vim.selection()),
        (Mode::VisualLine, Some(((0, 0), (1, 0))))
    );
    let mut block = Vim::new("abc\ndef\n");
    block.keys("<C-V>jl<Esc>gv");
    assert_eq!(
        (block.mode(), block.selection()),
        (Mode::VisualBlock, Some(((0, 0), (1, 1))))
    );
}

#[test]
fn equals_hands_the_lines_it_reaches_to_the_editor() {
    let mut vim = Vim::new("  a\nb\n");
    vim.set_cursor(1, 0);
    assert_eq!(vim.keys("==").effects, [Effect::Reindent(1..2)]);
    let mut whole = Vim::new("a {\nb\n    }\n");
    assert_eq!(whole.keys("gg=G").effects, [Effect::Reindent(0..3)]);
    let mut motion = Vim::new("a\nb\nc\n");
    assert_eq!(motion.keys("=j").effects, [Effect::Reindent(0..2)]);
    assert_eq!(motion.keys("Vj=").effects, [Effect::Reindent(0..2)]);
    assert_eq!(motion.mode(), Mode::Normal);
}

#[test]
fn replacing_the_lines_keeps_the_selection_on_the_lines_it_was_on() {
    let mut vim = Vim::new("a\nb\nc\nd");
    vim.set_cursor(2, 0);
    vim.keys("vj");
    vim.replace_lines("b\nc\nd\ne\nf", 1);
    assert_eq!(vim.mode(), Mode::Visual);
    assert_eq!(vim.selection(), Some(((1, 0), (2, 0))));
    vim.keys("j");
    assert_eq!(vim.selection(), Some(((1, 0), (3, 0))));
}
