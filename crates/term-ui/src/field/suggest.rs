//! The field's suggestions (E8): the instant stage on every change, the history first and then the spec's best
//! candidate, and the delayed stage after it, whose answer shows only while the field still holds the same text. With
//! nothing typed, the history's next command shows at once and a fix or the project's next step replaces it when the
//! pool finds one. Tab completes the word being typed from the spec, the dynamic sources and the paths.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::DefinedClass;
use splat_i18n::tf;
use term_suggest::{Completion, Next, Reason};

use super::InputField;
use crate::TerminalView;

thread_local! {
    /// The delayed stage's last completion per view, with the line it is for, for Tab.
    static LATER: RefCell<HashMap<u64, (String, Completion)>> = RefCell::new(HashMap::new());
}

/// The grey hint for `line` suggested after `text`: its rest, or all of it with nothing typed.
fn rest_hint(text: &str, line: &str, description: Option<&str>) -> String {
    let rest = line
        .get(text.len()..)
        .filter(|_| line.starts_with(text))
        .unwrap_or(line);
    match description {
        Some(description) => format!("{rest} · {description}  →"),
        None => format!("{rest}  →"),
    }
}

fn next_hint(next: &Next) -> String {
    match next.reason {
        Reason::Typo => tf("{}  (Tippfehler?)  →", &[&next.line]),
        Reason::Install => tf("{}  (nicht installiert)  →", &[&next.line]),
        Reason::Project => tf("{}  (Lockfile geändert)  →", &[&next.line]),
    }
}

/// The line the best candidate of `completion` makes of `text`, and its description; none for a path, which Tab
/// completes.
fn best_line(completion: &Completion, text: &str) -> Option<(String, Option<String>)> {
    let best = completion.candidates.first()?;
    if completion.word.is_empty() && best.kind == term_suggest::Kind::Path {
        return None;
    }
    let line = completion.apply(text, &best.text).trim_end().to_string();
    (line != text.trim_end()).then(|| (line, best.description.clone()))
}

impl TerminalView {
    /// The suggestion for `text` in `field`: the instant stage now, the delayed one asked for.
    pub(super) fn field_suggest(&self, field: &InputField, text: &str) {
        let id = self.id();
        let history = crate::host().suggest(id, text);
        let mut hint = history.as_deref().map(|line| rest_hint(text, line, None));
        let mut suggestion = history.clone();
        if !crate::host().config().input.suggestions {
            term_suggest::cancel(id);
        } else if text.trim().is_empty() {
            self.ask_next(id);
        } else {
            let (folder, home) = (self.cwd(), term_base::home());
            if !text.contains(char::is_whitespace) {
                term_suggest::warm_spec(text);
            }
            let completion = term_suggest::complete(text, &folder, &home, false);
            if history.is_none()
                && let Some((line, description)) = best_line(&completion, text)
            {
                hint = Some(rest_hint(text, &line, description.as_deref()));
                suggestion = Some(line);
            }
            if completion.missing_spec.is_some() || completion.wanted.is_some() {
                self.ask_later(id, text, history.is_some());
            } else {
                term_suggest::cancel(id);
            }
        }
        *self.ivars().suggestion.borrow_mut() = suggestion;
        field.set_hint(hint.as_deref());
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
                if has_history {
                    return;
                }
                if let Some((suggested, description)) = best_line(&completion, &line) {
                    field.set_hint(Some(&rest_hint(&line, &suggested, description.as_deref())));
                    *view.ivars().suggestion.borrow_mut() = Some(suggested);
                }
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
                    field.set_hint(Some(&next_hint(&next)));
                    *view.ivars().suggestion.borrow_mut() = Some(next.line);
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

    /// Tab: completes the word being typed from the spec, the dynamic sources the delayed stage found and the paths;
    /// with several, as far as they agree, listed in grey.
    pub(crate) fn field_complete(&self) {
        let Some(text) = self.with_field(|field, _| field.text()) else {
            return;
        };
        let id = self.id();
        let later = LATER.with(|later| {
            later
                .borrow()
                .get(&id)
                .filter(|(line, _)| *line == text)
                .map(|(_, c)| c.clone())
        });
        let completion = later.unwrap_or_else(|| term_suggest::complete(&text, &self.cwd(), &term_base::home(), true));
        let names: Vec<&str> = completion.candidates.iter().map(|c| c.text.as_str()).collect();
        let Some(first) = names.first() else {
            objc2_app_kit::NSBeep();
            return;
        };
        let completed = if names.len() == 1 {
            completion.apply(&text, first)
        } else {
            let common = names.iter().fold(first.to_string(), |common, name| {
                common
                    .chars()
                    .zip(name.chars())
                    .take_while(|(a, b)| a == b)
                    .map(|(a, _)| a)
                    .collect()
            });
            let applied = completion.apply(&text, &common);
            if common.ends_with(['/', '=']) {
                applied
            } else {
                applied.trim_end().to_string()
            }
        };
        let list = (names.len() > 1).then(|| names.iter().take(12).copied().collect::<Vec<_>>().join("  "));
        self.with_field(|field, _| {
            if completed.len() >= text.len() {
                field.set_text(&completed);
            }
            if let Some(list) = &list {
                field.set_hint(Some(list));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hints_show_the_rest_and_the_description() {
        assert_eq!(rest_hint("git st", "git status", None), "atus  →");
        assert_eq!(rest_hint("", "git commit", None), "git commit  →");
        assert_eq!(rest_hint("git comm", "git commit", Some("Record")), "it · Record  →");
    }

    #[test]
    fn the_best_candidate_completes_the_line_but_not_a_bare_path() {
        let dir = std::env::temp_dir().join(format!("field-suggest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "").unwrap();
        let completion = term_suggest::complete("git comm", &dir, &dir, true);
        assert_eq!(
            best_line(&completion, "git comm").map(|b| b.0).as_deref(),
            Some("git commit")
        );
        let bare = term_suggest::complete("cat ", &dir, &dir, true);
        assert_eq!(best_line(&bare, "cat "), None);
    }
}
