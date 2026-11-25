//! The `.editorconfig` settings of a file: the files from its folder up to the one marked `root = true`, their
//! sections matched by glob as <https://spec.editorconfig.org> describes, a nearer file and a later section winning.
//! The files are only read; nothing in them runs, so they count in folders the user does not trust as well.

use std::path::{Path, PathBuf};

/// A larger `.editorconfig` is left out.
const MAX_FILE: u64 = 1 << 20;

/// Steps one glob may take against one path, so that a hostile pattern cannot hold the caller.
const MATCH_BUDGET: usize = 100_000;

/// The widest indentation or tab a file may ask for.
const MAX_WIDTH: usize = 32;

/// What a level of indentation is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentStyle {
    /// Tabs.
    Tab,
    /// Spaces.
    Space,
}

/// How wide a level of indentation is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndentSize {
    /// This many columns.
    Columns(usize),
    /// As wide as a tab.
    Tab,
}

/// The line break new lines get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndOfLine {
    /// `\n`.
    Lf,
    /// `\r\n`.
    Crlf,
    /// `\r`.
    Cr,
}

/// The encoding a file should have.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    /// ISO 8859-1.
    Latin1,
    /// UTF-8 without a byte order mark.
    Utf8,
    /// UTF-8 behind a byte order mark.
    Utf8Bom,
    /// UTF-16, big-endian.
    Utf16Be,
    /// UTF-16, little-endian.
    Utf16Le,
}

impl Charset {
    /// The value as the file writes it.
    pub fn name(self) -> &'static str {
        match self {
            Charset::Latin1 => "latin1",
            Charset::Utf8 => "utf-8",
            Charset::Utf8Bom => "utf-8-bom",
            Charset::Utf16Be => "utf-16be",
            Charset::Utf16Le => "utf-16le",
        }
    }
}

/// The properties that apply to one file; `None` where no section sets one, or one set it to `unset`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Properties {
    /// `indent_style`.
    pub indent_style: Option<IndentStyle>,
    /// `indent_size`.
    pub indent_size: Option<IndentSize>,
    /// `tab_width`.
    pub tab_width: Option<usize>,
    /// `end_of_line`.
    pub end_of_line: Option<EndOfLine>,
    /// `charset`.
    pub charset: Option<Charset>,
    /// `trim_trailing_whitespace`.
    pub trim_trailing_whitespace: Option<bool>,
    /// `insert_final_newline`.
    pub insert_final_newline: Option<bool>,
}

impl Properties {
    /// Whether no property is set.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        *self == Properties::default()
    }

    /// The columns a tab takes: `tab_width`, else a numeric `indent_size`.
    pub fn tab_columns(&self) -> Option<usize> {
        self.tab_width.or(match self.indent_size {
            Some(IndentSize::Columns(n)) => Some(n),
            _ => None,
        })
    }

    /// The columns of a level: a numeric `indent_size`, else the tab's columns when the size or the style is the tab.
    pub fn indent_columns(&self) -> Option<usize> {
        match self.indent_size {
            Some(IndentSize::Columns(n)) => Some(n),
            Some(IndentSize::Tab) => self.tab_width,
            None if self.indent_style == Some(IndentStyle::Tab) => self.tab_width,
            None => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) {
        let unset = value == "unset";
        let number = || value.parse::<usize>().ok().filter(|n| (1..=MAX_WIDTH).contains(n));
        let flag = || match value {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        };
        macro_rules! take {
            ($field:ident, $parsed:expr) => {
                if unset {
                    self.$field = None;
                } else if let Some(parsed) = $parsed {
                    self.$field = Some(parsed);
                }
            };
        }
        match key {
            "indent_style" => take!(
                indent_style,
                match value {
                    "tab" => Some(IndentStyle::Tab),
                    "space" => Some(IndentStyle::Space),
                    _ => None,
                }
            ),
            "indent_size" => take!(
                indent_size,
                if value == "tab" {
                    Some(IndentSize::Tab)
                } else {
                    number().map(IndentSize::Columns)
                }
            ),
            "tab_width" => take!(tab_width, number()),
            "end_of_line" => take!(
                end_of_line,
                match value {
                    "lf" => Some(EndOfLine::Lf),
                    "crlf" => Some(EndOfLine::Crlf),
                    "cr" => Some(EndOfLine::Cr),
                    _ => None,
                }
            ),
            "charset" => take!(
                charset,
                match value {
                    "latin1" => Some(Charset::Latin1),
                    "utf-8" => Some(Charset::Utf8),
                    "utf-8-bom" => Some(Charset::Utf8Bom),
                    "utf-16be" => Some(Charset::Utf16Be),
                    "utf-16le" => Some(Charset::Utf16Le),
                    _ => None,
                }
            ),
            "trim_trailing_whitespace" => take!(trim_trailing_whitespace, flag()),
            "insert_final_newline" => take!(insert_final_newline, flag()),
            _ => {}
        }
    }
}

