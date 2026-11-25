//! The next command before anything is typed (F210) beyond the history's sequences: a fix for the command that was
//! not found, by a typo or by a Homebrew formula from the local Homebrew index, and what the project's state asks
//! for, such as installing after the lockfile changed. Nothing here touches the network or runs a program.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::SystemTime;

use crate::rank::Context;
use crate::sources::{in_path, path_folders};

/// The exit code shells give a command they did not find.
const NOT_FOUND: i32 = 127;

/// Why the next command is suggested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The previous command was not found and this is the one most like it.
    Typo,
    /// The previous command was not found and Homebrew has a formula with it.
    Install,
    /// The project's files ask for it.
    Project,
}

/// A command to run next and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Next {
    /// The command line.
    pub line: String,
    /// Why.
    pub reason: Reason,
}

/// The fix for the previous command or what the project asks for in `context`, whichever applies first.
pub fn next_without_input(context: &Context) -> Option<Next> {
    not_found_fix(context).or_else(|| project_state(&context.folder, context))
}

fn not_found_fix(context: &Context) -> Option<Next> {
    if context.exit_code != Some(NOT_FOUND) {
        return None;
    }
    let line = context.previous.as_deref()?;
    let (start, word) = command_word(line)?;
    if word.contains('/') || in_path(word, &context.folder).is_some() {
        return None;
    }
    if let Some(formula) = formula_for(word) {
        return Some(Next {
            line: format!("brew install {formula}"),
            reason: Reason::Install,
        });
    }
    let fixed = closest_command(word, &context.folder)?;
    Some(Next {
        line: format!("{}{fixed}{}", &line[..start], &line[start + word.len()..]),
        reason: Reason::Typo,
    })
}

/// The command's name in `line` after assignments such as `FOO=1`, and where it starts.
fn command_word(line: &str) -> Option<(usize, &str)> {
    let mut at = 0;
    for word in line.split(' ') {
        if !word.is_empty() && !(word.contains('=') && !word.starts_with('=')) {
            return Some((at, word));
        }
        at += word.len() + 1;
    }
    None
}

/// The program in the `PATH` or with a spec closest to `word`, at most two edits away and fewer than half its length.
fn closest_command(word: &str, folder: &Path) -> Option<String> {
    let most = (word.chars().count() / 2).clamp(1, 2);
    let mut best: Option<(usize, String)> = None;
    let mut consider = |name: &str| {
        if name == word || name.len().abs_diff(word.len()) > most {
            return;
        }
        let distance = distance(word, name);
        if distance <= most && best.as_ref().is_none_or(|(d, n)| (distance, name) < (*d, n.as_str())) {
            best = Some((distance, name.to_string()));
        }
    };
    for dir in path_folders(folder) {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                consider(&entry.file_name().to_string_lossy());
            }
        }
    }
    for name in crate::spec::names_starting("", usize::MAX) {
        consider(name);
    }
    best.map(|(_, name)| name)
}

/// The edits, swaps of neighbours included, that turn `a` into `b`.
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut rows = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in rows.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in rows[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (rows[i - 1][j] + 1)
                .min(rows[i][j - 1] + 1)
                .min(rows[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(rows[i - 2][j - 2] + 1);
            }
            rows[i][j] = best;
        }
    }
    rows[a.len()][b.len()]
}

/// Homebrew's local list of which formula installs which programs, read once: `formula:program program …` per line.
fn executables() -> &'static [(String, Vec<String>)] {
    static TABLE: OnceLock<Vec<(String, Vec<String>)>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let cache = std::env::var_os("HOMEBREW_CACHE")
            .map(PathBuf::from)
            .unwrap_or_else(|| term_base::home().join("Library/Caches/Homebrew"));
        let text = std::fs::read_to_string(cache.join("api/internal/executables.txt")).unwrap_or_default();
        parse_executables(&text)
    })
}

fn parse_executables(text: &str) -> Vec<(String, Vec<String>)> {
    text.lines()
        .filter_map(|line| {
            let (formula, programs) = line.split_once(':')?;
            Some((
                formula.to_string(),
                programs.split_whitespace().map(str::to_string).collect(),
            ))
        })
        .collect()
}

