//! Folder and file names for the word being typed, relative to the session's folder or the home folder.

use std::path::Path;

/// The names that complete `word` in `folder`, sorted; a folder's name ends in `/`. Hidden files only when the word
/// asks for them with a leading dot.
pub fn complete_path(word: &str, folder: &Path, home: &Path) -> Vec<String> {
    let expanded = match word.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", home.display()),
        None => word.to_string(),
    };
    let (dir_part, name_part) = match expanded.rfind('/') {
        Some(at) => (&expanded[..=at], &expanded[at + 1..]),
        None => ("", expanded.as_str()),
    };
    let dir = if dir_part.starts_with('/') {
        Path::new(dir_part).to_path_buf()
    } else {
        folder.join(dir_part)
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let typed_prefix = &word[..word.len() - name_part.len().min(word.len())];
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(name_part) || (name.starts_with('.') && !name_part.starts_with('.')) {
                return None;
            }
            let slash = if entry.path().is_dir() { "/" } else { "" };
            Some(format!("{typed_prefix}{name}{slash}"))
        })
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_complete_in_the_folder_with_folders_marked() {
        let dir = term_base::testing::scratch("suggest-paths");
        std::fs::create_dir_all(dir.join("src/bin")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        std::fs::write(dir.join(".hidden"), "").unwrap();
        assert_eq!(complete_path("sr", &dir, &dir), ["src/"]);
        assert_eq!(complete_path("src/", &dir, &dir), ["src/bin/", "src/main.rs"]);
        assert_eq!(complete_path("", &dir, &dir), ["src/"]);
        assert_eq!(complete_path(".h", &dir, &dir), [".hidden"]);
        assert_eq!(complete_path("~/sr", &dir, &dir), ["~/src/"]);
    }
}