/// One parsed `.editorconfig`.
#[derive(Debug, Default)]
struct File {
    root: bool,
    sections: Vec<(String, Vec<(String, String)>)>,
}

fn parse(text: &str) -> File {
    let mut file = File::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') && line.len() >= 2 {
            file.sections.push((line[1..line.len() - 1].to_string(), Vec::new()));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();
        match file.sections.last_mut() {
            Some((_, pairs)) => pairs.push((key, value.to_ascii_lowercase())),
            None if key == "root" => file.root = value.eq_ignore_ascii_case("true"),
            None => {}
        }
    }
    file
}

/// The properties the `.editorconfig` files from `path`'s folder up to the top give `path`.
pub fn for_file(path: &Path) -> Properties {
    let mut found: Vec<(PathBuf, File)> = Vec::new();
    for folder in path.ancestors().skip(1) {
        let candidate = folder.join(".editorconfig");
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_FILE {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&candidate) else {
            continue;
        };
        let file = parse(&text);
        let root = file.root;
        found.push((folder.to_path_buf(), file));
        if root {
            break;
        }
    }
    let mut properties = Properties::default();
    for (folder, file) in found.iter().rev() {
        let Ok(relative) = path.strip_prefix(folder) else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        for (glob, pairs) in &file.sections {
            if section_matches(glob, &relative) {
                for (key, value) in pairs {
                    properties.set(key, value);
                }
            }
        }
    }
    properties
}

/// Whether a section's `glob` covers the file at `relative`, a path below the `.editorconfig`'s folder with `/`
/// between names. A glob without a `/` matches the file's name in any folder; one with a `/` matches from the folder.
fn section_matches(glob: &str, relative: &str) -> bool {
    let (glob, subject) = if glob.contains('/') {
        (glob.strip_prefix('/').unwrap_or(glob), relative)
    } else {
        (glob, relative.rsplit('/').next().unwrap_or(relative))
    };
    let Some(pattern) = compile(glob) else {
        return false;
    };
    let subject: Vec<char> = subject.chars().collect();
    let mut budget = MATCH_BUDGET;
    matches(&pattern, &subject, &mut budget)
}

#[derive(Debug, Clone)]
enum Token {
    Char(char),
    /// `?`: one character but `/`.
    One,
    /// `*`: any characters but `/`.
    Star,
    /// `**`: any characters.
    Stars,
    /// `[...]`: one character in (or, negated, not in) the ranges.
    Class(bool, Vec<(char, char)>),
    /// `{a,b}`: one of the alternatives.
    Either(Vec<Vec<Token>>),
    /// `{1..9}`: a whole number in the range.
    Number(i64, i64),
}

fn compile(glob: &str) -> Option<Vec<Token>> {
    let chars: Vec<char> = glob.chars().collect();
    let mut at = 0;
    let tokens = sequence(&chars, &mut at, false)?;
    (at == chars.len()).then_some(tokens)
}

/// Tokens up to the end, or inside braces up to a `,` or `}` at this level, which is left for the caller.
fn sequence(chars: &[char], at: &mut usize, nested: bool) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    while let Some(&c) = chars.get(*at) {
        match c {
            ',' | '}' if nested => break,
            '\\' => {
                *at += 1;
                tokens.push(Token::Char(*chars.get(*at)?));
                *at += 1;
            }
            '?' => {
                tokens.push(Token::One);
                *at += 1;
            }
            '*' if chars.get(*at + 1) == Some(&'*') => {
                tokens.push(Token::Stars);
                *at += 2;
            }
            '*' => {
                tokens.push(Token::Star);
                *at += 1;
            }
            '[' => match class(chars, *at) {
                Some((token, end)) => {
                    tokens.push(token);
                    *at = end;
                }
                None => {
                    tokens.push(Token::Char('['));
                    *at += 1;
                }
            },
            '{' => {
                let (token, end) = braces(chars, *at)?;
                tokens.push(token);
                *at = end;
            }
            _ => {
                tokens.push(Token::Char(c));
                *at += 1;
            }
        }
    }
    Some(tokens)
}

