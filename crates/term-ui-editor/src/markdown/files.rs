//! Paths beside a Markdown document, as Splat's editor makes them (copied from splat-editor-state): a new file under a
//! name no file has, such as a pasted picture, and the relative link to a file.

use std::fs::OpenOptions;
use std::io::{self, ErrorKind, Write};
use std::path::{Path, PathBuf};

/// More files of one name than anyone pastes; past this the folder behaves oddly, and the write gives up.
const MOST_TRIES: usize = 10_000;

/// The name of try `n` of a new file, counting from 1: `stem.extension`, then `stem 2.extension` and on.
fn numbered_name(stem: &str, extension: &str, n: usize) -> String {
    if n <= 1 {
        format!("{stem}.{extension}")
    } else {
        format!("{stem} {n}.{extension}")
    }
}

/// Writes `bytes` to a new file in `folder` under the first name of `numbered_name` that no file has, and returns its
/// path. The file is created only if none of that name exists, so a file that appears meanwhile keeps its content; a
/// write that fails leaves no file behind.
pub(crate) fn write_new_file(folder: &Path, stem: &str, extension: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    for n in 1..=MOST_TRIES {
        let path = folder.join(numbered_name(stem, extension, n));
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        };
        let written = file.write_all(bytes);
        if written.is_err() {
            let _ = std::fs::remove_file(&path);
        }
        return written.map(|()| path);
    }
    Err(ErrorKind::AlreadyExists.into())
}

/// The link from a document in `dir` to `target`: relative where they share more than the root, else absolute.
pub(crate) fn relative_path(dir: Option<&Path>, target: &Path) -> String {
    let Some(dir) = dir else {
        return target.to_string_lossy().into_owned();
    };
    let from: Vec<_> = dir.components().collect();
    let to: Vec<_> = target.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if common <= 1 {
        return target.to_string_lossy().into_owned();
    }
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c);
    }
    out.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_name_has_no_number_and_the_next_count_from_two() {
        assert_eq!(numbered_name("Bild", "png", 1), "Bild.png");
        assert_eq!(numbered_name("Bild", "png", 2), "Bild 2.png");
        assert_eq!(numbered_name("Bild 1", "jpeg", 10), "Bild 1 10.jpeg");
    }

    #[test]
    fn paths_with_only_the_root_in_common_stay_absolute() {
        assert_eq!(
            relative_path(Some(Path::new("/a/b")), Path::new("/c/d.png")),
            "/c/d.png"
        );
        assert_eq!(
            relative_path(Some(Path::new("/a/b")), Path::new("/a/d.png")),
            "../d.png"
        );
        assert_eq!(relative_path(Some(Path::new("/a")), Path::new("/a/b/d.png")), "b/d.png");
        assert_eq!(relative_path(None, Path::new("/a/d.png")), "/a/d.png");
    }
}
