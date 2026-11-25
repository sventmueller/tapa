//! Takes over what builds under `term_product::FORMER_BUNDLE_ID` kept (E19): data folders, defaults and keychain
//! items, once, at the first start under the current identifier. Folders move; defaults and keychain items are
//! copied, so the former ones stay until the user removes them.

use std::path::{Path, PathBuf};

use objc2_foundation::NSUserDefaults;
use term_ui::kit::ns;

/// The folders and files named after the bundle identifier, below the home folder: the folder and the name's suffix.
const PLACES: &[(&str, &str)] = &[
    ("Library/Application Support", ""),
    ("Library/Caches", ""),
    ("Library/Logs", ""),
    ("Library/HTTPStorages", ""),
    ("Library/HTTPStorages", ".binarycookies"),
    ("Library/WebKit", ""),
    ("Library/Saved Application State", ".savedState"),
];

/// The file in the data folder that says the takeover ran.
const MARKER: &str = ".former-id-taken-over";

/// Runs the takeover unless it ran before; a self-test moves folders and defaults but leaves the keychain alone and
/// so does not count as the run.
pub(crate) fn run() {
    let current = term_base::identity().bundle_id.clone();
    let Some(former) = former_id(&current, term_product::BUNDLE_ID, term_product::FORMER_BUNDLE_ID) else {
        return;
    };
    let marker = term_base::data_dir().join(MARKER);
    if marker.exists() {
        return;
    }
    for (from, to) in moves(&term_base::home(), &former, &current) {
        if let Err(error) = take_over(&from, &to) {
            term_base::diag::report(
                "taking over a former data folder",
                format!("{}: {error}", from.display()),
            );
        }
    }
    copy_defaults(&former, &current);
    if std::env::var_os("TAPA_SELFTEST").is_some() {
        return;
    }
    match term_seal::former::copy_from(&former) {
        Ok(copied) if !copied.is_empty() => term_base::diag::report("keychain items taken over", copied.join(", ")),
        Ok(_) => {}
        Err(error) => term_base::diag::report("taking over former keychain items", &error),
    }
    let _ = std::fs::create_dir_all(term_base::data_dir());
    let _ = std::fs::write(&marker, format!("{former}\n"));
}

/// The former identifier that corresponds to `current`: the same suffix, such as `.debug`, after the former base.
fn former_id(current: &str, base: &str, former_base: &str) -> Option<String> {
    let suffix = current.strip_prefix(base)?;
    (suffix.is_empty() || suffix.starts_with('.')).then(|| format!("{former_base}{suffix}"))
}

/// Every place of the former identifier with the place of the current one, below `home`.
fn moves(home: &Path, former: &str, current: &str) -> Vec<(PathBuf, PathBuf)> {
    PLACES
        .iter()
        .map(|(folder, suffix)| {
            let folder = home.join(folder);
            (
                folder.join(format!("{former}{suffix}")),
                folder.join(format!("{current}{suffix}")),
            )
        })
        .collect()
}

/// Moves `from` to `to` when `to` does not exist; when both are folders, moves each entry `to` lacks, so what the
/// current build already made wins and the rest stays in `from`.
fn take_over(from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.exists() {
        return Ok(());
    }
    if !to.exists() {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        return std::fs::rename(from, to);
    }
    if !(from.is_dir() && to.is_dir()) {
        return Ok(());
    }
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if !target.exists() {
            std::fs::rename(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Copies the former defaults domain when the current one holds nothing yet.
fn copy_defaults(former: &str, current: &str) {
    let defaults = NSUserDefaults::standardUserDefaults();
    if defaults
        .persistentDomainForName(&ns(current))
        .is_some_and(|domain| domain.count() > 0)
    {
        return;
    }
    let Some(domain) = defaults.persistentDomainForName(&ns(former)) else {
        return;
    };
    // SAFETY: The domain is a property list dictionary, as `persistentDomainForName` returned it.
    unsafe { defaults.setPersistentDomain_forName(&domain, &ns(current)) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_former_identifier_keeps_the_suffix_of_the_build() {
        let (base, former) = ("com.example.terminal", "local.old");
        assert_eq!(former_id(base, base, former).as_deref(), Some("local.old"));
        assert_eq!(
            former_id("com.example.terminal.debug", base, former).as_deref(),
            Some("local.old.debug")
        );
        assert_eq!(former_id("com.example.terminalx", base, former), None);
        assert_eq!(former_id("example.host", base, former), None);
    }

    #[test]
    fn every_place_is_named_after_the_identifier() {
        let moves = moves(Path::new("/h"), "old", "new");
        assert_eq!(moves.len(), PLACES.len());
        assert!(moves.contains(&(
            PathBuf::from("/h/Library/Application Support/old"),
            PathBuf::from("/h/Library/Application Support/new")
        )));
        assert!(moves.contains(&(
            PathBuf::from("/h/Library/Saved Application State/old.savedState"),
            PathBuf::from("/h/Library/Saved Application State/new.savedState")
        )));
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tapa-migrate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_folder_moves_whole_when_the_current_one_is_missing() {
        let home = scratch("whole");
        let (from, to) = (home.join("Support/old"), home.join("Support/new"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::write(from.join("config.toml"), "x").unwrap();
        take_over(&from, &to).unwrap();
        assert!(!from.exists());
        assert_eq!(std::fs::read_to_string(to.join("config.toml")).unwrap(), "x");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn entries_the_current_folder_has_stay_behind() {
        let home = scratch("merge");
        let (from, to) = (home.join("old"), home.join("new"));
        std::fs::create_dir_all(&from).unwrap();
        std::fs::create_dir_all(&to).unwrap();
        std::fs::write(from.join("history.sqlite"), "old").unwrap();
        std::fs::write(from.join("config.toml"), "old").unwrap();
        std::fs::write(to.join("history.sqlite"), "new").unwrap();
        take_over(&from, &to).unwrap();
        assert_eq!(std::fs::read_to_string(to.join("history.sqlite")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(to.join("config.toml")).unwrap(), "old");
        assert_eq!(std::fs::read_to_string(from.join("history.sqlite")).unwrap(), "old");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn nothing_happens_without_a_former_folder_or_over_a_file() {
        let home = scratch("none");
        take_over(&home.join("missing"), &home.join("new")).unwrap();
        assert!(!home.join("new").exists());
        std::fs::write(home.join("old.binarycookies"), "old").unwrap();
        std::fs::write(home.join("new.binarycookies"), "new").unwrap();
        take_over(&home.join("old.binarycookies"), &home.join("new.binarycookies")).unwrap();
        assert_eq!(std::fs::read_to_string(home.join("new.binarycookies")).unwrap(), "new");
        let _ = std::fs::remove_dir_all(&home);
    }
}