/// The class that opens at `start`, and where it ends; `None` when it does not close, so the `[` is a character.
fn class(chars: &[char], start: usize) -> Option<(Token, usize)> {
    let mut at = start + 1;
    let negated = matches!(chars.get(at), Some('!' | '^'));
    if negated {
        at += 1;
    }
    let mut ranges = Vec::new();
    let mut first = true;
    loop {
        let c = *chars.get(at)?;
        if c == ']' && !first {
            return Some((Token::Class(negated, ranges), at + 1));
        }
        if c == '/' {
            return None;
        }
        first = false;
        let c = if c == '\\' {
            at += 1;
            *chars.get(at)?
        } else {
            c
        };
        if chars.get(at + 1) == Some(&'-') && chars.get(at + 2).is_some_and(|&e| e != ']') {
            ranges.push((c, chars[at + 2]));
            at += 3;
        } else {
            ranges.push((c, c));
            at += 1;
        }
    }
}

/// The braces that open at `start` and where they end: a number range, alternatives, or, with neither a `,` nor a
/// range inside, the characters themselves.
fn braces(chars: &[char], start: usize) -> Option<(Token, usize)> {
    let close = (start + 1..chars.len()).find(|&i| chars[i] == '}');
    if let Some(close) = close {
        let inner: String = chars[start + 1..close].iter().collect();
        if let Some((low, high)) = inner.split_once("..")
            && let (Ok(low), Ok(high)) = (low.parse::<i64>(), high.parse::<i64>())
        {
            return Some((Token::Number(low.min(high), low.max(high)), close + 1));
        }
    }
    let mut at = start + 1;
    let mut alternatives = vec![sequence(chars, &mut at, true)?];
    while chars.get(at) == Some(&',') {
        at += 1;
        alternatives.push(sequence(chars, &mut at, true)?);
    }
    if chars.get(at) != Some(&'}') {
        return None;
    }
    if alternatives.len() == 1 {
        let mut literal = vec![Token::Char('{')];
        literal.extend(alternatives.remove(0));
        literal.push(Token::Char('}'));
        return Some((Token::Either(vec![literal]), at + 1));
    }
    Some((Token::Either(alternatives), at + 1))
}

