//! How a command line reads as far as approvals need it: its commands, their words, and the git commands among them
//! that destroy work git cannot bring back (E27). Every agent kind asks before those through the same matcher. Also
//! the files a command sends its output to, where a background command's output is to be found.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// A git command that destroys work git cannot bring back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destructive {
    /// `git push --force` rewrites the remote's history.
    ForcePush,
    /// `git reset --hard` discards uncommitted changes.
    ResetHard,
    /// `git clean -f` deletes untracked files.
    Clean,
    /// `git stash drop` deletes a stash entry.
    StashDrop,
    /// `git stash clear` deletes every stash entry.
    StashClear,
}

/// How deep `sh -c` and `eval` nest before the matcher stops looking further in.
const MAX_NESTING: usize = 8;

/// How many words named `git` of one command are each read as the start of a git call, such as the second in
/// `sudo -u git git reset --hard`.
const MAX_GIT_WORDS: usize = 16;

/// The options git takes before its subcommand with their value as the next word.
const GIT_OPTIONS_WITH_VALUE: [&str; 8] = [
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--config-env",
    "--attr-source",
    "--super-prefix",
];

/// The quoting a character of a command line stands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Quote {
    /// `'…'`, where only the closing quote is special.
    Single,
    /// `"…"`, where a backslash escapes and `$(…)` and backticks still run commands.
    Double,
    /// `$'…'`, where a backslash escapes even the quote.
    AnsiC,
}

/// A command substitution opened inside double quotes.
struct Inside {
    /// The quoted command around it, which goes on after it.
    outer: String,
    /// `)` or the backtick.
    closer: char,
    /// The parentheses opened inside it and not yet closed.
    depth: usize,
}

/// The commands of a command line, split at `&&`, `||`, `;`, `|`, `&` and line breaks outside quotes; a command
/// substitution counts as a command of its own, also inside double quotes.
pub fn segments(command: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<Quote> = None;
    let mut inside: Vec<Inside> = Vec::new();
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(Quote::Double | Quote::AnsiC), '\\') => {
                current.push(c);
                current.extend(chars.next());
            }
            (Some(Quote::Single | Quote::AnsiC), '\'') | (Some(Quote::Double), '"') => {
                quote = None;
                current.push(c);
            }
            (Some(Quote::Double), '$' | '`') if c == '`' || chars.peek() == Some(&'(') => {
                if c == '$' {
                    chars.next();
                }
                inside.push(Inside {
                    outer: std::mem::take(&mut current),
                    closer: if c == '$' { ')' } else { '`' },
                    depth: 0,
                });
                quote = None;
            }
            (Some(_), c) => current.push(c),
            (None, '\'' | '"') => {
                quote = Some(if c == '"' { Quote::Double } else { Quote::Single });
                current.push(c);
            }
            (None, '$') if chars.peek() == Some(&'\'') => {
                chars.next();
                quote = Some(Quote::AnsiC);
                current.push_str("$'");
            }
            (None, '\\') => {
                current.push(c);
                current.extend(chars.next());
            }
            (None, ')' | '`') if inside.last().is_some_and(|open| open.closer == c && open.depth == 0) => {
                if let Some(open) = inside.pop() {
                    parts.push(std::mem::replace(&mut current, open.outer));
                }
                quote = Some(Quote::Double);
            }
            (None, '(' | ')') => {
                if let Some(open) = inside.last_mut().filter(|open| open.closer == ')') {
                    open.depth = if c == '(' {
                        open.depth + 1
                    } else {
                        open.depth.saturating_sub(1)
                    };
                }
                parts.push(std::mem::take(&mut current));
            }
            (None, '$') if chars.peek() == Some(&'(') => {
                chars.next();
                if let Some(open) = inside.last_mut().filter(|open| open.closer == ')') {
                    open.depth += 1;
                }
                parts.push(std::mem::take(&mut current));
            }
            (None, ';' | '|' | '&' | '\n' | '`') => parts.push(std::mem::take(&mut current)),
            (None, c) => current.push(c),
        }
    }
    parts.push(current);
    parts.extend(inside.into_iter().rev().map(|open| open.outer));
    parts
        .into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The words of one command, with quotes removed and the escapes of `"…"` and `$'…'` resolved.
