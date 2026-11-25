//! Completions for the word being typed from the command's spec (F93): its subcommands, options and argument values,
//! file and folder names where the argument is a path, and what the dynamic sources should look up for it.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use crate::spec::{self, Arg, Opt, Spec};
use crate::words::{self, Word};

/// The most candidates a completion lists.
const MOST: usize = 200;

/// What a candidate is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A command with a spec.
    Command,
    /// A subcommand of the command.
    Subcommand,
    /// An option.
    Option,
    /// A value the spec lists for an argument.
    Value,
    /// A file or folder name.
    Path,
    /// A value a dynamic source found, such as a branch.
    Dynamic,
}

/// A word that may take the place of the word being typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The whole word.
    pub text: String,
    /// What it does or is, one line.
    pub description: Option<String>,
    /// What it is.
    pub kind: Kind,
}

/// What a dynamic source should look up for the argument being typed (E8, delayed stage).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Wanted {
    /// The command's words before the one being typed.
    pub words: Vec<String>,
    /// How the spec's generators for the argument are named, such as `gitGenerators.localBranches`.
    pub markers: Vec<String>,
}

/// The candidates for the word being typed.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Completion {
    /// Where the word being typed starts in the line, in bytes.
    pub start: usize,
    /// The word as typed so far, quotes and backslashes taken out.
    pub word: String,
    /// The candidates, best first.
    pub candidates: Vec<Candidate>,
    /// What the dynamic sources could add.
    pub wanted: Option<Wanted>,
    /// The spec the instant stage needed but has not parsed yet; the delayed stage parses it.
    pub missing_spec: Option<String>,
}

impl Completion {
    /// `line` with the word being typed replaced by `candidate`, a space after it unless it is a folder or an option
    /// that takes its value after `=`.
    pub fn apply(&self, line: &str, candidate: &str) -> String {
        let keep = &line[..self.start.min(line.len())];
        let open = candidate.ends_with('/') || candidate.ends_with('=');
        format!("{keep}{}{}", escape(candidate), if open { "" } else { " " })
    }
}