/// The formula that installs `program`, if Homebrew is here and the formula is not installed yet.
fn formula_for(program: &str) -> Option<String> {
    let prefix = ["/opt/homebrew", "/usr/local"]
        .into_iter()
        .map(Path::new)
        .find(|prefix| prefix.join("bin/brew").exists())?;
    let formula = formula_in(executables(), program)?;
    (!prefix.join("Cellar").join(&formula).exists()).then_some(formula)
}

fn formula_in(table: &[(String, Vec<String>)], program: &str) -> Option<String> {
    table
        .iter()
        .find(|(formula, _)| formula == program)
        .or_else(|| table.iter().find(|(_, programs)| programs.iter().any(|p| p == program)))
        .map(|(formula, _)| formula.clone())
}

/// Lockfiles and the command that installs what they name, with the folder or file that install leaves behind.
const INSTALLS: &[(&str, &str, &str)] = &[
    ("bun.lock", "bun install", "node_modules"),
    ("bun.lockb", "bun install", "node_modules"),
    ("pnpm-lock.yaml", "pnpm install", "node_modules"),
    ("yarn.lock", "yarn install", "node_modules"),
    ("package-lock.json", "npm install", "node_modules/.package-lock.json"),
    ("uv.lock", "uv sync", ".venv"),
    ("Gemfile.lock", "bundle install", ".bundle"),
];

/// What `folder` asks for: an install where a lockfile is newer than what the last install left.
fn project_state(folder: &Path, context: &Context) -> Option<Next> {
    let modified = |path: &Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    for (lockfile, command, installed) in INSTALLS {
        let Some(locked) = modified(&folder.join(lockfile)) else {
            continue;
        };
        let done = modified(&folder.join(installed)).unwrap_or(SystemTime::UNIX_EPOCH);
        let just_failed = context.previous.as_deref() == Some(*command) && context.exit_code.is_some_and(|c| c != 0);
        if locked > done && !just_failed {
            return Some(Next {
                line: (*command).to_string(),
                reason: Reason::Project,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(line: &str, folder: &Path) -> Context {
        Context {
            folder: folder.to_path_buf(),
            branch: None,
            previous: Some(line.to_string()),
            exit_code: Some(NOT_FOUND),
        }
    }

    #[test]
    fn a_typo_is_fixed_to_the_closest_program() {
        let dir = term_base::testing::scratch("suggest-next-typo");
        let next = not_found_fix(&failed("FOO=1 gitt status", &dir)).expect("a fix");
        assert_eq!(next.line, "FOO=1 git status");
        assert_eq!(next.reason, Reason::Typo);
        assert_eq!(not_found_fix(&failed("ls -l", &dir)), None, "ls was found");
        let mut ok = failed("gitt status", &dir);
        ok.exit_code = Some(1);
        assert_eq!(not_found_fix(&ok), None);
        assert_eq!(distance("gti", "git"), 1);
        assert_eq!(distance("kubeclt", "kubectl"), 1);
    }

    #[test]
    fn a_missing_program_is_found_in_homebrews_list() {
        let table = parse_executables("ripgrep:rg\nfd:fd\nhtop:htop\n");
        assert_eq!(formula_in(&table, "rg").as_deref(), Some("ripgrep"));
        assert_eq!(formula_in(&table, "htop").as_deref(), Some("htop"));
        assert_eq!(formula_in(&table, "nope"), None);
    }

    #[test]
    fn a_lockfile_newer_than_the_install_asks_for_an_install() {
        let dir = term_base::testing::scratch("suggest-next-project");
        let context = Context {
            folder: dir.to_path_buf(),
            ..Context::default()
        };
        assert_eq!(project_state(&dir, &context), None);
        std::fs::write(dir.join("bun.lock"), "{}").unwrap();
        let next = project_state(&dir, &context).expect("install");
        assert_eq!((next.line.as_str(), next.reason), ("bun install", Reason::Project));
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::create_dir_all(dir.join("node_modules")).unwrap();
        assert_eq!(
            project_state(&dir, &context),
            None,
            "installed after the lockfile changed"
        );
    }
}