pub fn words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<Quote> = None;
    let mut started = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(Quote::Single | Quote::AnsiC), '\'') | (Some(Quote::Double), '"') => quote = None,
            (Some(Quote::Double), '\\') => match chars.next() {
                Some(next @ ('"' | '\\' | '$' | '`')) => current.push(next),
                Some('\n') => {}
                next => {
                    current.push('\\');
                    current.extend(next);
                }
            },
            (Some(Quote::AnsiC), '\\') => ansi_c_escape(&mut chars, &mut current),
            (Some(_), c) => current.push(c),
            (None, '$') if matches!(chars.peek().copied(), Some('\'' | '"')) => {
                quote = Some(match chars.next() {
                    Some('\'') => Quote::AnsiC,
                    _ => Quote::Double,
                });
                started = true;
            }
            (None, '\'' | '"') => {
                quote = Some(if c == '"' { Quote::Double } else { Quote::Single });
                started = true;
            }
            (None, '\\') => {
                if let Some(next) = chars.next() {
                    current.push(next);
                }
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started || !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
                started = false;
            }
            (None, c) => {
                current.push(c);
                started = true;
            }
        }
    }
    if started || !current.is_empty() {
        words.push(current);
    }
    words
}

/// How `command` destroys work that git cannot bring back, if it does: a forced push, `reset --hard`, `clean` with
/// `-f`, or dropping or clearing the stash; also inside `sh -c '…'`, `bash -lc "…"` and `eval`.
pub fn destructive_git(command: &str) -> Option<Destructive> {
    nested(command, 0)
}

/// The files `command` sends its standard output to with `>`, `>>`, `1>` or `&>`, in the order its commands run, as
/// paths from the folder `cwd` it starts in and from the folders its `cd`s go to. Left out are files under `/dev`,
/// targets the shell would expand further, relative targets after a `cd` to a folder that cannot be told, and the file a
/// command writes from a here-document.
pub fn output_files(command: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut dir = Some(cwd.to_path_buf());
    let mut files = Vec::new();
    for part in segments(command) {
        let words = words(&part);
        if words.first().is_some_and(|w| w == "cd") {
            dir = match words.get(1) {
                None => Some(term_base::home()),
                Some(to) if to.starts_with('-') => None,
                Some(to) => place(dir.as_deref(), to),
            };
            continue;
        }
        if words.iter().any(|w| w.starts_with("<<") && !w.starts_with("<<<")) {
            continue;
        }
        files.extend(
            stdout_targets(&part)
                .iter()
                .filter_map(|target| place(dir.as_deref(), target))
                .filter(|path| !path.starts_with("/dev")),
        );
    }
    files
}

/// Where the shell word `word` leads from folder `dir`, unless it holds something the shell would expand first.
fn place(dir: Option<&Path>, word: &str) -> Option<PathBuf> {
    if word.is_empty() || word.contains(['$', '`', '*', '?', '[']) {
        return None;
    }
    match word.strip_prefix('~') {
        Some("") => Some(term_base::home()),
        Some(rest) => rest.strip_prefix('/').map(|rest| term_base::home().join(rest)),
        None if word.starts_with('/') => Some(PathBuf::from(word)),
        None => Some(dir?.join(word)),
    }
}