/// Backslashes before what the shell would split or expand.
fn escape(word: &str) -> String {
    let mut out = String::with_capacity(word.len());
    for c in word.chars() {
        if " '\"\\$`&|;<>()*?!#".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The candidates for the last word of `line` in `folder`; with `may_parse` a spec not parsed yet is parsed, which
/// only the delayed stage may afford.
pub fn complete(line: &str, folder: &Path, home: &Path, may_parse: bool) -> Completion {
    let words = words::last_command(line);
    let Some(last) = words.last().cloned() else {
        return Completion::default();
    };
    let mut completion = Completion {
        start: last.start,
        word: last.text.clone(),
        ..Completion::default()
    };
    let lookup = |name: &str, missing: &mut Option<String>| {
        if may_parse {
            return spec::load(name);
        }
        match spec::cached(name) {
            Some(found) => found,
            None if spec::exists(name) => {
                *missing = Some(name.to_string());
                None
            }
            None => None,
        }
    };
    walk(&words, folder, home, &mut completion, &lookup);
    completion.candidates.truncate(MOST);
    completion
}

type Lookup<'l> = dyn Fn(&str, &mut Option<String>) -> Option<Arc<Spec>> + 'l;

/// Fills `out` for the command whose words are `words`, the last one being typed.
fn walk(words: &[Word], folder: &Path, home: &Path, out: &mut Completion, lookup: &Lookup<'_>) {
    let Some((last, before)) = words.split_last() else {
        return;
    };
    if before.is_empty() {
        commands(&last.text, folder, home, out);
        return;
    }
    let Some(root) = lookup(&before[0].text, &mut out.missing_spec) else {
        paths(&last.text, false, folder, home, out);
        return;
    };
    let mut state = State::new(&root);
    for (at, word) in before.iter().enumerate().skip(1) {
        if let Some(rest) = state.take(word, lookup, &mut out.missing_spec) {
            if rest {
                return walk(&words[at..], folder, home, out, lookup);
            }
            continue;
        }
    }
    if state.positional_is_command() {
        return walk(&words[before.len()..], folder, home, out, lookup);
    }
    state.candidates(last, before, folder, home, out);
}

/// Where the walk through a spec is.
struct State {
    specs: Vec<Arc<Spec>>,
    /// The path of subcommand indices from the last spec in `specs` down to the current subcommand.
    path: Vec<usize>,
    positional: usize,
    option_args: Vec<Arg>,
    used: HashSet<String>,
    after_dashes: bool,
}

impl State {
    fn new(root: &Arc<Spec>) -> State {
        State {
            specs: vec![root.clone()],
            path: Vec::new(),
            positional: 0,
            option_args: Vec::new(),
            used: HashSet::new(),
            after_dashes: false,
        }
    }

    fn current(&self) -> &Spec {
        let mut spec: &Spec = match self.specs.last() {
            Some(root) => root,
            None => empty_spec(),
        };
        for &index in &self.path {
            spec = &spec.subcommands[index];
        }
        spec
    }

    /// The options that apply: the current command's and the persistent ones of those above it.
    fn options(&self) -> Vec<&Opt> {
        let mut all: Vec<&Opt> = self.current().options.iter().collect();
        if let Some(root) = self.specs.last() {
            let mut spec: &Spec = root;
            for &index in &self.path {
                all.extend(spec.options.iter().filter(|o| o.is_persistent));
                spec = &spec.subcommands[index];
            }
        }
        all
    }

    /// Takes in a word before the one being typed; `Some(true)` when the rest of the line is a command of its own.
    fn take(&mut self, word: &Word, lookup: &Lookup<'_>, missing: &mut Option<String>) -> Option<bool> {
        let text = word.text.as_str();
        if !self.option_args.is_empty() {
            let arg = self.option_args.remove(0);
            if !(arg.is_optional && text.starts_with('-')) {
                return Some(arg.is_command);
            }
        }
        if text == "--" {
            self.after_dashes = true;
            return Some(false);
        }
        if !self.after_dashes && text.len() > 1 && text.starts_with('-') {
            let name = text.split_once('=').map_or(text, |(name, _)| name);
            let found = self
                .options()
                .into_iter()
                .find(|o| o.name.iter().any(|n| n == name))
                .cloned();
            if let Some(option) = found {
                self.used.extend(option.name.iter().cloned());
                if !text.contains('=') {
                    self.option_args = option.args;
                }
            }
            return Some(false);
        }
        if self.positional == 0
            && let Some(index) = self
                .current()
                .subcommands
                .iter()
                .position(|s| s.name.iter().any(|n| n == text))
        {
            self.path.push(index);
            if let Some(name) = self.current().load_spec.clone()
                && let Some(loaded) = lookup(&name, missing)
            {
                self.specs.push(loaded);
                self.path.clear();
            }
            return Some(false);
        }
        let arg = self.current().args.get(self.positional).cloned();
        match arg {
            Some(arg) if arg.is_command => Some(true),
            Some(arg) if arg.is_variadic => Some(false),
            _ => {
                self.positional += 1;
                Some(false)
            }
        }
    }

    fn positional_is_command(&self) -> bool {
        self.option_args.is_empty()
            && self
                .current()
                .args
                .get(self.positional)
                .is_some_and(|arg| arg.is_command)
    }

    fn candidates(&self, last: &Word, before: &[Word], folder: &Path, home: &Path, out: &mut Completion) {
        let word = last.text.as_str();
        let mut wanted = |arg: &Arg| {
            if !arg.dynamic.is_empty() || crate::sources::has_rule(&texts_of(before)) {
                out.wanted = Some(Wanted {
                    words: before.iter().map(|w| w.text.clone()).collect(),
                    markers: arg.dynamic.clone(),
                });
            }
        };
        if let Some(arg) = self.option_args.first()
            && !(arg.is_optional && word.starts_with('-'))
        {
            wanted(arg);
            values(arg, word, folder, home, out);
            return;
        }
        if word.starts_with('-') && !self.after_dashes {
            self.option_candidates(word, out);
            return;
        }
        if self.positional == 0 {
            for sub in &self.current().subcommands {
                if let Some(name) = sub.name.iter().find(|n| n.starts_with(word)) {
                    out.candidates.push(Candidate {
                        text: name.clone(),
                        description: sub.description.clone(),
                        kind: Kind::Subcommand,
                    });
                }
            }
        }
        let args = &self.current().args;
        let arg = args
            .get(self.positional)
            .or_else(|| args.last().filter(|a| a.is_variadic))
            .cloned();
        match arg {
            Some(arg) => {
                wanted(&arg);
                values(&arg, word, folder, home, out);
            }
            None if crate::sources::has_rule(&texts_of(before)) => wanted(&Arg::default()),
            None => {}
        }
    }

    fn option_candidates(&self, word: &str, out: &mut Completion) {
        for option in self.options() {
            if !option.repeatable() && option.name.iter().any(|n| self.used.contains(n)) {
                continue;
            }
            let long = word.starts_with("--");
            let name = option
                .name
                .iter()
                .filter(|n| n.starts_with(word))
                .max_by_key(|n| (n.starts_with("--") == long, std::cmp::Reverse(n.len())));
            if let Some(name) = name {
                let separator = (!option.args.is_empty()).then(|| option.separator()).flatten();
                out.candidates.push(Candidate {
                    text: format!("{name}{}", separator.unwrap_or_default()),
                    description: option.description.clone(),
                    kind: Kind::Option,
                });
            }
        }
    }
}

fn texts_of(words: &[Word]) -> Vec<&str> {
    words.iter().map(|w| w.text.as_str()).collect()
}

fn empty_spec() -> &'static Spec {
    static EMPTY: std::sync::OnceLock<Spec> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Spec::default)
}

