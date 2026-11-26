//! The colours of the command at the shell's prompt without the input field (F92): the parts the field colours,
//! handed to the renderer, which draws them over the shell's own text where the shell left it in the text's colour.

use objc2::DefinedClass;
use term_render::tint::{Kind, Span, Tint};
use term_session::Point;
use term_suggest::Part;

use crate::TerminalView;

/// How long a spec the colours need has to be parsed before they are looked for again.
const SPEC_WAIT: std::time::Duration = std::time::Duration::from_millis(250);

/// The colour of `part`, where it has one.
fn kind_of(part: Part) -> Option<Kind> {
    match part {
        Part::Command => Some(Kind::Command),
        Part::Subcommand => Some(Kind::Subcommand),
        Part::Option => Some(Kind::Option),
        Part::Variable => Some(Kind::Variable),
        Part::Operator => Some(Kind::Operator),
        Part::Unknown | Part::Argument => None,
    }
}

/// The character `byte` of `text` falls on.
fn char_at(text: &str, byte: usize) -> usize {
    text.get(..byte).map_or(0, |head| head.chars().count())
}

impl TerminalView {
    /// Finds the colours of the line typed at the prompt anew; whether they changed and the view has to be drawn.
    pub(super) fn raw_tint(&self) -> bool {
        let tint = self.find_tint();
        if *self.ivars().tint.borrow() == tint {
            return false;
        }
        *self.ivars().tint.borrow_mut() = tint;
        true
    }

    /// The colours of the line typed at the prompt as the settings ask, while the shell waits there without the field.
    fn find_tint(&self) -> Option<Tint> {
        let input = crate::host().config().input.clone();
        if !input.highlighting && !input.underline_errors || !self.at_raw_prompt() {
            return None;
        }
        let (typed, cursor) = self.with_session(|session| session.typed_line()).flatten()?;
        let text = typed.text;
        if text.trim().is_empty() || text.contains('\n') {
            return None;
        }
        let start = Point {
            line: cursor.line.checked_sub(typed.rows_up)?,
            column: typed.start_column,
        };
        let highlight = self.command_parts(&text);
        if let Some(missing) = &highlight.missing_spec {
            term_suggest::warm_spec(missing);
            let id = self.id();
            crate::kit::main_after(SPEC_WAIT, move || {
                if let Some(view) = crate::find(id) {
                    view.raw_suggest();
                }
            });
        }
        let spans = highlight
            .spans
            .into_iter()
            .map(|span| Span {
                chars: char_at(&text, span.range.start)..char_at(&text, span.range.end),
                kind: kind_of(span.part).filter(|_| input.highlighting),
                unknown: input.underline_errors && span.part == Part::Unknown,
            })
            .filter(|span| span.kind.is_some() || span.unknown)
            .collect();
        Some(Tint { start, cursor, spans })
    }

    /// The words of the command typed at the prompt with the colours drawn over them, as `word:Kind`, `!` after a
    /// word the shell cannot run, for the self-test.
    #[cfg(feature = "selftest")]
    pub fn raw_tint_parts(&self) -> Vec<String> {
        let Some(tint) = self.ivars().tint.borrow().clone() else {
            return Vec::new();
        };
        let text = self
            .with_session(|session| session.typed_line())
            .flatten()
            .map(|(typed, _)| typed.text)
            .unwrap_or_default();
        let chars: Vec<char> = text.chars().collect();
        tint.spans
            .iter()
            .map(|span| {
                let word: String = chars.get(span.chars.clone()).unwrap_or_default().iter().collect();
                let kind = span.kind.map_or_else(|| "-".to_string(), |kind| format!("{kind:?}"));
                format!("{word}:{kind}{}", if span.unknown { "!" } else { "" })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_offsets_become_character_offsets() {
        assert_eq!(char_at("ls ä 😀 x", 0), 0);
        assert_eq!(char_at("ls ä 😀 x", "ls ä".len()), 4);
        assert_eq!(char_at("ls ä 😀 x", "ls ä 😀".len()), 6);
    }

    #[test]
    fn only_coloured_parts_have_a_kind() {
        assert_eq!(kind_of(Part::Command), Some(Kind::Command));
        assert_eq!(kind_of(Part::Argument), None);
        assert_eq!(kind_of(Part::Unknown), None);
    }
}