/// The words one command sends its standard output to, in order.
fn stdout_targets(part: &str) -> Vec<String> {
    let chars: Vec<char> = part.chars().collect();
    let mut targets = Vec::new();
    let mut quote: Option<Quote> = None;
    let mut at = 0;
    while let Some(&c) = chars.get(at) {
        at += 1;
        match (quote, c) {
            (Some(Quote::Double | Quote::AnsiC) | None, '\\') => at += 1,
            (Some(Quote::Single | Quote::AnsiC), '\'') | (Some(Quote::Double), '"') => quote = None,
            (Some(_), _) => {}
            (None, '\'') => {
                let ansi_c = at >= 2 && chars[at - 2] == '$';
                quote = Some(if ansi_c { Quote::AnsiC } else { Quote::Single });
            }
            (None, '"') => quote = Some(Quote::Double),
            (None, '>') => {
                // Digits before `>` name the descriptor only when they are a word of their own, as in `2>`.
                let digits = chars[..at - 1].iter().rev().take_while(|d| d.is_ascii_digit()).count();
                let start = at - 1 - digits;
                let descriptor = digits > 0 && (start == 0 || chars[start - 1].is_whitespace());
                let stdout = !descriptor || chars[start..at - 1] == ['1'];
                if chars.get(at) == Some(&'>') {
                    at += 1;
                }
                if stdout && chars.get(at) != Some(&'&') {
                    let rest: String = chars[at..].iter().collect();
                    targets.extend(words(&rest).into_iter().next());
                }
            }
            (None, _) => {}
        }
    }
    targets
}

/// The command line a tool call's input runs, whether one string or a list of words.
pub fn command_of(input: &Value) -> Option<String> {
    match input.get("command")? {
        Value::String(command) => Some(command.clone()),
        Value::Array(parts) => Some(parts.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")),
        _ => None,
    }
}

/// How the command a tool call's `input` runs destroys work, if it does.
pub fn destructive_input(input: &Value) -> Option<Destructive> {
    destructive_git(&command_of(input)?).or_else(|| {
        // A list is the program's arguments, which may hold quotes or operators that the joined line misreads.
        let Some(Value::Array(parts)) = input.get("command") else {
            return None;
        };
        let quoted: Vec<String> = parts
            .iter()
            .filter_map(Value::as_str)
            .map(|word| format!("'{}'", word.replace('\'', r"'\''")))
            .collect();
        destructive_git(&quoted.join(" "))
    })
}

fn nested(command: &str, depth: usize) -> Option<Destructive> {
    segments(command).iter().find_map(|part| {
        let words = words(part);
        destructive_part(&words).or_else(|| {
            if depth >= MAX_NESTING {
                return None;
            }
            inner_script(&words).and_then(|script| nested(&script, depth + 1))
        })
    })
}

/// The script a shell runs with `-c` or from a here-string, or the line `eval` runs.
fn inner_script(words: &[String]) -> Option<String> {
    if let Some(at) = words.iter().position(|w| w == "eval") {
        return Some(words[at + 1..].join(" "));
    }
    let shell = words.iter().position(|w| is_shell(w))?;
    let mut rest = words[shell + 1..].iter();
    while let Some(word) = rest.next() {
        if word.starts_with('-') && !word.starts_with("--") && word[1..].contains('c') {
            return rest.next().cloned();
        }
        if let Some(script) = word.strip_prefix("<<<") {
            return if script.is_empty() {
                rest.next().cloned()
            } else {
                Some(script.to_string())
            };
        }
        if !word.starts_with('-') && !word.starts_with('+') {
            return None;
        }
    }
    None
}

fn is_shell(word: &str) -> bool {
    let name = word.rsplit('/').next().unwrap_or(word);
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish")
}

/// Appends what the escape after a backslash inside `$'…'` stands for, as bash reads it.
fn ansi_c_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, out: &mut String) {
    let Some(c) = chars.next() else {
        out.push('\\');
        return;
    };
    let plain = match c {
        'a' => Some('\x07'),
        'b' => Some('\x08'),
        'e' | 'E' => Some('\x1b'),
        'f' => Some('\x0c'),
        'n' => Some('\n'),
        'r' => Some('\r'),
        't' => Some('\t'),
        'v' => Some('\x0b'),
        '\\' | '\'' | '"' | '?' => Some(c),
        _ => None,
    };
    if let Some(plain) = plain {
        out.push(plain);
        return;
    }
    let (radix, most) = match c {
        'x' => (16, 2),
        'u' => (16, 4),
        'U' => (16, 8),
        '0'..='7' => (8, 3),
        _ => {
            out.push('\\');
            out.push(c);
            return;
        }
    };
    let mut digits = String::new();
    if radix == 8 {
        digits.push(c);
    }
    while digits.len() < most && chars.peek().is_some_and(|d| d.is_digit(radix)) {
        digits.extend(chars.next());
    }
    match u32::from_str_radix(&digits, radix).ok().and_then(char::from_u32) {
        Some(decoded) => out.push(decoded),
        None => {
            out.push('\\');
            out.push(c);
            out.push_str(digits.get(usize::from(radix == 8)..).unwrap_or_default());
        }
    }
}

