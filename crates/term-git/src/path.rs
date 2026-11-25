//! Paths as git prints and reads them: raw bytes, C-style quoting, and pathspecs that match only themselves.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// A path from git's bytes.
pub(crate) fn from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// A path's bytes as git reads them.
pub(crate) fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}

/// A path as git prints it, unquoted when git quoted it.
pub(crate) fn unquoted(text: &[u8]) -> Vec<u8> {
    if text.len() < 2 || text[0] != b'"' || text[text.len() - 1] != b'"' {
        return text.to_vec();
    }
    let inner = &text[1..text.len() - 1];
    let mut out = Vec::with_capacity(inner.len());
    let mut i = 0;
    while i < inner.len() {
        let byte = inner[i];
        if byte != b'\\' || i + 1 == inner.len() {
            out.push(byte);
            i += 1;
            continue;
        }
        let next = inner[i + 1];
        let escaped = match next {
            b'a' => Some(0x07),
            b'b' => Some(0x08),
            b'f' => Some(0x0c),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b't' => Some(b'\t'),
            b'v' => Some(0x0b),
            b'"' | b'\\' => Some(next),
            _ => None,
        };
        if let Some(escaped) = escaped {
            out.push(escaped);
            i += 2;
        } else if inner.len() >= i + 4 && inner[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b)) {
            let value = inner[i + 1..i + 4]
                .iter()
                .fold(0u32, |n, b| n * 8 + u32::from(b - b'0'));
            out.push(u8::try_from(value).unwrap_or(b'?'));
            i += 4;
        } else {
            out.push(byte);
            i += 1;
        }
    }
    out
}

/// `prefix` and `path` as one name in a patch header, quoted as git quotes it when the path needs it.
pub(crate) fn quoted(prefix: &str, path: &[u8]) -> Vec<u8> {
    let needs_quotes = path.iter().any(|&b| b < 0x20 || b == 0x7f || b == b'"' || b == b'\\');
    if !needs_quotes {
        let mut out = prefix.as_bytes().to_vec();
        out.extend_from_slice(path);
        return out;
    }
    let mut out = vec![b'"'];
    out.extend_from_slice(prefix.as_bytes());
    for &byte in path {
        match byte {
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b'\r' => out.extend_from_slice(b"\\r"),
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b if b < 0x20 || b == 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out.push(b'"');
    out
}

/// `paths` for `--pathspec-from-file --pathspec-file-nul`, each matching only itself even with `*` or `:` in it.
pub(crate) fn literal_pathspecs<P: AsRef<Path>>(paths: &[P]) -> Vec<u8> {
    let mut out = Vec::new();
    for path in paths {
        out.extend_from_slice(b":(literal)");
        out.extend_from_slice(bytes(path.as_ref()));
        out.push(0);
    }
    out
}

/// `path` relative to `root`; an absolute path must lie inside it, also when one of them goes through a symlink.
pub(crate) fn relative(root: &Path, path: &Path) -> Result<PathBuf> {
    if path.is_relative() {
        if path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return Err(Error::OutsideFolder {
                path: path.to_path_buf(),
            });
        }
        return Ok(path.to_path_buf());
    }
    if let Ok(inside) = path.strip_prefix(root)
        && !climbs_out(inside)
    {
        return Ok(inside.to_path_buf());
    }
    let resolved = match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => parent.canonicalize().map(|p| p.join(name)).ok(),
        _ => None,
    };
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    resolved
        .as_deref()
        .and_then(|p| p.strip_prefix(&root).ok())
        .map(Path::to_path_buf)
        .ok_or_else(|| Error::OutsideFolder {
            path: path.to_path_buf(),
        })
}

/// Whether the relative `path` leaves the folder it starts in through its `..` names, such as `a/../../b`.
fn climbs_out(path: &Path) -> bool {
    let mut depth = 0usize;
    for component in path.components() {
        match component {
            std::path::Component::ParentDir if depth == 0 => return true,
            std::path::Component::ParentDir => depth -= 1,
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir | std::path::Component::RootDir | std::path::Component::Prefix(_) => {}
        }
    }
    false
}

/// All `paths` relative to `root`.
pub(crate) fn all_relative<P: AsRef<Path>>(root: &Path, paths: &[P]) -> Result<Vec<PathBuf>> {
    paths.iter().map(|p| relative(root, p.as_ref())).collect()
}

/// The fields of NUL-terminated output; a missing final NUL still ends the last field.
pub(crate) fn nul_fields(output: &[u8]) -> impl Iterator<Item = &[u8]> {
    let output = output.strip_suffix(b"\0").unwrap_or(output);
    output.split(|&b| b == 0).filter(move |_| !output.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_paths_come_back_as_their_bytes() {
        assert_eq!(unquoted(br#""a/new\nline.txt""#), b"a/new\nline.txt");
        assert_eq!(unquoted(br#""a/quo\"te\\.txt""#), b"a/quo\"te\\.txt");
        assert_eq!(unquoted(br#""a/\303\244.txt""#), "a/ä.txt".as_bytes());
        assert_eq!(unquoted(b"a/plain name.txt"), b"a/plain name.txt");
    }

    #[test]
    fn quoting_and_unquoting_are_inverse() {
        for path in [
            &b"plain"[..],
            b"new\nline",
            b"tab\there",
            b"quo\"te",
            b"back\\slash",
            "ümlaut".as_bytes(),
            b"bell\x07",
        ] {
            let quoted = quoted("b/", path);
            let mut expected = b"b/".to_vec();
            expected.extend_from_slice(path);
            assert_eq!(unquoted(&quoted), expected, "{}", String::from_utf8_lossy(&quoted));
        }
        assert_eq!(quoted("a/", b"sp ace"), b"a/sp ace");
    }

    #[test]
    fn literal_pathspecs_end_each_path_with_a_nul() {
        assert_eq!(literal_pathspecs(&["a*b", "c"]), b":(literal)a*b\0:(literal)c\0");
    }

    #[test]
    fn relative_paths_stay_inside_the_root() {
        let root = Path::new("/repo");
        assert_eq!(relative(root, Path::new("a/b")).unwrap(), PathBuf::from("a/b"));
        assert_eq!(relative(root, Path::new("/repo/a")).unwrap(), PathBuf::from("a"));
        assert!(matches!(
            relative(root, Path::new("../x")),
            Err(Error::OutsideFolder { .. })
        ));
        assert!(matches!(
            relative(root, Path::new("/elsewhere/x")),
            Err(Error::OutsideFolder { .. })
        ));
    }

    #[test]
    fn an_absolute_path_that_climbs_out_of_the_root_is_outside() {
        let root = Path::new("/nonexistent-repo");
        assert!(matches!(
            relative(root, Path::new("/nonexistent-repo/../etc/passwd")),
            Err(Error::OutsideFolder { .. })
        ));
        assert!(matches!(
            relative(root, Path::new("/nonexistent-repo/a/../../x")),
            Err(Error::OutsideFolder { .. })
        ));
        assert_eq!(
            relative(root, Path::new("/nonexistent-repo/a/../b")).unwrap(),
            PathBuf::from("a/../b"),
            "a path that stays inside is taken as it was"
        );
    }

    #[test]
    fn nul_separated_output_splits_into_fields() {
        assert_eq!(nul_fields(b"a\0b c\0").collect::<Vec<_>>(), vec![&b"a"[..], b"b c"]);
        assert_eq!(nul_fields(b"").count(), 0);
        assert_eq!(nul_fields(b"x").collect::<Vec<_>>(), vec![&b"x"[..]]);
    }
}