/// The values the spec lists for `arg`, and paths where it takes them.
fn values(arg: &Arg, word: &str, folder: &Path, home: &Path, out: &mut Completion) {
    for suggestion in &arg.suggestions {
        if let Some(name) = suggestion.name.iter().find(|n| n.starts_with(word)) {
            out.candidates.push(Candidate {
                text: name.clone(),
                description: suggestion.description.clone(),
                kind: Kind::Value,
            });
        }
    }
    let files = arg.template.iter().any(|t| t == "filepaths");
    let folders = arg.template.iter().any(|t| t == "folders");
    if files || folders {
        paths(word, !files, folder, home, out);
    }
}

fn paths(word: &str, only_folders: bool, folder: &Path, home: &Path, out: &mut Completion) {
    for name in crate::paths::complete_path(word, folder, home) {
        if only_folders && !name.ends_with('/') {
            continue;
        }
        out.candidates.push(Candidate {
            text: name,
            description: None,
            kind: Kind::Path,
        });
    }
}

/// The first word: commands with a spec, or paths when it looks like one.
fn commands(word: &str, folder: &Path, home: &Path, out: &mut Completion) {
    if word.contains('/') || word.starts_with('.') || word.starts_with('~') {
        paths(word, false, folder, home, out);
        return;
    }
    if word.is_empty() {
        return;
    }
    for name in spec::names_starting(word, MOST) {
        let description = spec::cached(name).flatten().and_then(|spec| spec.description.clone());
        out.candidates.push(Candidate {
            text: name.to_string(),
            description,
            kind: Kind::Command,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(completion: &Completion) -> Vec<&str> {
        completion.candidates.iter().map(|c| c.text.as_str()).collect()
    }

    #[test]
    fn subcommands_options_and_arguments_come_from_the_spec() {
        let dir = term_base::testing::scratch("suggest-complete");
        let git = complete("git comm", &dir, &dir, true);
        assert_eq!(git.start, 4);
        assert!(texts(&git).contains(&"commit"), "{:?}", texts(&git));
        assert!(git.candidates.iter().any(|c| c.description.is_some()));
        let options = complete("git commit --am", &dir, &dir, true);
        assert!(texts(&options).contains(&"--amend"), "{:?}", texts(&options));
        let used = complete("git commit --amend --am", &dir, &dir, true);
        assert!(!texts(&used).contains(&"--amend"), "used once");
        let branches = complete("git checkout ", &dir, &dir, true);
        assert!(branches.wanted.is_some(), "branches come from a dynamic source");
        assert!(texts(&complete("gi", &dir, &dir, true)).contains(&"git"));
    }

    #[test]
    fn a_command_after_sudo_completes_as_its_own_and_paths_where_the_spec_says() {
        let dir = term_base::testing::scratch("suggest-complete-paths");
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("setup.sh"), "").unwrap();
        let sudo = complete("sudo git sta", &dir, &dir, true);
        assert!(texts(&sudo).contains(&"status"), "{:?}", texts(&sudo));
        let cd = complete("cd s", &dir, &dir, true);
        assert_eq!(texts(&cd), ["src/"], "cd takes folders only");
        let unknown = complete("nosuchtool s", &dir, &dir, true);
        assert_eq!(texts(&unknown), ["setup.sh", "src/"]);
    }

    #[test]
    fn the_instant_stage_never_parses_and_names_the_spec_it_lacks() {
        let dir = term_base::testing::scratch("suggest-complete-instant");
        let first = complete("rustup to", &dir, &dir, false);
        if spec::cached("rustup").is_none() {
            assert_eq!(first.missing_spec.as_deref(), Some("rustup"));
        }
        spec::load("rustup");
        let then = complete("rustup to", &dir, &dir, false);
        assert!(texts(&then).contains(&"toolchain"), "{:?}", texts(&then));
    }

    #[test]
    fn applying_a_candidate_escapes_it_and_leaves_folders_open() {
        let completion = Completion {
            start: 3,
            ..Completion::default()
        };
        assert_eq!(completion.apply("cd my", "my dir/"), "cd my\\ dir/");
        assert_eq!(completion.apply("ls x", "x.txt"), "ls x.txt ");
    }
}
