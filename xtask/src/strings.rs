//! The interface texts (E3): each crate keeps the entries of its own texts in `strings/<language>.strings`; this
//! joins them into the app's `resources/<language>.lproj/Localizable.strings` and checks that every German key the
//! code uses has its English entry, and that every language translates every English entry with the same
//! placeholders.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::root;

/// The interface languages, the same as Splat's (F191). macOS picks one of them from the user's preferred languages
/// by its `.lproj` folder; German is the key and needs no file.
pub(crate) const LANGUAGES: [&str; 35] = [
    "ar", "ca", "cs", "da", "de", "el", "en", "es", "fi", "fr", "he", "hi", "hr", "hu", "id", "it", "ja", "ko", "ms",
    "nl", "no", "pl", "pt_BR", "pt_PT", "ro", "ru", "sk", "sl", "sv", "th", "tr", "uk", "vi", "zh-Hans", "zh-Hant",
];

/// The language the keys are written in.
const SOURCE: &str = "de";

type Entries = BTreeMap<String, String>;

/// The folders whose code and `strings/` count: the app's and each crate's.
fn parts() -> Vec<PathBuf> {
    let mut parts = vec![root()];
    if let Ok(entries) = std::fs::read_dir(root().join("crates")) {
        let mut crates: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
        crates.sort();
        parts.extend(crates);
    }
    parts
}

/// The entries of a `.strings` file.
fn parse(text: &str) -> Entries {
    let mut entries = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(rest) = line.strip_prefix('"') else { continue };
        let Some((key, rest)) = split_quoted(rest) else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let Some(rest) = rest.trim_start().strip_prefix('"') else {
            continue;
        };
        let Some((value, _)) = split_quoted(rest) else { continue };
        entries.insert(key, value);
    }
    entries
}

/// The text up to the next unescaped quote, still escaped, and what follows it.
fn split_quoted(text: &str) -> Option<(String, &str)> {
    let mut escaped = false;
    for (at, c) in text.char_indices() {
        match c {
            '\\' if !escaped => escaped = true,
            '"' if !escaped => return Some((text[..at].to_string(), &text[at + 1..])),
            _ => escaped = false,
        }
    }
    None
}

/// The keys of `t("…")` and `tf("…", …)` in the Rust files under `dir`.
fn keys_in(dir: &Path, into: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if entry.file_name() != "target" {
                keys_in(&path, into);
            }
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for opener in ["t(", "tf("] {
            let mut rest = text.as_str();
            while let Some(at) = rest.find(opener) {
                let before = rest[..at].chars().next_back();
                rest = &rest[at + opener.len()..];
                if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                    continue;
                }
                let Some(quoted) = rest.trim_start().strip_prefix('"') else {
                    continue;
                };
                if let Some((key, _)) = split_quoted(quoted)
                    && !key.is_empty()
                {
                    into.push(key);
                }
            }
        }
    }
}

/// The `strings/` folder of one part with its entries by language.
struct Part {
    folder: String,
    by_language: BTreeMap<String, Entries>,
}

/// What a translation lacks against the English entries of its part.
#[derive(Default)]
struct Gaps {
    /// Per part and key, the languages without an entry.
    missing: BTreeMap<(String, String), Vec<String>>,
    /// Per part, the languages without a file.
    no_file: BTreeMap<String, Vec<String>>,
    /// Entries whose `{}` count or ellipsis differs from the English one.
    mismatched: Vec<String>,
    /// Entries whose key has no English entry any more.
    stale: Vec<String>,
}

/// How a translation must keep the English value's shape: `tf` fills `{}` in order, and `…` marks a menu item that
/// asks for more before it acts.
fn shape(value: &str) -> (usize, bool) {
    (value.matches("{}").count(), value.contains('…'))
}