/// Whether `arg` is the long option `option` or a start of it of at least `shortest` bytes, which git takes for the
/// option when no other one starts the same.
fn long_option(arg: &str, option: &str, shortest: usize) -> bool {
    arg.len() >= shortest && option.starts_with(arg)
}

fn destructive_part(words: &[String]) -> Option<Destructive> {
    words
        .iter()
        .enumerate()
        .filter(|(_, w)| *w == "git" || w.ends_with("/git"))
        // Each candidate reads the words after it, so a line of nothing but `git` would cost the square of its length.
        .take(MAX_GIT_WORDS)
        .find_map(|(at, _)| destructive_call(&words[at + 1..]))
}

/// How the git command with the words after `git` destroys work, if it does.
fn destructive_call(words: &[String]) -> Option<Destructive> {
    let mut rest = words.iter();
    let sub = loop {
        let word = rest.next()?;
        match word.as_str() {
            w if GIT_OPTIONS_WITH_VALUE.contains(&w) => {
                rest.next();
            }
            w if w.starts_with('-') => {}
            w => break w,
        }
    };
    if !matches!(sub, "push" | "reset" | "clean" | "stash") {
        return None;
    }
    let args: Vec<&str> = rest.map(String::as_str).collect();
    let short = |flag: char| {
        args.iter()
            .any(|a| a.starts_with('-') && !a.starts_with("--") && a[1..].contains(flag))
    };
    match sub {
        "push"
            if args.iter().any(|a| {
                a.starts_with("--force") || (a.starts_with('+') && a.len() > 1) || long_option(a, "--mirror", 3)
            }) || short('f') =>
        {
            Some(Destructive::ForcePush)
        }
        "reset" if args.iter().any(|a| long_option(a, "--hard", 3)) => Some(Destructive::ResetHard),
        "clean"
            if (short('f') || args.iter().any(|a| long_option(a, "--force", 3)))
                && !short('n')
                && !args.contains(&"--dry-run") =>
        {
            Some(Destructive::Clean)
        }
        "stash" => match args.iter().find(|a| !a.starts_with('-')).copied() {
            Some("drop") => Some(Destructive::StashDrop),
            Some("clear") => Some(Destructive::StashClear),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn destructive_git_commands_are_found_in_any_spelling() {
        for (command, why) in [
            ("git push --force", Destructive::ForcePush),
            ("git push -f origin main", Destructive::ForcePush),
            ("git push --force-with-lease", Destructive::ForcePush),
            (
                "git push --force-with-lease=main:abc origin main",
                Destructive::ForcePush,
            ),
            ("git -C repo push origin +main", Destructive::ForcePush),
            ("echo ok; git push -uf origin x", Destructive::ForcePush),
            ("git push --mirror backup", Destructive::ForcePush),
            ("cd x && git reset --hard HEAD~1", Destructive::ResetHard),
            ("git -C x reset --hard", Destructive::ResetHard),
            (
                "git --git-dir=.git --work-tree=. reset --hard origin/main",
                Destructive::ResetHard,
            ),
            ("git -c core.pager=cat reset --hard", Destructive::ResetHard),
            ("git clean -fd", Destructive::Clean),
            ("git clean -fdx", Destructive::Clean),
            ("git clean -xdf", Destructive::Clean),
            ("/usr/bin/git clean --force", Destructive::Clean),
            ("git stash drop", Destructive::StashDrop),
            ("git stash drop stash@{1}", Destructive::StashDrop),
            ("git -C repo stash --quiet drop", Destructive::StashDrop),
            ("git stash clear", Destructive::StashClear),
            ("make && git stash clear", Destructive::StashClear),
            ("cargo test || git reset --hard", Destructive::ResetHard),
            ("ls | xargs git clean -f", Destructive::Clean),
            ("echo $(git reset --hard)", Destructive::ResetHard),
            ("sudo git push --force", Destructive::ForcePush),
            ("env GIT_TRACE=1 git push -f", Destructive::ForcePush),
        ] {
            assert_eq!(destructive_git(command), Some(why), "{command}");
        }
    }

    #[test]
    fn commands_inside_a_shell_or_eval_count_too() {
        for (command, why) in [
            ("sh -c 'git push -f'", Destructive::ForcePush),
            ("bash -c \"cd repo && git reset --hard\"", Destructive::ResetHard),
            ("bash -lc 'git clean -fdx'", Destructive::Clean),
            ("/bin/zsh -e -c 'git stash clear'", Destructive::StashClear),
            ("env X=1 sh -c 'sh -c \"git push --force\"'", Destructive::ForcePush),
            ("eval 'git reset --hard'", Destructive::ResetHard),
            ("ssh-agent sh -c 'git push -f' && true", Destructive::ForcePush),
        ] {
            assert_eq!(destructive_git(command), Some(why), "{command}");
        }
    }

    #[test]
    fn harmless_commands_are_not_destructive() {
        for command in [
            "git push origin main",
            "git push -u origin feature",
            "git reset --soft HEAD~1",
            "git reset HEAD file",
            "git clean -n",
            "git clean -fn",
            "git clean -f --dry-run",
            "echo 'git push --force'",
            "git log --format=%f",
            "git stash",
            "git stash push -m drop",
            "git stash list",
            "git stash pop",
            "git stash show clear",
            "sh -c 'git status'",
            "bash script.sh -c",
            "bash -c",
            "grep -c 'git reset --hard' notes.txt",
            "",
        ] {
            assert_eq!(destructive_git(command), None, "{command}");
        }
    }

    #[test]
    fn the_command_is_read_from_a_string_or_a_list_of_words() {
        assert_eq!(
            destructive_input(&json!({"command": "git push -f"})),
            Some(Destructive::ForcePush)
        );
        assert_eq!(
            destructive_input(&json!({"command": ["git", "reset", "--hard"]})),
            Some(Destructive::ResetHard)
        );
        assert_eq!(destructive_input(&json!({"file_path": "git clean -fd"})), None);
        assert_eq!(destructive_input(&json!({"command": 3})), None);
    }

    #[test]
    fn quoting_and_git_spellings_do_not_hide_a_destructive_command() {
        for (command, why) in [
            ("echo \"$(git reset --hard)\"", Destructive::ResetHard),
            ("echo \"`git reset --hard`\"", Destructive::ResetHard),
            ("echo \"a $(echo (x)) $(git push -f) b\"", Destructive::ForcePush),
            ("echo \"\\\"\"; git reset --hard", Destructive::ResetHard),
            ("echo $'\\''; git clean -fd", Destructive::Clean),
            ("git reset $'--hard'", Destructive::ResetHard),
            ("git reset $'\\x2d\\x2dhard'", Destructive::ResetHard),
            ("git reset $\"--hard\"", Destructive::ResetHard),
            ("git reset --har HEAD~1", Destructive::ResetHard),
            ("git clean --forc -d", Destructive::Clean),
            ("git push --mirr backup", Destructive::ForcePush),
            ("git --config-env user.name=NAME reset --hard", Destructive::ResetHard),
            ("git --attr-source HEAD push -f", Destructive::ForcePush),
            ("sudo -u git git reset --hard", Destructive::ResetHard),
            ("bash <<< 'git reset --hard'", Destructive::ResetHard),
            ("sh <<<\"git stash clear\"", Destructive::StashClear),
        ] {
            assert_eq!(destructive_git(command), Some(why), "{command}");
        }
        for command in [
            "echo \"\\\"; git reset --hard; \\\"\"",
            "git commit -m \"say \\\"git push -f\\\"\"",
            "bash script.sh <<< 'git reset --hard'",
            "git reset --soft",
        ] {
            assert_eq!(destructive_git(command), None, "{command}");
        }
    }

    #[test]
    fn substitutions_inside_double_quotes_are_commands_of_their_own() {
        assert_eq!(
            segments("cargo test \"$(git rev-parse HEAD)\" && ls"),
            ["git rev-parse HEAD", "cargo test \"\"", "ls"]
        );
        assert_eq!(segments("echo \"`date`\""), ["date", "echo \"\""]);
        assert_eq!(
            words("say \"a \\\"b\\\" \\x\" $'c\\td'"),
            ["say", "a \"b\" \\x", "c\td"]
        );
    }

    #[test]
    fn a_list_of_arguments_is_read_word_by_word() {
        assert_eq!(
            destructive_input(&json!({"command": ["env", "a='", "git", "reset", "--hard"]})),
            Some(Destructive::ResetHard)
        );
        assert_eq!(
            destructive_input(&json!({"command": ["/bin/zsh", "-lc", "git push --force"]})),
            Some(Destructive::ForcePush)
        );
        assert_eq!(
            destructive_input(&json!({"command": ["git", "commit", "-m", "it's fine"]})),
            None
        );
    }

    #[test]
    fn the_files_a_command_writes_its_output_to_are_found_from_its_folder() {
        let cwd = Path::new("/repo");
        let home = term_base::home();
        for (command, files) in [
            (
                "make check > /tmp/check.log 2>&1; echo \"exit $?\" >> /tmp/check.log; tail -5 /tmp/check.log",
                vec![PathBuf::from("/tmp/check.log"), PathBuf::from("/tmp/check.log")],
            ),
            (
                "make deploy >target/deploy.log 2>&1",
                vec![PathBuf::from("/repo/target/deploy.log")],
            ),
            (
                "cd /work/app && cargo test 1> out.txt",
                vec![PathBuf::from("/work/app/out.txt")],
            ),
            (
                "cd sub; cd .. ; ls &> list.txt",
                vec![PathBuf::from("/repo/sub/../list.txt")],
            ),
            (
                "(time make check) > ~/logs/check.log 2>&1",
                vec![home.join("logs/check.log")],
            ),
            ("echo \"a > b\" > 'my log.txt'", vec![PathBuf::from("/repo/my log.txt")]),
            ("run foo2>x.log", vec![PathBuf::from("/repo/x.log")]),
            (
                "cat > /tmp/b.sh <<'EOF'\n$M -json > /tmp/r/$name.json 2> /tmp/r/$name.err\nEOF\nchmod +x /tmp/b.sh; /tmp/b.sh > /tmp/b.log 2>&1",
                vec![PathBuf::from("/tmp/b.log")],
            ),
            ("cd \"$DIR\" && make > /abs.log", vec![PathBuf::from("/abs.log")]),
        ] {
            assert_eq!(output_files(command, cwd), files, "{command}");
        }
    }

    #[test]
    fn errors_devices_pipes_and_unknown_places_are_no_output_files() {
        for command in [
            "cargo build 2> err.log",
            "make > /dev/null 2>&1",
            "make 2>&1 | tee build.log",
            "echo 'x > y'",
            "make > \"$LOG\"",
            "cd \"$DIR\" && make > rel.log",
            "cd - && make > rel.log",
            "cat > script.sh <<EOF\necho hi\nEOF",
            "make >&2",
            "",
        ] {
            assert_eq!(
                output_files(command, Path::new("/repo")),
                Vec::<PathBuf>::new(),
                "{command}"
            );
        }
    }

    #[test]
    fn deep_nesting_ends() {
        let mut command = "git status".to_string();
        for _ in 0..40 {
            command = format!("eval {command}");
        }
        assert_eq!(destructive_git(&command), None);
    }
}
