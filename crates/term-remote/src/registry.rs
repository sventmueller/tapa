//! Which folders on this Mac stand for remote workspaces, and where their stand-ins are: the agent tab asks for the
//! `PATH` its agent gets, and the start of every agent refuses a program that would run on this Mac instead (E25).

use std::path::{Path, PathBuf};
use std::sync::Mutex;

static BINS: Mutex<Vec<(PathBuf, PathBuf)>> = Mutex::new(Vec::new());

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
        set(base, None);
        assert_eq!(bin_for(base), None);
    }
}
