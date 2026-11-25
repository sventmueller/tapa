//! The files below a workspace's base path that match what the user typed after `@` in an agent's composer: a walk
//! that leaves out version control, dependencies and build output, and stops after a bound.

use std::path::Path;

use term_ui_agent::Hit;

/// Folders whose files nobody means after `@`.
const SKIPPED: [&str; 4] = [".git", "node_modules", "target", ".build"];
/// The most entries one walk looks at, so a home folder as base path does not take long.
const MOST_SEEN: usize = 50_000;

/// Up to `limit` files below `base` whose path relative to it contains `query`, case aside, those whose name does
/// first.
pub(crate) fn matching(base: &Path, query: &str, limit: usize) -> Vec<Hit> {
    let query = query.to_lowercase();
    let mut named = Vec::new();
    let mut within = Vec::new();
    let mut folders = vec![base.to_path_buf()];
    let mut seen = 0;
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries.into_iter().rev() {
            seen += 1;
            if seen > MOST_SEEN || named.len() >= limit {
                return finish(named, within, limit);
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                if !SKIPPED.contains(&name.as_str()) {
                    folders.push(entry.path());
                }
                continue;
            }
            let Ok(relative) = entry.path().strip_prefix(base).map(Path::to_path_buf) else {
                continue;
            };
            let relative = relative.to_string_lossy().into_owned();
            if name.to_lowercase().contains(&query) {
                named.push(Hit { path: relative });
            } else if within.len() < limit && relative.to_lowercase().contains(&query) {
                within.push(Hit { path: relative });
            }
        }
    }
    finish(named, within, limit)
}

fn finish(mut named: Vec<Hit>, within: Vec<Hit>, limit: usize) -> Vec<Hit> {
    named.sort_by(|a, b| a.path.len().cmp(&b.path.len()).then_with(|| a.path.cmp(&b.path)));
    named.extend(within);
    named.truncate(limit);
    named
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_walk_skips_build_output_and_puts_names_first() {
        let dir = std::env::temp_dir().join(format!("tapa-files-{}", std::process::id()));
        for file in [
            "src/main.rs",
            "src/parser/main_loop.rs",
            "target/debug/main.d",
            "docs/guide.md",
            ".git/main",
        ] {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, "").unwrap();
        }
        let paths: Vec<String> = matching(&dir, "MAIN", 10).into_iter().map(|hit| hit.path).collect();
        assert_eq!(paths, ["src/main.rs", "src/parser/main_loop.rs"]);
        let paths: Vec<String> = matching(&dir, "src/", 10).into_iter().map(|hit| hit.path).collect();
        assert_eq!(paths.len(), 2);
        assert_eq!(matching(&dir, "main", 1).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
