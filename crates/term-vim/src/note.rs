//! What Vim says in the status bar when a key or a command cannot do what it asks, as data the app puts in the
//! user's language; `Display` is Vim's English, with Vim's error numbers, for logs and tests.

use modalkit::editing::application::EmptyInfo;
use modalkit::editing::store::RegisterError;
use modalkit::errors::{EditError, UIError};
use modalkit::key::MacroError;

/// A message of the Vim mode for the status bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// Nothing matches this pattern (Vim's E486).
    PatternNotFound(String),
    /// An empty pattern or a search key repeats the last search, and there is none (E35).
    NoPreviousPattern,
    /// This pattern is no regular expression (E383); `detail` is the regex crate's English reason, for the log.
    InvalidPattern {
        /// The pattern as typed.
        pattern: String,
        /// Why it does not compile.
        detail: String,
    },
    /// The Ex command typed after `:` is none the Vim mode knows.
    UnsupportedCommand(String),
    /// A key that takes the word under the cursor found none (E348).
    NoWordUnderCursor,
    /// A key that works on the selection found none.
    NoSelection,
    /// A key jumps to a mark that was never set (E20).
    MarkNotSet,
    /// The text cannot be changed (E21).
    ReadOnly,
    /// `@@` repeats the last macro, and none ran yet (E748).
    NoPreviousMacro,
    /// A register the Vim mode does not keep (E354).
    UnsupportedRegister,
    /// These two characters after ⌃K name no digraph.
    InvalidDigraph(char, char),
    /// Anything else `modalkit` refused; its English words, for the log only.
    Other(String),
}

impl std::fmt::Display for Note {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Note::PatternNotFound(pattern) => write!(f, "E486: Pattern not found: {pattern}"),
            Note::NoPreviousPattern => f.write_str("E35: No previous regular expression"),
            Note::InvalidPattern { pattern, detail } => write!(f, "E383: Invalid search string: {pattern} ({detail})"),
            Note::UnsupportedCommand(command) => write!(f, "Command not supported: {command}"),
            Note::NoWordUnderCursor => f.write_str("E348: No string under cursor"),
            Note::NoSelection => f.write_str("No selection"),
            Note::MarkNotSet => f.write_str("E20: Mark not set"),
            Note::ReadOnly => f.write_str("E21: Cannot make changes"),
            Note::NoPreviousMacro => f.write_str("E748: No previously used register"),
            Note::UnsupportedRegister => f.write_str("E354: Invalid register name"),
            Note::InvalidDigraph(first, second) => write!(f, "Unknown digraph: {first}{second}"),
            Note::Other(detail) => f.write_str(detail),
        }
    }
}

impl Note {
    /// What the user meets of `error`, which an editing action of `modalkit` returned.
    pub(crate) fn of_edit(error: &EditError<EmptyInfo>) -> Note {
        match error {
            EditError::NoCursorWord => Note::NoWordUnderCursor,
            EditError::NoSearch => Note::NoPreviousPattern,
            EditError::NoSelection => Note::NoSelection,
            EditError::InvalidDigraph(first, second) => Note::InvalidDigraph(*first, *second),
            EditError::MarkNotSet(_) => Note::MarkNotSet,
            EditError::ReadOnly => Note::ReadOnly,
            EditError::Register(error) => Note::of_register(error),
            EditError::MacroFailure(error) => Note::of_macro(error),
            // The user's patterns go through `search`; `modalkit` compiles only patterns it built itself.
            EditError::InvalidRegex(_)
            | EditError::NeedConfirm(_)
            | EditError::WrongBuffer(_)
            | EditError::CursorGroupCombine(_)
            | EditError::IntConversionError(_)
            | EditError::IntParseError(_)
            | EditError::Unimplemented(_)
            | EditError::Failure(_) => Note::Other(error.to_string()),
            // The enum is `non_exhaustive`, so a variant `modalkit` adds lands here rather than failing the build.
            _ => Note::Other(error.to_string()),
        }
    }

    /// What the user meets of `error`, which a jump of `modalkit` returned.
    pub(crate) fn of_ui(error: &UIError<EmptyInfo>) -> Note {
        match error {
            UIError::EditingFailure(error) => Note::of_edit(error),
            UIError::Application(_)
            | UIError::IOError(_)
            | UIError::CommandFailure(_)
            | UIError::NeedConfirm(_)
            | UIError::NoList(_)
            | UIError::NoTab
            | UIError::NoWindow
            | UIError::Unimplemented(_)
            | UIError::Failure(_) => Note::Other(error.to_string()),
            _ => Note::Other(error.to_string()),
        }
    }

    fn of_register(error: &RegisterError) -> Note {
        match error {
            RegisterError::NoLastMacro => Note::NoPreviousMacro,
            RegisterError::Unimplemented(_) => Note::UnsupportedRegister,
            // Also `ClipboardImage`, which exists only with `modalkit`'s feature `clipboard`, off here.
            _ => Note::Other(error.to_string()),
        }
    }

    fn of_macro(error: &MacroError) -> Note {
        match error {
            MacroError::InvalidMacro(_) | MacroError::EmptyMacro | MacroError::LoopingMacro(_) => {
                Note::Other(error.to_string())
            }
            _ => Note::Other(error.to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use modalkit::prelude::{Mark, Register};

    use super::*;

    #[test]
    fn modalkits_errors_become_what_the_user_meets() {
        let edit = |error: EditError<EmptyInfo>| Note::of_edit(&error);
        assert_eq!(edit(EditError::MarkNotSet(Mark::BufferNamed('a'))), Note::MarkNotSet);
        assert_eq!(edit(EditError::NoCursorWord), Note::NoWordUnderCursor);
        assert_eq!(edit(EditError::NoSearch), Note::NoPreviousPattern);
        assert_eq!(edit(EditError::ReadOnly), Note::ReadOnly);
        assert_eq!(
            edit(EditError::InvalidDigraph('a', 'b')),
            Note::InvalidDigraph('a', 'b')
        );
        assert_eq!(
            edit(EditError::Register(RegisterError::NoLastMacro)),
            Note::NoPreviousMacro
        );
        assert_eq!(
            edit(EditError::Register(RegisterError::Unimplemented(Register::AltBufName))),
            Note::UnsupportedRegister
        );
        assert_eq!(
            edit(EditError::MacroFailure(MacroError::EmptyMacro)),
            Note::Other("Empty macro string".into())
        );
        assert_eq!(edit(EditError::Failure("odd".into())), Note::Other("Error: odd".into()));
        assert_eq!(
            Note::of_ui(&UIError::EditingFailure(EditError::NoSelection)),
            Note::NoSelection
        );
        assert_eq!(
            Note::of_ui(&UIError::NoTab),
            Note::Other("No tab currently selected".into())
        );
    }

    #[test]
    fn a_note_says_it_as_vim_does_with_vims_number() {
        assert_eq!(
            Note::PatternNotFound("x".into()).to_string(),
            "E486: Pattern not found: x"
        );
        assert_eq!(
            Note::NoPreviousPattern.to_string(),
            "E35: No previous regular expression"
        );
        assert_eq!(
            Note::UnsupportedCommand("vsplit".into()).to_string(),
            "Command not supported: vsplit"
        );
        assert_eq!(Note::MarkNotSet.to_string(), "E20: Mark not set");
        assert_eq!(Note::Other("Error: odd".into()).to_string(), "Error: odd");
    }
}
