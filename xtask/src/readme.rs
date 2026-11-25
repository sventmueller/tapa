//! Checks that the README and its translations under `assets/readme/<language>/` stay complete and in step: one per
//! interface language, each with the link to the newest release's disk image and the issue link, and every relative link, picture and in-page anchor
//! resolving.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::root;

const ISSUES: &str = "https://github.com/sventmueller/tapa/issues/new/choose";

/// The README and every translation of it.
fn readmes() -> Vec<PathBuf> {
    let mut readmes = vec![root().join("README.md")];
    if let Ok(entries) = fs::read_dir(root().join("assets/readme")) {
        let mut translated: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path().join("README.md"))
            .filter(|readme| readme.is_file())
            .collect();
        translated.sort();
        readmes.extend(translated);
    }
    readmes
}

/// The anchors GitHub gives the headings of `text`, outside code blocks.
fn anchors(text: &str) -> BTreeSet<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut anchors = BTreeSet::new();
    let mut fenced = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if fenced || !line.starts_with('#') {
            continue;
        }
        let slug = slug(line.trim_start_matches('#').trim());
        let count = seen.entry(slug.clone()).or_insert(0);
        anchors.insert(if *count == 0 {
            slug.clone()
        } else {
            format!("{slug}-{count}")
        });
        *count += 1;
    }
    anchors
}

/// GitHub's anchor of a heading: lower case, spaces as hyphens, and only letters, marks, digits, `_` and `-` kept.
fn slug(heading: &str) -> String {
    heading
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' => Some('-'),
            '-' | '_' => Some(c),
            c if c.is_alphanumeric() || mark(c) => Some(c),
            _ => None,
        })
        .collect()
}

/// Whether `c` is a combining mark of a script the translations use, which `char::is_alphanumeric` partly misses.
fn mark(c: char) -> bool {
    matches!(c,
        '\u{0300}'..='\u{036F}' | '\u{0591}'..='\u{05BD}' | '\u{05BF}' | '\u{05C1}'..='\u{05C2}' | '\u{05C4}'..='\u{05C7}'
        | '\u{064B}'..='\u{065F}' | '\u{0670}' | '\u{0900}'..='\u{0963}' | '\u{0966}'..='\u{096F}'
        | '\u{0971}'..='\u{097F}' | '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

/// The targets of the Markdown links and pictures and of the HTML `src` attributes in `text`.
fn targets(text: &str) -> Vec<&str> {
    let mut targets = Vec::new();
    for (opener, closer) in [("](", ')'), ("src=\"", '"')] {
        let mut rest = text;
        while let Some(at) = rest.find(opener) {
            rest = &rest[at + opener.len()..];
            if let Some(end) = rest.find(closer) {
                targets.push(&rest[..end]);
            }
        }
    }
    targets
}

/// What is wrong with the links of `readme`, one line each.
fn broken_links(readme: &Path) -> Vec<String> {
    let text = fs::read_to_string(readme).unwrap_or_default();
    let own = anchors(&text);
    let folder = readme.parent().unwrap_or(Path::new("."));
    let mut broken = Vec::new();
    for target in targets(&text) {
        if target.starts_with("http") || target.starts_with("mailto:") {
            continue;
        }
        let (path, anchor) = target.split_once('#').unwrap_or((target, ""));
        let file = folder.join(path);
        if !path.is_empty() && !file.exists() {
            broken.push(format!("{}: {target} does not exist", readme.display()));
            continue;
        }
        if anchor.is_empty() {
            continue;
        }
        let known = if path.is_empty() {
            own.clone()
        } else {
            anchors(&fs::read_to_string(&file).unwrap_or_default())
        };
        if !known.contains(anchor) {
            broken.push(format!("{}: no heading for {target}", readme.display()));
        }
    }
    broken
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_a_readme_for_every_interface_language() {
        let languages = fs::read_dir(root().join("resources"))
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "lproj"))
            .count();
        assert_eq!(readmes().len(), languages);
    }

    #[test]
    fn every_readme_downloads_the_image_of_the_latest_release_and_takes_issues() {
        let download = format!(
            "{}/latest/download/{}",
            term_product::RELEASES,
            crate::updates::latest_image_name()
        );
        for readme in readmes() {
            let text = fs::read_to_string(&readme).unwrap();
            assert_eq!(text.matches(&download).count(), 2, "{}", readme.display());
            assert!(text.contains(ISSUES) && !text.contains(".zip"), "{}", readme.display());
        }
    }

    #[test]
    fn every_readme_links_to_all_the_others() {
        let all = readmes();
        for readme in &all {
            let text = fs::read_to_string(readme).unwrap();
            let folder = readme.parent().unwrap();
            let linked: BTreeSet<PathBuf> = targets(&text)
                .into_iter()
                .filter(|target| target.ends_with("README.md") && !target.starts_with("http"))
                .filter_map(|target| folder.join(target).canonicalize().ok())
                .collect();
            for other in all.iter().filter(|other| *other != readme) {
                assert!(
                    linked.contains(&other.canonicalize().unwrap()),
                    "{} lacks a link to {}",
                    readme.display(),
                    other.display()
                );
            }
        }
    }

    #[test]
    fn every_link_picture_and_anchor_of_the_readmes_resolves() {
        let broken: Vec<String> = readmes().iter().flat_map(|readme| broken_links(readme)).collect();
        assert!(broken.is_empty(), "{}", broken.join("\n"));
    }

    #[test]
    fn anchors_follow_github() {
        let text = "# Tapa\n## Search, selection and copying\n```\n# not a heading\n```\n## Für Entwickler\n## Tapa\n";
        let anchors = anchors(text);
        assert!(anchors.contains("search-selection-and-copying"));
        assert!(anchors.contains("für-entwickler"));
        assert!(anchors.contains("tapa-1"));
        assert!(!anchors.contains("not-a-heading"));
        assert_eq!(slug("डेवलपर्स के लिए"), "डेवलपर्स-के-लिए");
        assert_eq!(slug("「快速」終端機"), "快速終端機");
    }
}
