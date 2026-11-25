//! The colours of a command line (F92), as Warp tells the parts of a command apart: the command, the subcommands its
//! spec knows, options, other arguments and variables, and what joins or redirects commands. A command the shell
//! cannot run is marked, as Warp underlines it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::complete::State;
use crate::spec::{self, Spec};
use crate::words::Word;

/// How long the answer for a program in the `PATH` holds, so one installed meanwhile counts soon.
const PROGRAM_CHECKED_FOR: Duration = Duration::from_secs(5);

/// What a stretch of the command line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    /// The program, builtin, alias or function a command runs.
    Command,
    /// A command the shell cannot run: nothing it knows and no program in its `PATH` has the name.
    Unknown,
    /// A subcommand the command's spec knows.
    Subcommand,
    /// An option, such as `-l` or `--all`.
    Option,
    /// Any other argument.
    Argument,
    /// A variable or an assignment, such as `$HOME` or `FOO=1`.
    Variable,
    /// What joins commands or redirects them, such as `|`, `&&`, `;` and `>`.
    Operator,
}

/// A stretch of the command line, in bytes, and what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    /// Where it lies in the line.
    pub range: Range<usize>,
    /// What it is.
    pub part: Part,
}

/// The parts of a command line, first to last.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Highlight {
    /// The stretches that are words or operators; spaces belong to none.
    pub spans: Vec<Span>,
    /// The spec the subcommands needed but that is not parsed yet; `warm_spec` parses it for the next time.
    pub missing_spec: Option<String>,
}

#[derive(Debug)]
enum Token {
    Word(Word, Range<usize>),
    Separator(Range<usize>),
    Redirect(Range<usize>),
}

/// The parts of `line`; `known` tells whether the shell can run a command's name, or `None` where it cannot say.
pub fn highlight(line: &str, known: &dyn Fn(&str) -> Option<bool>) -> Highlight {
    let mut out = Highlight::default();
    let mut words: Vec<(Word, Range<usize>)> = Vec::new();
    let mut target = false;
    for token in tokens(line) {
        match token {
            Token::Separator(range) => {
                command(&words, known, &mut out);
                words.clear();
                out.spans.push(Span {
                    range,
                    part: Part::Operator,
                });
            }
            Token::Redirect(range) => {
                out.spans.push(Span {
                    range,
                    part: Part::Operator,
                });
                target = true;
            }
            Token::Word(word, range) if target => {
                target = false;
                let part = if word.text.starts_with('$') {
                    Part::Variable
                } else {
                    Part::Argument
                };
                out.spans.push(Span { range, part });
            }
            Token::Word(word, range) => words.push((word, range)),
        }
    }
    command(&words, known, &mut out);
    out.spans.sort_by_key(|span| span.range.start);
    out
}

/// The parts of one command's words: assignments first, then the command and what its spec makes of the rest.
fn command(words: &[(Word, Range<usize>)], known: &dyn Fn(&str) -> Option<bool>, out: &mut Highlight) {
    let mut at = 0;
    while let Some((word, range)) = words.get(at).filter(|(word, _)| is_assignment(&word.text)) {
        let _ = word;
        out.spans.push(Span {
            range: range.clone(),
            part: Part::Variable,
        });
        at += 1;
    }
    let Some((name, range)) = words.get(at) else { return };
    let checkable = !name.text.is_empty() && !name.text.contains(['$', '`', '*', '?', '[', '{']);
    let part = if checkable && known(&name.text) == Some(false) {
        Part::Unknown
    } else {
        Part::Command
    };
    out.spans.push(Span {
        range: range.clone(),
        part,
    });
    let lookup = |name: &str, missing: &mut Option<String>| -> Option<Arc<Spec>> {
        match spec::cached(name) {
            Some(found) => found,
            None if spec::exists(name) => {
                *missing = Some(name.to_string());
                None
            }
            None => None,
        }
    };
    let mut state = lookup(&name.text, &mut out.missing_spec).map(|root| State::new(&root));
    let mut after_dashes = false;
    for (index, (word, range)) in words.iter().enumerate().skip(at + 1) {
        let text = word.text.as_str();
        let option = !after_dashes && text.len() > 1 && text.starts_with('-');
        let mut part = if option { Part::Option } else { Part::Argument };
        if let Some(state) = state.as_mut() {
            let value = state.wants_value();
            let entered = state.entered();
            if state.take(word, &lookup, &mut out.missing_spec) == Some(true) {
                return command(&words[index..], known, out);
            }
            if value {
                part = Part::Argument;
            } else if state.entered() > entered {
                part = Part::Subcommand;
            }
        }
        if text == "--" {
            after_dashes = true;
        }
        if text.starts_with('$') {
            part = Part::Variable;
        }
        out.spans.push(Span {
            range: range.clone(),
            part,
        });
    }
}

