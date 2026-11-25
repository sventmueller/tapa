//! The name git gives a commit.

use std::borrow::Borrow;
use std::ffi::OsStr;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A commit's full hash, as git prints it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommitId(String);

impl CommitId {
    /// `hash` as a commit's name if it is a full one: 40 hexadecimal digits, or 64 in a repository of SHA-256 objects.
    pub fn parse(hash: &str) -> Option<CommitId> {
        let full = matches!(hash.len(), 40 | 64) && hash.bytes().all(|b| b.is_ascii_hexdigit());
        full.then(|| CommitId(hash.to_ascii_lowercase()))
    }

    /// The hash git printed for a commit.
    pub(crate) fn from_git(hash: impl Into<String>) -> CommitId {
        CommitId(hash.into())
    }

    /// The hash.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The first eight digits, as people tell commits apart.
    pub fn short(&self) -> &str {
        self.0.get(..8).unwrap_or(&self.0)
    }
}

impl fmt::Display for CommitId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for CommitId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl AsRef<OsStr> for CommitId {
    fn as_ref(&self) -> &OsStr {
        OsStr::new(&self.0)
    }
}

impl Borrow<str> for CommitId {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

    #[test]
    fn only_full_hashes_name_a_commit() {
        assert_eq!(CommitId::parse(HASH).map(|c| c.to_string()).as_deref(), Some(HASH));
        assert_eq!(
            CommitId::parse(&HASH.to_uppercase()).as_ref().map(CommitId::as_str),
            Some(HASH)
        );
        assert!(CommitId::parse(&"a".repeat(64)).is_some());
        for wrong in ["", "4b825dc", "HEAD", &"g".repeat(40), &format!("{HASH}0")] {
            assert_eq!(CommitId::parse(wrong), None, "{wrong}");
        }
        assert_eq!(CommitId::parse(HASH).unwrap().short(), "4b825dc6");
    }

    #[test]
    fn a_commit_id_is_stored_as_its_hash() {
        let id = CommitId::from_git(HASH);
        let stored = serde_json::to_string(&id).unwrap();
        assert_eq!(stored, format!("\"{HASH}\""));
        assert_eq!(serde_json::from_str::<CommitId>(&stored).unwrap(), id);
    }
}
