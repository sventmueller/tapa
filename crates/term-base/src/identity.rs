//! Which product the crates work for (E3): the app hands its identity over once at start, a host such as Dango its
//! own, so a terminal embedded in another app never writes into this one's data.

use std::path::PathBuf;
use std::sync::OnceLock;

/// The product the crates serve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    /// What users read, also the value of `TERM_PROGRAM`.
    pub name: String,
    /// The product's version, the value of `TERM_PROGRAM_VERSION`.
    pub version: String,
    /// The bundle identifier: the defaults domain, the keychain service and the name of the data folders.
    pub bundle_id: String,
}

impl Identity {
    /// The identity of a test or a tool that no app set one for.
    fn fallback() -> Identity {
        Identity {
            name: "term".into(),
            version: "0.0.0".into(),
            bundle_id: "local.term-crates".into(),
        }
    }
}

static IDENTITY: OnceLock<Identity> = OnceLock::new();
static HOME: OnceLock<PathBuf> = OnceLock::new();

/// Makes `identity` the product of this process; only the first call counts.
pub fn set_identity(identity: Identity) {
    let _ = IDENTITY.set(identity);
}

/// The product of this process.
pub fn identity() -> &'static Identity {
    IDENTITY.get_or_init(Identity::fallback)
}

/// Puts the data folders under `home` instead of the user's home folder, for self-tests; only the first call counts.
pub fn set_home(home: PathBuf) {
    let _ = HOME.set(home);
}

/// The user's home folder, or the one `set_home` chose.
pub fn home() -> PathBuf {
    if let Some(home) = HOME.get() {
        return home.clone();
    }
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// Settings, history and anything else that must survive: `~/Library/Application Support/<bundle id>`.
pub fn data_dir() -> PathBuf {
    home().join("Library/Application Support").join(&identity().bundle_id)
}

/// What can be rebuilt, such as shell integration scripts and pasted images: `~/Library/Caches/<bundle id>`.
pub fn cache_dir() -> PathBuf {
    home().join("Library/Caches").join(&identity().bundle_id)
}

/// Logs that stay on this Mac: `~/Library/Logs/<bundle id>`.
pub fn logs_dir() -> PathBuf {
    home().join("Library/Logs").join(&identity().bundle_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_named_after_the_bundle_id() {
        let id = &identity().bundle_id;
        assert!(data_dir().ends_with(id));
        assert!(cache_dir().ends_with(id));
        assert!(logs_dir().ends_with(id));
    }
}