fn matches(pattern: &[Token], subject: &[char], budget: &mut usize) -> bool {
    if *budget == 0 {
        return false;
    }
    *budget -= 1;
    let Some((token, rest)) = pattern.split_first() else {
        return subject.is_empty();
    };
    match token {
        Token::Char(c) => subject.first() == Some(c) && matches(rest, &subject[1..], budget),
        Token::One => subject.first().is_some_and(|&c| c != '/') && matches(rest, &subject[1..], budget),
        Token::Class(negated, ranges) => {
            subject
                .first()
                .is_some_and(|&c| c != '/' && ranges.iter().any(|&(low, high)| (low..=high).contains(&c)) != *negated)
                && matches(rest, &subject[1..], budget)
        }
        Token::Star => {
            let reach = subject.iter().position(|&c| c == '/').unwrap_or(subject.len());
            (0..=reach).any(|skip| matches(rest, &subject[skip..], budget))
        }
        Token::Stars => (0..=subject.len()).any(|skip| matches(rest, &subject[skip..], budget)),
        Token::Either(alternatives) => alternatives.iter().any(|alternative| {
            let mut joined = alternative.clone();
            joined.extend_from_slice(rest);
            matches(&joined, subject, budget)
        }),
        Token::Number(low, high) => {
            let sign = usize::from(subject.first() == Some(&'-'));
            let digits = subject[sign..].iter().take_while(|c| c.is_ascii_digit()).count();
            (1..=digits).any(|length| {
                let text: String = subject[..sign + length].iter().collect();
                text.parse::<i64>().is_ok_and(|n| (*low..=*high).contains(&n))
                    && matches(rest, &subject[sign + length..], budget)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{scratch, write};

    #[test]
    fn globs_follow_the_specification() {
        assert!(section_matches("*", "src/main.rs"));
        assert!(section_matches("*.rs", "src/deep/main.rs"));
        assert!(!section_matches("*.rs", "src/main.rsx"));
        assert!(section_matches("*.{js,py}", "a/b.py"));
        assert!(!section_matches("*.{js,py}", "a/b.rb"));
        assert!(section_matches("{package.json,.travis.yml}", "package.json"));
        assert!(section_matches("Makefile", "sub/Makefile"));
        assert!(section_matches("lib/**.js", "lib/a/b/c.js"));
        assert!(
            !section_matches("lib/**.js", "src/lib/c.js"),
            "a glob with a slash starts at its folder"
        );
        assert!(section_matches("/src/*.rs", "src/main.rs"));
        assert!(
            !section_matches("src/*.rs", "src/a/main.rs"),
            "a star stays within a folder"
        );
        assert!(section_matches("file[0-9].txt", "file7.txt"));
        assert!(!section_matches("file[!0-9].txt", "file7.txt"));
        assert!(section_matches("f?.md", "fx.md"));
        assert!(section_matches("v{1..10}.txt", "v10.txt"));
        assert!(!section_matches("v{1..10}.txt", "v11.txt"));
        assert!(
            section_matches("{single}.txt", "{single}.txt"),
            "braces without a comma are characters"
        );
        assert!(section_matches("\\*.txt", "*.txt"));
        assert!(!section_matches("\\*.txt", "a.txt"));
    }

    #[test]
    fn hostile_globs_give_up_instead_of_hanging() {
        let glob = "**a**a**a**a**a**a**a**a**a**a**b";
        let subject = "a".repeat(200);
        let started = std::time::Instant::now();
        assert!(!section_matches(glob, &subject));
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn nearer_files_and_later_sections_win_up_to_the_root() {
        let top = scratch("editorconfig");
        write(&top, ".editorconfig", "[*]\nindent_style = tab\ncharset = latin1\n");
        write(
            &top,
            "project/.editorconfig",
            "# comment\nroot = true\n\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\n\
             insert_final_newline = true\n\n[*.{yml,yaml}]\nindent_size = 2\n\n[Makefile]\nindent_style = tab\n",
        );
        write(
            &top,
            "project/sub/.editorconfig",
            "[*.yml]\nINDENT_SIZE = Unset\ntrim_trailing_whitespace = TRUE\ntab_width = 99\n",
        );
        let project = top.join("project");
        let rust = for_file(&project.join("src/main.rs"));
        assert_eq!(rust.indent_style, Some(IndentStyle::Space));
        assert_eq!(rust.indent_size, Some(IndentSize::Columns(4)));
        assert_eq!(rust.end_of_line, Some(EndOfLine::Lf));
        assert_eq!(rust.insert_final_newline, Some(true));
        assert_eq!(rust.charset, None, "the root file stops the search upwards");
        assert_eq!(rust.tab_columns(), Some(4));
        assert_eq!(
            for_file(&project.join("ci.yaml")).indent_size,
            Some(IndentSize::Columns(2))
        );
        let make = for_file(&project.join("Makefile"));
        assert_eq!(make.indent_style, Some(IndentStyle::Tab));
        let nested = for_file(&project.join("sub/deploy.yml"));
        assert_eq!(nested.indent_size, None, "unset removes what a farther file set");
        assert_eq!(nested.trim_trailing_whitespace, Some(true));
        assert_eq!(nested.tab_width, None, "a width beyond the limit is left out");
        assert!(for_file(&top.join("elsewhere/x.rs")).indent_style == Some(IndentStyle::Tab));
        let _ = std::fs::remove_dir_all(top);
    }

    #[test]
    fn tab_and_indent_columns_derive_from_each_other() {
        let tabs = Properties {
            indent_style: Some(IndentStyle::Tab),
            tab_width: Some(8),
            ..Properties::default()
        };
        assert_eq!(tabs.indent_columns(), Some(8));
        let sized = Properties {
            indent_size: Some(IndentSize::Tab),
            tab_width: Some(3),
            ..Properties::default()
        };
        assert_eq!(sized.indent_columns(), Some(3));
        assert!(Properties::default().is_empty());
    }
}
