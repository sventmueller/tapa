//! The field's suggestions (E8), as Warp shows them: the rest of the best line faint right after the text, the
//! history first and then the spec's best candidate, and the delayed stage after it, whose answer shows only while
//! the field still holds the same text. With nothing typed, the history's next command shows at once and a fix or the
//! project's next step replaces it when the pool finds one. Tab takes a suggestion that is the one line meant;
//! ⌃Space, or Tab otherwise, opens the completion menu (`menu`).

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::DefinedClass;
use splat_i18n::tf;
use term_suggest::{Completion, Next, Reason};

use super::InputField;
use crate::TerminalView;

thread_local! {
    /// The delayed stage's last completion per view, with the line it is for, for Tab and the menu.
    pub(super) static LATER: RefCell<HashMap<u64, (String, Completion)>> = RefCell::new(HashMap::new());
}

/// The fewest characters typed before a suggestion shows; after a single letter it mostly guesses wrong (owner's
/// decision, 2026-11-25).
const FEWEST_TYPED: usize = 2;

/// Whether enough of `text` is typed for a suggestion to show.
pub(crate) fn typed_enough(text: &str) -> bool {
    text.trim_start().chars().count() >= FEWEST_TYPED
}

/// What of `line` shows after `text`: its rest, or nothing when it does not continue the text.
fn rest_of<'l>(text: &str, line: &'l str) -> Option<&'l str> {
    line.strip_prefix(text)
        .and_then(|rest| rest.lines().next())
        .filter(|rest| !rest.is_empty())
}

fn next_hint(next: &Next) -> String {
    match next.reason {
        Reason::Typo => tf("{}  (Tippfehler?)  →", &[&next.line]),
        Reason::Install => tf("{}  (nicht installiert)  →", &[&next.line]),
        Reason::Project => tf("{}  (Lockfile geändert)  →", &[&next.line]),
    }
}

/// The line the best candidate of `completion` makes of `text`; none for a path, which Tab completes.
fn best_line(completion: &Completion, text: &str) -> Option<String> {
    let best = completion.candidates.first()?;
    if completion.word.is_empty() && best.kind == term_suggest::Kind::Path {
        return None;
    }
    let line = completion.apply(text, &best.text).trim_end().to_string();
    (line != text.trim_end()).then_some(line)
}

impl TerminalView {
    /// The line the instant stage suggests for `text`: the history's, else the spec's best candidate, and whether it
    /// is the one line meant rather than one candidate of several; with the spec it would have needed but that is not
    /// parsed yet.
    pub(crate) fn instant_line(&self, text: &str) -> (Option<(String, bool)>, Option<String>) {
        if let Some(line) = crate::host().suggest(self.id(), text) {
            return (Some((line, true)), None);
        }
        let completion = term_suggest::complete(text, &self.cwd(), &term_base::home(), false);
        let sure = completion.candidates.len() == 1;
        (
            best_line(&completion, text).map(|line| (line, sure)),
            completion.missing_spec,
        )
    }

    /// The suggestion for `text` in `field`: the instant stage now, the delayed one asked for.
    pub(super) fn field_suggest(&self, field: &InputField, text: &str) {
        let id = self.id();
        let history = crate::host().suggest(id, text);
        let mut suggestion = history.clone();
        let mut sure = history.is_some();
        if !crate::host().config().input.suggestions {
            term_suggest::cancel(id);
        } else if text.trim().is_empty() {
            self.ask_next(id);
        } else if !typed_enough(text) {
            suggestion = None;
            term_suggest::cancel(id);
            term_suggest::warm_spec(text.trim());
        } else {
            let (folder, home) = (self.cwd(), term_base::home());
            if !text.contains(char::is_whitespace) {
                term_suggest::warm_spec(text);
            }
            let completion = term_suggest::complete(text, &folder, &home, false);
            if history.is_none() {
                suggestion = best_line(&completion, text);
                sure = completion.candidates.len() == 1;
            }
            // A shell on another machine has its own branches, scripts and containers, which this Mac cannot know.
            let remote = self.ivars().shell.borrow().remote;
            if completion.missing_spec.is_some() || (completion.wanted.is_some() && !remote) {
                self.ask_later(id, text, history.is_some());
            } else {
                term_suggest::cancel(id);
            }
        }
        let rest = suggestion.as_deref().and_then(|line| rest_of(text, line));
        field.set_ghost(rest);
        let shows = rest.is_some();
        *self.ivars().suggestion.borrow_mut() = suggestion.filter(|_| shows);
        self.ivars().suggestion_sure.set(shows && sure);
    }

    fn ask_later(&self, id: u64, text: &str, has_history: bool) {
        let line = text.to_string();
        term_suggest::complete_later(id, line.clone(), self.cwd(), term_base::home(), move |completion| {
            term_base::main_soon(move || {
                let Some(view) = crate::find(id) else { return };
                let Some(field) = view.field_with_text(&line) else {
                    return;
                };
                LATER.with(|later| later.borrow_mut().insert(id, (line.clone(), completion.clone())));
                if view.menu_is_open() {
                    drop(field);
                    view.menu_follow(&line);
                    return;
                }
                if has_history {
                    return;
                }
                if let Some(suggested) = best_line(&completion, &line)
                    && let Some(rest) = rest_of(&line, &suggested)
                {
                    field.set_ghost(Some(rest));
                    *view.ivars().suggestion.borrow_mut() = Some(suggested);
                    view.ivars().suggestion_sure.set(completion.candidates.len() == 1);
                }
                view.field_colour(&field, &line);
            });
        });
    }

    fn ask_next(&self, id: u64) {
        let Some(context) = crate::host().context(id) else {
            term_suggest::cancel(id);
            return;
        };
        term_suggest::next_later(id, context, move |next| {
            let Some(next) = next else { return };
            term_base::main_soon(move || {
                let Some(view) = crate::find(id) else { return };
                if let Some(field) = view.field_with_text("") {
                    field.set_ghost(Some(&next_hint(&next)));
                    *view.ivars().suggestion.borrow_mut() = Some(next.line);
                    view.ivars().suggestion_sure.set(true);
                }
            });
        });
    }

    /// The field while it shows and holds `text`, else none: an answer for older text is stale.
    fn field_with_text(&self, text: &str) -> Option<std::cell::Ref<'_, InputField>> {
        let field = self.ivars().field.try_borrow().ok()?;
        let field = std::cell::Ref::filter_map(field, Option::as_ref).ok()?;
        (field.is_visible() && field.text().trim() == text.trim()).then_some(field)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_suggestion_waits_for_the_second_character() {
        assert!(!typed_enough("g"));
        assert!(!typed_enough("  g"));
        assert!(typed_enough("gi"));
        assert!(typed_enough("ä😀"));
    }

    #[test]
    fn the_rest_shows_only_where_the_line_continues_the_text() {
        assert_eq!(rest_of("git st", "git status"), Some("atus"));
        assert_eq!(rest_of("git status", "git status"), None);
        assert_eq!(rest_of("ls", "cd x"), None);
        assert_eq!(rest_of("echo", "echo a\nb"), Some(" a"), "the first line only");
    }

    #[test]
    fn the_best_candidate_completes_the_line_but_not_a_bare_path() {
        let dir = std::env::temp_dir().join(format!("field-suggest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let completion = term_suggest::complete("git comm", &dir, &dir, true);
        assert_eq!(best_line(&completion, "git comm").as_deref(), Some("git commit"));
        let bare = term_suggest::complete("cat ", &dir, &dir, true);
        assert_eq!(best_line(&bare, "cat "), None);
    }
}
