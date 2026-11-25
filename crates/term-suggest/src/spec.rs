//! The Fig completion specs (MIT, E8): their static part, converted once by `xtask/specs` into `specs/fig.tsv` and
//! bundled. One line per spec, sorted by name, so a spec is found by bisecting the lines without reading the rest; a
//! spec is parsed the first time it is asked for and kept.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Deserialize;

/// The specs, `name\tjson\n` per line, sorted by name.
static SPECS: &str = include_str!("../specs/fig.tsv");

/// A command, or a subcommand of one.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Spec {
    /// Its names; aliases after the first.
    pub name: Vec<String>,
    /// What it does, one line.
    pub description: Option<String>,
    /// Its subcommands.
    pub subcommands: Vec<Spec>,
    /// Its options.
    pub options: Vec<Opt>,
    /// Its positional arguments.
    pub args: Vec<Arg>,
    /// The spec that holds the rest of this subcommand.
    pub load_spec: Option<String>,
}

/// An option such as `-m` or `--message`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Opt {
    /// Its spellings.
    pub name: Vec<String>,
    /// What it does.
    pub description: Option<String>,
    /// The arguments it takes.
    pub args: Vec<Arg>,
    /// Whether subcommands take it too.
    pub is_persistent: bool,
    /// Whether it may be given more than once; Fig also writes a count here.
    pub is_repeatable: serde_json::Value,
    /// Whether its argument follows after `=` in the same word.
    pub requires_equals: bool,
    /// The same, as Fig writes it now: `true` for `=`, or the separator.
    pub requires_separator: serde_json::Value,
}

/// An argument of a command or an option.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Arg {
    /// What it is called, as shown while it is typed.
    pub name: Vec<String>,
    /// What it is.
    pub description: Option<String>,
    /// The values it may take.
    pub suggestions: Vec<Suggestion>,
    /// `filepaths` or `folders`: names in the folder complete it.
    pub template: Vec<String>,
    /// Whether it may be left out.
    pub is_optional: bool,
    /// Whether it takes any number of words.
    pub is_variadic: bool,
    /// Whether it is a command line of its own, as after `sudo`.
    pub is_command: bool,
    /// What computes its values in Fig, by the generator's name or script, for the dynamic sources.
    pub dynamic: Vec<String>,
}

/// A value an argument may take.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Suggestion {
    /// Its spellings.
    pub name: Vec<String>,
    /// What it means.
    pub description: Option<String>,
}

impl Opt {
    /// The separator between the option and its argument in one word, such as `=`.
    pub(crate) fn separator(&self) -> Option<String> {
        match &self.requires_separator {
            serde_json::Value::Bool(true) => Some("=".into()),
            serde_json::Value::String(s) => Some(s.clone()),
            _ => self.requires_equals.then(|| "=".into()),
        }
    }

    pub(crate) fn repeatable(&self) -> bool {
        match &self.is_repeatable {
            serde_json::Value::Bool(b) => *b,
            serde_json::Value::Number(_) => true,
            _ => false,
        }
    }
}

fn cache() -> &'static Mutex<HashMap<String, Option<Arc<Spec>>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Arc<Spec>>>>> = OnceLock::new();
    CACHE.get_or_init(Mutex::default)
}

/// The spec named `name` if it was parsed already: what the instant stage may use within its 2 ms. `Some(None)`
/// when there is none.
pub(crate) fn cached(name: &str) -> Option<Option<Arc<Spec>>> {
    cache().lock().ok()?.get(name).cloned()
}

/// The spec named `name`, parsed now if it was not; the delayed stage's call, as a large spec takes milliseconds.
pub(crate) fn load(name: &str) -> Option<Arc<Spec>> {
    if let Some(known) = cached(name) {
        return known;
    }
    let spec = line(name)
        .and_then(|json| serde_json::from_str::<Spec>(json).ok())
        .map(Arc::new);
    if let Ok(mut cache) = cache().lock() {
        cache.insert(name.to_string(), spec.clone());
    }
    spec
}

/// Whether there is a spec for `name`, without parsing it.
pub(crate) fn exists(name: &str) -> bool {
    line(name).is_some()
}

/// The JSON of the spec `name`, found by bisecting the sorted lines.
fn line(name: &str) -> Option<&'static str> {
    let (mut low, mut high) = (0, SPECS.len());
    while low < high {
        let middle = start_of_line(low + (high - low) / 2);
        let (key, json, end) = entry_at(middle)?;
        match key.cmp(name) {
            std::cmp::Ordering::Equal => return Some(json),
            std::cmp::Ordering::Less => low = end,
            std::cmp::Ordering::Greater => {
                if middle == low {
                    return None;
                }
                high = middle;
            }
        }
    }
    None
}

/// The names of the specs starting with `prefix`, at most `limit`, sorted; sub-specs such as `aws/s3` left out.
pub(crate) fn names_starting(prefix: &str, limit: usize) -> Vec<&'static str> {
    let (mut low, mut high) = (0, SPECS.len());
    while low < high {
        let middle = start_of_line(low + (high - low) / 2);
        let Some((key, _, end)) = entry_at(middle) else { break };
        if key < prefix {
            low = end;
        } else if middle == low {
            break;
        } else {
            high = middle;
        }
    }
    let mut names = Vec::new();
    let mut at = low;
    while names.len() < limit
        && let Some((key, _, end)) = entry_at(at)
        && key.starts_with(prefix)
    {
        if !key.contains('/') {
            names.push(key);
        }
        at = end;
    }
    names
}

fn start_of_line(at: usize) -> usize {
    SPECS[..at].rfind('\n').map_or(0, |newline| newline + 1)
}

/// The name and JSON of the line starting at `at`, and where the next begins.
fn entry_at(at: usize) -> Option<(&'static str, &'static str, usize)> {
    let rest = SPECS.get(at..)?;
    let end = rest.find('\n').map_or(SPECS.len(), |newline| at + newline + 1);
    let (key, json) = SPECS[at..end].trim_end_matches('\n').split_once('\t')?;
    Some((key, json, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_found_by_name_and_prefix() {
        let git = load("git").expect("git has a spec");
        assert!(git.subcommands.iter().any(|sub| sub.name.iter().any(|n| n == "commit")));
        assert!(load("no-such-command-here").is_none());
        assert!(exists("cargo") && exists("act") && !exists("zzzz-none"));
        assert!(names_starting("gi", 50).contains(&"git"));
        assert!(names_starting("aws", 50).iter().all(|name| !name.contains('/')));
        let first = SPECS.split_once('\t').map(|(name, _)| name).unwrap_or_default();
        assert!(exists(first), "the first line is found too");
    }

    #[test]
    fn every_line_is_a_spec_in_order() {
        let mut previous = "";
        for line in SPECS.lines() {
            let (name, json) = line.split_once('\t').expect("name and JSON");
            assert!(name > previous, "{name} after {previous}");
            assert!(serde_json::from_str::<Spec>(json).is_ok(), "{name} parses");
            previous = name;
        }
    }
}
