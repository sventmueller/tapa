//! Which folders on this Mac stand for remote workspaces, and where their stand-ins are: the agent tab asks for the
//! `PATH` its agent gets, and the start of every agent refuses a program that would run on this Mac instead (E25).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static BINS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());
/// The remote workspaces' folders whose connection is open and whose stand-ins are written.
static OPEN: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

/// Makes `base` a remote workspace's folder whose stand-ins are in `bin`, or, with `None`, a folder of this Mac again.
pub fn set(base: &Path, bin: Option<&Path>) {
    let Ok(mut bins) = BINS.lock() else { return };
    bins.retain(|(known, _)| known != base);
    if let Some(bin) = bin {
        bins.push((base.to_path_buf(), bin.to_path_buf()));
    }
}

/// The stand-ins' folder of the remote workspace `folder` is in, if it is in one.
pub fn bin_for(folder: &Path) -> Option<PathBuf> {
    let bins = BINS.lock().ok()?;
    bins.iter()
        .find(|(base, _)| folder.starts_with(base))
        .map(|(_, bin)| bin.clone())
}

/// Notes whether the connection of the remote workspace `base` is open, its stand-ins written for it.
pub fn set_open(base: &Path, open: bool) {
    let Ok(mut bases) = OPEN.lock() else { return };
    bases.retain(|known| known != base);
    if open {
        bases.push(base.to_path_buf());
    }
}

/// Whether `folder` is in a remote workspace whose connection is not open, so an agent there cannot start yet.
pub fn waits_for_connection(folder: &Path) -> bool {
    if bin_for(folder).is_none() {
        return false;
    }
    !OPEN
        .lock()
        .is_ok_and(|bases| bases.iter().any(|base| folder.starts_with(base)))
}

/// Whether `program` is one of the stand-ins of a remote workspace.
pub fn is_stand_in(program: &Path) -> bool {
    let Ok(bins) = BINS.lock() else { return false };
    program
        .parent()
        .is_some_and(|dir| bins.iter().any(|(_, bin)| bin == dir))
}

/// `env` for an agent working in `folder`: in a remote workspace with its stand-ins as the whole `PATH` besides the
/// system's folders, so that no program of this Mac stands in for one there.
pub fn environment(folder: &Path, mut env: Vec<(String, String)>) -> Vec<(String, String)> {
    if let Some(bin) = bin_for(folder) {
        env.retain(|(name, _)| name != "PATH");
        env.push(("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_remote_folder_has_only_its_stand_ins_on_the_path() {
        let base = Path::new("/tmp/registry-test/remote/project");
        set(base, Some(Path::new("/tmp/registry-test/remote/bin")));
        assert_eq!(
            bin_for(&base.join("sub")),
            Some(PathBuf::from("/tmp/registry-test/remote/bin"))
        );
        let env = environment(
            base,
            vec![
                ("PATH".into(), "/opt/homebrew/bin".into()),
                ("HOME".into(), "/h".into()),
            ],
        );
        assert!(env.contains(&("PATH".into(), "/tmp/registry-test/remote/bin:/usr/bin:/bin".into())));
        assert!(env.contains(&("HOME".into(), "/h".into())));
        assert_eq!(environment(Path::new("/elsewhere"), env.clone()), env);
        assert!(is_stand_in(Path::new("/tmp/registry-test/remote/bin/claude")));
        assert!(!is_stand_in(Path::new("/usr/local/bin/claude")));
        assert!(waits_for_connection(&base.join("sub")));
        set_open(base, true);
        assert!(!waits_for_connection(&base.join("sub")));
        set_open(base, false);
        assert!(waits_for_connection(base));
        assert!(!waits_for_connection(Path::new("/elsewhere")));
        set(base, None);
        assert_eq!(bin_for(base), None);
    }
}
