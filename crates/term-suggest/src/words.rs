//! The words of the command being typed, as the shell will split them: quotes and backslashes keep spaces in a word,
//! and `|`, `;`, `&&`, `||` and `(` start a new command.

/// A word of the command line and where it starts, in bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Word {
    pub(crate) text: String,
    pub(crate) start: usize,
}

/// The words of the last command in `line`; the last word is the one being typed, empty after a space.
pub(crate) fn last_command(line: &str) -> Vec<Word> {
    let mut words = Vec::new();
    let mut current: Option<Word> = None;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if escaped {
            escaped = false;
            push(&mut current, at, c);
            continue;
        }
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') => escaped = true,
            (Some(_), c) => push(&mut current, at, c),
            (None, '\\') => {
                escaped = true;
                current.get_or_insert(Word {
                    text: String::new(),
                    start: at,
                });
            }
            (None, '"' | '\'') => {
                quote = Some(c);
                current.get_or_insert(Word {
                    text: String::new(),
                    start: at,
                });
            }
            (None, ' ' | '\t' | '\n') => words.extend(current.take()),
            (None, '|' | ';' | '(' | '&') => {
                words.clear();
                current = None;
                if matches!(chars.peek(), Some((_, '|' | '&'))) {
                    chars.next();
                }
            }
            (None, c) => push(&mut current, at, c),
        }
    }
    words.push(current.unwrap_or(Word {
        text: String::new(),
        start: line.len(),
    }));
    words
}

fn push(current: &mut Option<Word>, at: usize, c: char) {
    current
        .get_or_insert(Word {
            text: String::new(),
            start: at,
        })
        .text
        .push(c);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(line: &str) -> Vec<String> {
        last_command(line).into_iter().map(|w| w.text).collect()
    }

    #[test]
    fn words_split_like_the_shell() {
        assert_eq!(texts("git commit -m \"a b\" "), ["git", "commit", "-m", "a b", ""]);
        assert_eq!(texts("ls | gr"), ["gr"]);
        assert_eq!(texts("make && git ch"), ["git", "ch"]);
        assert_eq!(texts("cd my\\ dir"), ["cd", "my dir"]);
        assert_eq!(texts(""), [""]);
        let words = last_command("git co");
        assert_eq!(words[1].start, 4);
    }
}