/// `NAME=value` or `NAME+=value` before a command.
fn is_assignment(text: &str) -> bool {
    let Some((name, _)) = text.split_once('=') else {
        return false;
    };
    let name = name.strip_suffix('+').unwrap_or(name);
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The words, separators and redirections of `line` as the shell reads them: quotes, backslashes, `$(…)`, `${…}`
/// and backquotes keep what they hold in one word.
fn tokens(line: &str) -> Vec<Token> {
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let byte = |index: usize| chars.get(index).map_or(line.len(), |(at, _)| *at);
    let char_at = |index: usize| chars.get(index).map(|(_, c)| *c);
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(c) = char_at(i) {
        let start = i;
        match c {
            ' ' | '\t' => i += 1,
            '\n' | ';' | '|' | '&' | '(' | ')' => {
                i += 1;
                if c == '&' && char_at(i) == Some('>') {
                    while matches!(char_at(i), Some('>')) {
                        i += 1;
                    }
                    out.push(Token::Redirect(byte(start)..byte(i)));
                    continue;
                }
                if matches!(
                    (c, char_at(i)),
                    ('|', Some('|' | '&')) | ('&', Some('&')) | (';', Some(';'))
                ) {
                    i += 1;
                }
                out.push(Token::Separator(byte(start)..byte(i)));
            }
            '<' | '>' => {
                i = redirection_end(&char_at, i);
                out.push(Token::Redirect(byte(start)..byte(i)));
            }
            '0'..='9' if fd_redirection(&char_at, i) => {
                while matches!(char_at(i), Some('0'..='9')) {
                    i += 1;
                }
                i = redirection_end(&char_at, i);
                out.push(Token::Redirect(byte(start)..byte(i)));
            }
            _ => {
                let mut text = String::new();
                i = word_end(&char_at, i, &mut text);
                out.push(Token::Word(
                    Word {
                        text,
                        start: byte(start),
                    },
                    byte(start)..byte(i),
                ));
            }
        }
    }
    out
}

/// Digits that name a file descriptor right before `<` or `>`, as in `2>`.
fn fd_redirection(char_at: &dyn Fn(usize) -> Option<char>, mut i: usize) -> bool {
    while matches!(char_at(i), Some('0'..='9')) {
        i += 1;
    }
    matches!(char_at(i), Some('<' | '>'))
}

/// Where the redirection operator from `i` ends: `>`, `>>`, `<<<`, `>|`, `>&2`, `<&-`.
fn redirection_end(char_at: &dyn Fn(usize) -> Option<char>, mut i: usize) -> usize {
    while matches!(char_at(i), Some('<' | '>')) {
        i += 1;
    }
    match char_at(i) {
        Some('|') => i + 1,
        Some('&') => {
            i += 1;
            while matches!(char_at(i), Some('0'..='9' | '-')) {
                i += 1;
            }
            i
        }
        _ => i,
    }
}

/// Where the word from `i` ends, with its text as the shell sees it, quotes and backslashes taken out, in `text`.
fn word_end(char_at: &dyn Fn(usize) -> Option<char>, mut i: usize, text: &mut String) -> usize {
    while let Some(c) = char_at(i) {
        match c {
            ' ' | '\t' | '\n' | ';' | '|' | '&' | '<' | '>' | '(' | ')' => break,
            '\\' => {
                i += 1;
                if let Some(next) = char_at(i) {
                    text.push(next);
                    i += 1;
                }
            }
            '\'' => {
                i += 1;
                while let Some(inner) = char_at(i) {
                    i += 1;
                    if inner == '\'' {
                        break;
                    }
                    text.push(inner);
                }
            }
            '"' => {
                i += 1;
                while let Some(inner) = char_at(i) {
                    i += 1;
                    match inner {
                        '"' => break,
                        '\\' => {
                            if let Some(next) = char_at(i) {
                                text.push(next);
                                i += 1;
                            }
                        }
                        _ => text.push(inner),
                    }
                }
            }
            '$' if matches!(char_at(i + 1), Some('(' | '{')) => {
                let (open, close) = if char_at(i + 1) == Some('(') {
                    ('(', ')')
                } else {
                    ('{', '}')
                };
                text.push('$');
                i += 1;
                let mut depth = 0usize;
                while let Some(inner) = char_at(i) {
                    text.push(inner);
                    i += 1;
                    if inner == open {
                        depth += 1;
                    } else if inner == close {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                }
            }
            '`' => {
                text.push('`');
                i += 1;
                while let Some(inner) = char_at(i) {
                    text.push(inner);
                    i += 1;
                    if inner == '`' {
                        break;
                    }
                }
            }
            _ => {
                text.push(c);
                i += 1;
            }
        }
    }
    i
}

/// What a shell runs by name, as its integration reported: aliases, functions, builtins and reserved words, and the
/// programs in its `PATH`. A program is only looked for, never started (E16).
#[derive(Debug, Default)]
pub struct Runnable {
    names: HashSet<String>,
    path: Vec<PathBuf>,
    programs: RefCell<HashMap<String, (bool, Instant)>>,
}

impl Runnable {
    /// The shell's `names`, separated by white space, and its `PATH`.
    pub fn new(names: &str, path: &str) -> Runnable {
        Runnable {
            names: names.split_whitespace().map(str::to_string).collect(),
            path: path.split(':').map(PathBuf::from).collect(),
            programs: RefCell::new(HashMap::new()),
        }
    }

    /// Whether the shell runs `word` as a command in `folder`: a name it reported, a program in its `PATH`, or a path
    /// to a program or a folder, which zsh's `AUTO_CD` enters.
    pub fn knows(&self, word: &str, folder: &Path, home: &Path) -> bool {
        if self.names.contains(word) {
            return true;
        }
        if word.contains('/') {
            let path = match word.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => folder.join(word),
            };
            return path.is_dir() || crate::sources::is_executable(&path);
        }
        let now = Instant::now();
        if let Some((found, checked)) = self.programs.borrow().get(word)
            && now.duration_since(*checked) < PROGRAM_CHECKED_FOR
        {
            return *found;
        }
        let found = self.path.iter().any(|dir| {
            let dir = if dir.as_os_str().is_empty() {
                folder.to_path_buf()
            } else {
                folder.join(dir)
            };
            crate::sources::is_executable(&dir.join(word))
        });
        self.programs.borrow_mut().insert(word.to_string(), (found, now));
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts<'l>(line: &'l str, known: &dyn Fn(&str) -> Option<bool>) -> Vec<(&'l str, Part)> {
        highlight(line, known)
            .spans
            .into_iter()
            .map(|span| (&line[span.range], span.part))
            .collect()
    }

    #[test]
    fn each_part_of_a_command_line_has_its_kind() {
        spec::load("git");
        let line = "git commit -m \"a b\" | grep -v x && FOO=1 ls $HOME 2>/dev/null; echo $(date +%s)";
        assert_eq!(
            parts(line, &|_| Some(true)),
            [
                ("git", Part::Command),
                ("commit", Part::Subcommand),
                ("-m", Part::Option),
                ("\"a b\"", Part::Argument),
                ("|", Part::Operator),
                ("grep", Part::Command),
                ("-v", Part::Option),
                ("x", Part::Argument),
                ("&&", Part::Operator),
                ("FOO=1", Part::Variable),
                ("ls", Part::Command),
                ("$HOME", Part::Variable),
                ("2>", Part::Operator),
                ("/dev/null", Part::Argument),
                (";", Part::Operator),
                ("echo", Part::Command),
                ("$(date +%s)", Part::Variable),
            ]
        );
    }

    #[test]
    fn a_command_after_sudo_has_its_own_parts_and_an_unknown_one_is_marked() {
        spec::load("sudo");
        spec::load("git");
        let known = |name: &str| Some(name != "gti");
        assert_eq!(
            parts("sudo git status", &known),
            [
                ("sudo", Part::Command),
                ("git", Part::Command),
                ("status", Part::Subcommand),
            ]
        );
        assert_eq!(parts("gti status", &known)[0], ("gti", Part::Unknown));
        assert_eq!(
            parts("$EDITOR x", &|_| Some(false))[0],
            ("$EDITOR", Part::Command),
            "cannot tell"
        );
        assert_eq!(parts("gti", &|_| None)[0], ("gti", Part::Command), "nothing reported");
        assert_eq!(parts("ls -- -x", &known)[2], ("-x", Part::Argument), "after --");
    }

    #[test]
    fn the_shell_runs_its_names_its_programs_and_paths() {
        let dir = term_base::testing::scratch("suggest-runnable");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let tool = bin.join("tool");
        std::fs::write(&tool, "").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runnable = Runnable::new("ll gst", &bin.to_string_lossy());
        assert!(runnable.knows("ll", &dir, &dir));
        assert!(runnable.knows("tool", &dir, &dir));
        assert!(!runnable.knows("nosuchtool", &dir, &dir));
        assert!(runnable.knows("./bin/tool", &dir, &dir));
        assert!(runnable.knows("bin/", &dir, &dir), "a folder");
        assert!(!runnable.knows("./missing", &dir, &dir));
    }
}
