//! Dictation into the agent composer and the editor through lowspeak: one recording at a time, bound to the text
//! field that had the focus when it started, its text inserted at that field's cursor as one undo step.
//!
//! The app registers each field as a `DictationField`, holds the dictations through `Host` and hands over the
//! settings' `[dictation]` section; the crate knows no editor and no agent tab. Without the feature `engine` only this
//! file remains, so builds without dictation never compile lowspeak's whisper.cpp. How the menu writes the
//! dictation's shortcut:
//!
//! ```
//! assert_eq!(term_ui_dictation::written_shortcut("ctrl+alt+d"), "⌃⌥D");
//! ```

#[cfg(feature = "engine")]
mod hub;
#[cfg(feature = "engine")]
mod util;

#[cfg(feature = "engine")]
pub use hub::*;

/// A text field that dictation goes into, as the app sees it.
pub trait DictationField {
    /// Shows `text` in the window the field was in, for a dictation whose field closed; whether there was one.
    fn notice(&self, text: &str) -> bool;
}

/// The key binding's `chord`, such as "ctrl+alt+d", as macOS writes shortcuts; empty for one that is no chord.
pub fn written_shortcut(chord: &str) -> String {
    let Ok(chord) = chord.parse::<term_config::Chord>() else {
        return String::new();
    };
    let mut written: String = [
        (chord.ctrl, "⌃"),
        (chord.alt, "⌥"),
        (chord.shift, "⇧"),
        (chord.cmd, "⌘"),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, glyph)| *glyph)
    .collect();
    written.push_str(&chord.key.to_uppercase());
    written
}

#[cfg(test)]
mod tests {
    use super::written_shortcut;

    #[test]
    fn a_shortcut_is_written_in_the_order_of_macos_and_one_without_key_not_at_all() {
        assert_eq!(written_shortcut("shift+cmd+alt+ctrl+d"), "⌃⌥⇧⌘D");
        assert_eq!(written_shortcut("f5"), "F5");
        assert_eq!(written_shortcut(""), "");
        assert_eq!(written_shortcut("hyper+d"), "");
    }
}