/// Compares every language of `part` with its English entries.
fn compare(part: &Part, gaps: &mut Gaps) {
    let Some(english) = part.by_language.get("en") else {
        return;
    };
    for language in LANGUAGES.into_iter().filter(|l| *l != "en" && *l != SOURCE) {
        let Some(entries) = part.by_language.get(language) else {
            gaps.no_file
                .entry(part.folder.clone())
                .or_default()
                .push(language.into());
            continue;
        };
        for (key, value) in english {
            match entries.get(key) {
                None => gaps
                    .missing
                    .entry((part.folder.clone(), key.clone()))
                    .or_default()
                    .push(language.into()),
                Some(translated) if shape(translated) != shape(value) => {
                    let ((want, dots), (got, has)) = (shape(value), shape(translated));
                    gaps.mismatched.push(format!(
                        "{}/{language}.strings \"{key}\": {got} {{}} and {} where English has {want} and {}",
                        part.folder,
                        if has { "…" } else { "no …" },
                        if dots { "…" } else { "no …" },
                    ));
                }
                Some(_) => {}
            }
        }
        for key in entries.keys().filter(|key| !english.contains_key(*key)) {
            gaps.stale.push(format!("{}/{language}.strings \"{key}\"", part.folder));
        }
    }
}

/// The report of `gaps`, or none when every language has every entry.
fn report(gaps: &Gaps) -> Option<String> {
    let translated = LANGUAGES.len() - 2;
    let mut out = String::new();
    for (folder, languages) in &gaps.no_file {
        out.push_str(&format!("{folder}: no file for {}\n", languages.join(", ")));
    }
    if !gaps.missing.is_empty() {
        out.push_str("keys without a translation (add them to <folder>/<language>.strings):\n");
        for ((folder, key), languages) in &gaps.missing {
            let which = if languages.len() == translated {
                "every language".to_string()
            } else {
                languages.join(", ")
            };
            out.push_str(&format!("  {folder}: \"{key}\": {which}\n"));
        }
    }
    if !gaps.mismatched.is_empty() {
        out.push_str("translations whose placeholders or ellipsis differ from English:\n");
        for line in &gaps.mismatched {
            out.push_str(&format!("  {line}\n"));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// Reads the `strings/` folder of every part and collects the keys the code uses.
fn read_parts(used: &mut Vec<String>) -> Result<Vec<Part>, String> {
    let mut read = Vec::new();
    for path in parts() {
        keys_in(&path.join("src"), used);
        let folder = path.join("strings");
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut by_language = BTreeMap::new();
        for entry in entries.flatten() {
            let file = entry.path();
            if file.extension().is_none_or(|ext| ext != "strings") {
                continue;
            }
            let Some(language) = file.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            if !LANGUAGES.contains(&language) {
                return Err(format!("{}: {language} is none of the app's languages", file.display()));
            }
            let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
            by_language.insert(language.to_string(), parse(&text));
        }
        let folder = folder.strip_prefix(root()).unwrap_or(&folder).display().to_string();
        read.push(Part { folder, by_language });
    }
    Ok(read)
}

/// Joins the crates' entries into the app's resources and fails on keys without an English entry or without a
/// translation in one of the languages.
pub(crate) fn check() -> Result<(), String> {
    let mut used = Vec::new();
    let parts = read_parts(&mut used)?;
    for language in LANGUAGES {
        let mut joined = Entries::new();
        for part in &parts {
            if let Some(entries) = part.by_language.get(language) {
                joined.extend(entries.clone());
            }
        }
        let folder = root().join("resources").join(format!("{language}.lproj"));
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        let mut text =
            String::from("/* Joined by `cargo xtask strings` from the crates' strings/ folders; edit those. */\n");
        for (key, value) in joined {
            text.push_str(&format!("\"{key}\" = \"{value}\";\n"));
        }
        std::fs::write(folder.join("Localizable.strings"), text).map_err(|e| e.to_string())?;
    }
    let mut english = Entries::new();
    for part in &parts {
        english.extend(part.by_language.get("en").cloned().unwrap_or_default());
    }
    let mut missing: Vec<String> = used.into_iter().filter(|key| !english.contains_key(key)).collect();
    missing.sort();
    missing.dedup();
    let mut gaps = Gaps::default();
    for part in &parts {
        compare(part, &mut gaps);
    }
    for line in &gaps.stale {
        eprintln!("strings: no English entry any more, drop it: {line}");
    }
    let mut failure = String::new();
    if !missing.is_empty() {
        failure.push_str(&format!("keys without an English entry:\n{}\n", missing.join("\n")));
    }
    if let Some(gaps) = report(&gaps) {
        failure.push_str(&gaps);
    }
    if failure.is_empty() {
        println!(
            "strings: {} English entries, each in all {} languages",
            english.len(),
            LANGUAGES.len()
        );
        Ok(())
    } else {
        Err(failure.trim_end().to_string())
    }
}

/// Per language, the entries whose German and English keys differ (not only in case) but whose translation is the same, so that a
/// menu may show two items alike: lines of the translation and the English texts it stands for.
fn alike(english: &Entries, joined: &BTreeMap<String, Entries>) -> Vec<String> {
    let mut lines = Vec::new();
    for (language, entries) in joined {
        let mut by_value: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (key, value) in entries {
            by_value.entry(value).or_default().push(key);
        }
        for (value, keys) in by_value {
            let mut texts: Vec<&str> = keys
                .iter()
                .filter_map(|key| english.get(*key).map(String::as_str))
                .collect();
            texts.sort_unstable_by_key(|text| text.to_lowercase());
            texts.dedup_by_key(|text| text.to_lowercase());
            if texts.len() > 1 {
                lines.push(format!("{language}: \"{value}\" for \"{}\"", texts.join("\", \"")));
            }
        }
    }
    lines
}

/// `cargo xtask strings alike`: prints what [`alike`] finds, for review; it fails on nothing, since a language may
/// rightly use one word for two English ones.
pub(crate) fn print_alike() -> Result<(), String> {
    let parts = read_parts(&mut Vec::new())?;
    let mut joined: BTreeMap<String, Entries> = BTreeMap::new();
    for part in &parts {
        for (language, entries) in &part.by_language {
            joined.entry(language.clone()).or_default().extend(entries.clone());
        }
    }
    let english = joined.remove("en").unwrap_or_default();
    for line in alike(&english, &joined) {
        println!("{line}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_with_escaped_quotes_read() {
        let entries = parse("/* c */\n\"Sag \\\"Hallo\\\"\" = \"Say \\\"hello\\\"\";\n");
        assert_eq!(entries.get("Sag \\\"Hallo\\\""), Some(&"Say \\\"hello\\\"".to_string()));
    }

    #[test]
    fn the_languages_are_splats_thirty_five_with_german_and_english() {
        let mut sorted = LANGUAGES;
        sorted.sort_unstable();
        assert_eq!(sorted, LANGUAGES);
        assert!(LANGUAGES.contains(&SOURCE) && LANGUAGES.contains(&"en"));
    }

    #[test]
    fn a_translation_needs_every_key_with_its_placeholders_and_ellipsis() {
        let entries = |pairs: &[(&str, &str)]| -> Entries {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        let mut by_language = BTreeMap::new();
        by_language.insert(
            "en".to_string(),
            entries(&[("Über {}", "About {}"), ("Suchen …", "Find…"), ("Neu", "New")]),
        );
        for language in LANGUAGES.into_iter().filter(|l| *l != "en" && *l != SOURCE) {
            by_language.insert(
                language.to_string(),
                entries(&[
                    ("Über {}", "À propos de {}"),
                    ("Suchen …", "Rechercher…"),
                    ("Neu", "Nouveau"),
                ]),
            );
        }
        by_language.insert(
            "fr".into(),
            entries(&[("Über {}", "À propos"), ("Suchen …", "Rechercher")]),
        );
        by_language.remove("ja");
        let part = Part {
            folder: "strings".into(),
            by_language,
        };
        let mut gaps = Gaps::default();
        compare(&part, &mut gaps);
        let text = report(&gaps).unwrap();
        assert!(text.contains("strings: no file for ja\n"), "{text}");
        assert!(text.contains("  strings: \"Neu\": fr\n"), "{text}");
        assert!(text.contains("fr.strings \"Über {}\": 0 {} and no … where English has 1 and no …"));
        assert!(text.contains("fr.strings \"Suchen …\": 0 {} and no … where English has 0 and …"));
        assert!(report(&Gaps::default()).is_none());
    }

    #[test]
    fn one_translation_for_two_english_texts_is_listed() {
        let entries = |pairs: &[(&str, &str)]| -> Entries {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect()
        };
        let english = entries(&[
            ("Installieren …", "Install…"),
            ("Einstellungen …", "Settings…"),
            ("Neu", "New"),
        ]);
        let mut joined = BTreeMap::new();
        joined.insert(
            "vi".to_string(),
            entries(&[
                ("Installieren …", "Cài đặt…"),
                ("Einstellungen …", "Cài đặt…"),
                ("Neu", "Mới"),
            ]),
        );
        assert_eq!(
            alike(&english, &joined),
            ["vi: \"Cài đặt…\" for \"Install…\", \"Settings…\""]
        );
    }
}
