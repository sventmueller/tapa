//! Key bindings in the file (F167): giving an action its chords with the fewest entries in `[keys]`, and which other
//! action loses a chord that way, for the settings window to warn about.

use std::collections::BTreeMap;

use toml_edit::Item;

use super::Document;
use crate::keys::{Action, Chord, KeyError, default_bindings, merged};

/// The chords in `text`, separated by spaces; a comma after a chord separates too, except in a chord such as
/// `cmd+,` whose key it is.
pub fn parse_chords(text: &str) -> Result<Vec<Chord>, KeyError> {
    let mut chords: Vec<Chord> = Vec::new();
    for token in text.split_whitespace() {
        for part in split_commas(token) {
            let chord: Chord = part.parse()?;
            if !chords.contains(&chord) {
                chords.push(chord);
            }
        }
    }
    Ok(chords)
}

fn split_commas(token: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let bytes = token.as_bytes();
    for (at, &byte) in bytes.iter().enumerate() {
        if byte == b',' && at > start && bytes[at - 1] != b'+' {
            parts.push(&token[start..at]);
            start = at + 1;
        }
    }
    if start < token.len() {
        parts.push(&token[start..]);
    }
    parts
}

/// The chords `action` has, separated by spaces, as `parse_chords` reads them.
pub fn chords_text(chords: &[Chord]) -> String {
    chords.iter().map(Chord::to_string).collect::<Vec<_>>().join(" ")
}

impl Document {
    /// The `[keys]` entries as the file writes them, chord to action name.
    pub fn key_entries(&self) -> BTreeMap<String, String> {
        let Some(table) = self.doc.get("keys").and_then(Item::as_table_like) else {
            return BTreeMap::new();
        };
        table
            .iter()
            .filter_map(|(chord, action)| Some((chord.to_string(), action.as_str()?.to_string())))
            .collect()
    }

    /// Gives `action` exactly `chords`: a chord taken from another action is rebound, a chord it loses goes back to
    /// its default or is unbound. Returns the chords other actions lose, each with the action that had it.
    pub fn bind(&mut self, action: Action, chords: &[Chord]) -> Vec<(Chord, Action)> {
        let entries = self.key_entries();
        let (bindings, _) = merged(&entries);
        let current: Vec<Chord> = bindings
            .iter()
            .filter(|binding| binding.action == action)
            .map(|binding| binding.chord.clone())
            .collect();
        let mut taken = Vec::new();
        for chord in chords.iter().filter(|chord| !current.contains(chord)) {
            if let Some(other) = bindings.iter().find(|binding| &binding.chord == chord) {
                taken.push((chord.clone(), other.action));
            }
            self.write_key(&entries, chord, action, true);
        }
        for chord in current.iter().filter(|chord| !chords.contains(chord)) {
            self.write_key(&entries, chord, action, false);
        }
        taken
    }

    /// Writes the entry that binds `chord` to `action`, or with `bound` false takes it from `action`, back to the
    /// default where that is another action.
    fn write_key(&mut self, entries: &BTreeMap<String, String>, chord: &Chord, action: Action, bound: bool) {
        let written: Vec<&String> = entries
            .keys()
            .filter(|raw| raw.parse::<Chord>().ok().as_ref() == Some(chord))
            .collect();
        for raw in &written {
            self.remove(&["keys", raw.as_str()]);
        }
        let by_default = default_of(chord) == Some(action);
        let value = match (bound, by_default) {
            (true, false) => action.name(),
            (false, true) => "",
            _ => return,
        };
        let raw = written.first().map_or_else(|| chord.to_string(), |raw| (*raw).clone());
        self.set(&["keys", raw.as_str()], value);
    }
}

fn default_of(chord: &Chord) -> Option<Action> {
    default_bindings()
        .into_iter()
        .find(|binding| &binding.chord == chord)
        .map(|binding| binding.action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;

    fn chord(text: &str) -> Chord {
        text.parse().unwrap()
    }

    fn bound(doc: &Document, action: Action) -> Vec<String> {
        let (config, warnings) = Config::parse(&doc.text());
        assert!(warnings.is_empty(), "{warnings:?}");
        let (bindings, warnings) = config.bindings();
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(crate::conflicts(&bindings).is_empty());
        bindings
            .into_iter()
            .filter(|binding| binding.action == action)
            .map(|binding| binding.chord.to_string())
            .collect()
    }

    #[test]
    fn chords_read_with_spaces_and_commas_but_not_the_comma_key() {
        let chords = parse_chords("cmd+d, ctrl+cmd+t cmd+,").unwrap();
        assert_eq!(chords_text(&chords), "cmd+d ctrl+cmd+t cmd+,");
        assert!(parse_chords("cmd+banana").is_err());
        assert!(parse_chords("  ").unwrap().is_empty());
    }

    #[test]
    fn a_chord_taken_from_another_action_is_reported() {
        let mut doc = Document::parse("version = 1\n# my keys\n[keys]\n\"cmd+j\" = \"find\"\n").unwrap();
        let taken = doc.bind(Action::NewTab, &[chord("cmd+t"), chord("cmd+f")]);
        assert_eq!(taken, [(chord("cmd+f"), Action::Find)]);
        assert_eq!(bound(&doc, Action::NewTab), ["cmd+t", "cmd+f"]);
        assert_eq!(bound(&doc, Action::Find), ["cmd+j"]);
        assert!(doc.text().contains("# my keys"));
    }

    #[test]
    fn a_default_chord_given_up_is_unbound_and_given_back_drops_the_entry() {
        let mut doc = Document::parse("").unwrap();
        assert!(doc.bind(Action::SplitRight, &[chord("cmd+d")]).is_empty());
        assert_eq!(bound(&doc, Action::SplitRight), ["cmd+d"]);
        assert_eq!(doc.key_entries().get("ctrl+cmd+t").map(String::as_str), Some(""));
        doc.bind(Action::SplitRight, &[chord("cmd+d"), chord("ctrl+cmd+t")]);
        assert!(doc.key_entries().is_empty(), "{:?}", doc.key_entries());
    }

    #[test]
    fn a_users_chord_given_up_restores_the_default_beneath_it() {
        let mut doc = Document::parse("[keys]\n\"CMD+K\" = \"find\"\n").unwrap();
        assert_eq!(bound(&doc, Action::Clear), Vec::<String>::new());
        doc.bind(Action::Find, &[chord("cmd+f")]);
        assert_eq!(bound(&doc, Action::Clear), ["cmd+k"]);
        assert!(doc.key_entries().is_empty(), "{:?}", doc.key_entries());
    }

    #[test]
    fn the_users_spelling_of_a_chord_is_kept() {
        let mut doc = Document::parse("[keys]\n\"shift+cmd+J\" = \"find\"\n").unwrap();
        doc.bind(Action::Paste, &[chord("cmd+v"), chord("shift+cmd+j")]);
        assert_eq!(doc.key_entries().get("shift+cmd+J").map(String::as_str), Some("paste"));
    }
}
