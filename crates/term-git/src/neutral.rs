//! git in a workspace the user has not trusted (E16, E21, E22). git runs programs a repository names: hooks,
//! `core.fsmonitor`, clean and smudge filters, diff and merge drivers, `core.sshCommand`, credential helpers, pagers
//! and more. Under [`Policy::Untrusted`] every command gets settings of the command-line scope, which win over the
//! repository's own:
//!
//! - always: no hooks (`core.hooksPath=/dev/null`), no fsmonitor, no `ext::` or local-path remotes, editors that
//!   accept at once, no signing or signature checks, no automatic `gc` or maintenance, no bare repository found by
//!   walking up (`safe.bareRepository=explicit`), and `GIT_ATTR_NOSYSTEM=1`;
//! - for every key that names a program in the repository's `config`, `config.worktree`, the configuration of its
//!   submodules and every file these include (`include.path`, `includeIf.*.path`), read here without running git:
//!   the user's own system or global value of that key, or a value that runs nothing. Credential helpers are reset
//!   and the user's system and global ones added back.
//!
//! The settings go to git through `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_<n>` and `GIT_CONFIG_VALUE_<n>` rather than
//! `-c`, which cannot name a subsection holding `=`; git hands them on to the git processes it starts itself.
//! [`Policy::Trusted`] adds nothing: git runs as the user's own git would, hooks included.

mod config;

use std::ffi::OsString;
use std::io::Read;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use config::Entry;

use crate::error::{Error, Result};

/// How git may treat a workspace's repositories; the caller decides per workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Policy {
    /// The user trusts the workspace: git runs its hooks, filters and helpers.
    Trusted,
    /// The workspace is not trusted: git runs nothing the repository names.
    Untrusted,
}

/// The settings every command gets under [`Policy::Untrusted`], keyed as git keys them.
const FIXED: [(&str, &str); 13] = [
    ("core.hookspath", "/dev/null"),
    ("core.fsmonitor", "false"),
    ("protocol.ext.allow", "never"),
    // A local repository as remote runs its own hooks on a push, and git clears these settings for it.
    ("protocol.file.allow", "never"),
    ("core.editor", "true"),
    ("sequence.editor", "true"),
    ("commit.gpgsign", "false"),
    ("tag.gpgsign", "false"),
    ("push.gpgsign", "false"),
    ("log.showsignature", "false"),
    ("gc.auto", "0"),
    ("maintenance.auto", "false"),
    ("safe.barerepository", "explicit"),
];

/// How deep includes and nested submodules are followed; git itself stops including at 10.
const MAX_DEPTH: usize = 10;

/// More configuration files than any repository has a reason for.
const MAX_FILES: usize = 512;

/// The longest configuration file read.
const MAX_FILE: u64 = 16 << 20;

/// What takes the place of a key the repository set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reset {
    /// This value.
    To(&'static str),
    /// The user's own last value of the key, or this one.
    UserOr(&'static str),
    /// Nothing, then each of the user's own values; for lists such as credential helpers.
    UserList,
}

/// What replaces `section.subsection.key` from a repository, when it names a program.
fn reset(entry: &Entry) -> Option<Reset> {
    let sub = entry.subsection.is_some();
    Some(match (entry.section.as_slice(), sub, entry.key.as_slice()) {
        (b"filter", true, b"clean" | b"smudge" | b"process") => Reset::UserOr(""),
        // git runs a textconv of "" and fails, where `cat` changes nothing.
        (b"diff", true, b"textconv") => Reset::UserOr("cat"),
        (b"diff", true, b"command") | (b"diff", false, b"external") => Reset::UserOr(""),
        (b"merge", true, b"driver") => Reset::UserOr("false"),
        (b"core", false, b"sshcommand") => Reset::UserOr("ssh"),
        (b"core", false, b"askpass") => Reset::UserOr(""),
        (b"core", false, b"alternaterefscommand" | b"pager") => Reset::To(""),
        (b"pager", false, _) => Reset::To("false"),
        (b"credential", _, b"helper") => Reset::UserList,
        (b"gpg", _, b"program") | (b"gpg", true, b"defaultkeycommand") => Reset::UserOr(""),
        (b"remote", true, b"uploadpack") => Reset::UserOr("git-upload-pack"),
        (b"remote", true, b"receivepack") => Reset::UserOr("git-receive-pack"),
        (b"uploadpack", false, b"packobjectshook") => Reset::To(""),
        _ => return None,
    })
}

/// The user's system and global configuration, as `git config -z --list` printed it, in git's order.
#[derive(Debug, Clone, Default)]
pub(crate) struct UserConfig {
    entries: Vec<(Vec<u8>, Vec<u8>)>,
}

impl UserConfig {
    /// The entries of each listing that could be read, system first.
    pub(crate) fn parse(listings: &[Option<Vec<u8>>]) -> UserConfig {
        let entries = listings
            .iter()
            .flatten()
            .flat_map(|listing| listing.split(|&byte| byte == 0))
            .filter(|record| !record.is_empty())
            .map(|record| match record.iter().position(|&byte| byte == b'\n') {
                Some(end) => (record[..end].to_vec(), record[end + 1..].to_vec()),
                None => (record.to_vec(), b"true".to_vec()),
            })
            .collect();
        UserConfig { entries }
    }

    fn values<'a>(&'a self, name: &'a [u8]) -> impl Iterator<Item = &'a [u8]> + 'a {
        self.entries
            .iter()
            .filter(move |(key, _)| key.as_slice() == name)
            .map(|(_, value)| value.as_slice())
    }
}

/// The settings that hold whatever the repository configures.
pub(crate) fn fixed_environment() -> Vec<(OsString, OsString)> {
    encoded(&fixed())
}

fn fixed() -> Vec<(Vec<u8>, Vec<u8>)> {
    FIXED
        .iter()
        .map(|(key, value)| (key.as_bytes().to_vec(), value.as_bytes().to_vec()))
        .collect()
}

/// The environment for a git started in `dir` under [`Policy::Untrusted`]. `environment` is what the git gets
/// besides; `users` reads the user's own configuration in a folder, and is only asked when a reset needs it.
pub(crate) fn environment(
    dir: &Path,
    environment: &[(OsString, OsString)],
    users: impl FnOnce(&Path) -> UserConfig,
) -> Result<Vec<(OsString, OsString)>> {
    let home = environment
        .iter()
        .find(|(name, _)| name == "HOME")
        .map(|(_, value)| PathBuf::from(value));
    let found = repository_entries(dir, home.as_deref())?;
    let mut settings = fixed();
    let mut resets: Vec<(Vec<u8>, Reset, &Entry)> = Vec::new();
    for entry in &found {
        let name = entry.name();
        if let Some(how) = reset(entry)
            && !resets.iter().any(|(seen, ..)| *seen == name)
        {
            resets.push((name, how, entry));
        }
    }
    let needs_user = resets.iter().any(|(_, how, _)| !matches!(how, Reset::To(_)));
    let user = if needs_user { users(dir) } else { UserConfig::default() };
    for (name, how, entry) in &resets {
        match how {
            Reset::To(value) => settings.push((name.clone(), value.as_bytes().to_vec())),
            Reset::UserOr(value) => {
                let own = user.values(name).last().unwrap_or(value.as_bytes());
                settings.push((name.clone(), own.to_vec()));
            }
            Reset::UserList => {
                settings.push((name.clone(), Vec::new()));
                settings.extend(user.values(name).map(|own| (name.clone(), own.to_vec())));
            }
        }
        if entry.section == b"filter" {
            let required = Entry {
                key: b"required".to_vec(),
                ..Entry::clone(entry)
            }
            .name();
            if !settings.iter().any(|(seen, _)| *seen == required) {
                let own = user.values(&required).last().unwrap_or(b"false").to_vec();
                settings.push((required, own));
            }
        }
    }
    let mut env = encoded(&settings);
    // `GIT_PROXY_COMMAND` wins over `core.gitProxy`, and an empty one means none; git takes the first match of that
    // key, so a later setting could not override the repository's.
    let proxied = found
        .iter()
        .any(|entry| entry.section == b"core" && entry.subsection.is_none() && entry.key == b"gitproxy");
    let users_proxy = environment
        .iter()
        .any(|(name, value)| name == "GIT_PROXY_COMMAND" && !value.is_empty());
    if proxied && !users_proxy {
        env.push(("GIT_PROXY_COMMAND".into(), OsString::new()));
    }
    Ok(env)
}

/// `settings` as git's environment of command-line configuration.
fn encoded(settings: &[(Vec<u8>, Vec<u8>)]) -> Vec<(OsString, OsString)> {
    let mut env = vec![
        ("GIT_ATTR_NOSYSTEM".into(), "1".into()),
        ("GIT_CONFIG_COUNT".into(), settings.len().to_string().into()),
    ];
    for (index, (key, value)) in settings.iter().enumerate() {
        env.push((
            format!("GIT_CONFIG_KEY_{index}").into(),
            OsString::from_vec(key.clone()),
        ));
        env.push((
            format!("GIT_CONFIG_VALUE_{index}").into(),
            OsString::from_vec(value.clone()),
        ));
    }
    env
}

/// The git folder and shared git folder of the repository git finds from `dir`, and its top folder.
fn locate(dir: &Path) -> Option<(PathBuf, PathBuf, PathBuf)> {
    let dir = dir.canonicalize().ok()?;
    for root in dir.ancestors() {
        let dot_git = root.join(".git");
        let Ok(meta) = std::fs::symlink_metadata(&dot_git) else {
            continue;
        };
        let git_dir = if meta.is_file() {
            let text = std::fs::read_to_string(&dot_git).ok()?;
            let target = text.lines().next()?.strip_prefix("gitdir:")?.trim();
            root.join(target)
        } else {
            dot_git
        };
        let common = match std::fs::read_to_string(git_dir.join("commondir")) {
            Ok(text) => git_dir.join(text.trim_end_matches(['\n', '\r'])),
            Err(_) => git_dir.clone(),
        };
        return Some((git_dir, common, root.to_path_buf()));
    }
    None
}

/// Every entry of the configuration git reads for the repository found from `dir`, and of its submodules.
fn repository_entries(dir: &Path, home: Option<&Path>) -> Result<Vec<Entry>> {
    let Some((git_dir, common, root)) = locate(dir) else {
        return Ok(Vec::new());
    };
    let mut files = vec![
        common.join("config"),
        git_dir.join("config.worktree"),
        common.join("config.worktree"),
    ];
    submodule_configs(&common.join("modules"), 0, &mut files);
    for embedded in embedded_submodules(&root) {
        files.push(embedded.join("config"));
        submodule_configs(&embedded.join("modules"), 0, &mut files);
    }
    let mut reader = Files {
        home,
        read: Vec::new(),
        entries: Vec::new(),
    };
    for file in files {
        reader.read(&file, 0)?;
    }
    Ok(reader.entries)
}

/// The configuration of every git folder below `modules`, where git keeps the submodules it cloned.
fn submodule_configs(modules: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(listing) = std::fs::read_dir(modules) else {
        return;
    };
    for entry in listing.flatten() {
        if depth > MAX_DEPTH || files.len() > MAX_FILES {
            return;
        }
        let path = entry.path();
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        if path.join("HEAD").is_file() {
            files.push(path.join("config"));
            submodule_configs(&path.join("modules"), depth + 1, files);
        } else {
            // A submodule named `a/b` lies in `modules/a/b`.
            submodule_configs(&path, depth + 1, files);
        }
    }
}

/// The `.git` folders inside the working tree of the submodules `.gitmodules` names.
fn embedded_submodules(root: &Path) -> Vec<PathBuf> {
    let Ok(text) = std::fs::read(root.join(".gitmodules")) else {
        return Vec::new();
    };
    config::entries(&text)
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| entry.section == b"submodule" && entry.key == b"path")
        .filter_map(|entry| entry.value)
        .map(|path| root.join(Path::new(std::ffi::OsStr::from_bytes(&path))).join(".git"))
        .filter(|git| git.is_dir())
        .collect()
}

/// Configuration files with what they include.
struct Files<'a> {
    home: Option<&'a Path>,
    read: Vec<PathBuf>,
    entries: Vec<Entry>,
}

impl Files<'_> {
    fn read(&mut self, file: &Path, depth: usize) -> Result<()> {
        let refused = |why: &'static str| Error::UncheckedConfig {
            path: file.to_path_buf(),
            why,
        };
        if depth > MAX_DEPTH || self.read.len() >= MAX_FILES {
            return Err(refused("too many nested configuration files"));
        }
        // A named pipe would make an open without O_NONBLOCK wait for a writer.
        let opened = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(file);
        let handle = match opened {
            Ok(handle) => handle,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let meta = handle.metadata()?;
        if !meta.is_file() {
            return Err(refused("not a regular file"));
        }
        let mut text = Vec::new();
        handle.take(MAX_FILE + 1).read_to_end(&mut text)?;
        if meta.len() > MAX_FILE || text.len() as u64 > MAX_FILE {
            return Err(refused("configuration too large"));
        }
        self.read.push(file.to_path_buf());
        let entries = config::entries(&text).map_err(|_| refused("unreadable configuration"))?;
        for entry in entries {
            let include = entry.key == b"path"
                && ((entry.section == b"include" && entry.subsection.is_none())
                    || (entry.section == b"includeif" && entry.subsection.is_some()));
            if include && let Some(target) = entry.value.as_deref().and_then(|path| self.resolved(file, path)) {
                self.read(&target, depth + 1)?;
            }
            self.entries.push(entry);
        }
        Ok(())
    }

    /// Where an include in `file` points: `~/` is the home folder, a relative path starts beside `file`.
    fn resolved(&self, file: &Path, path: &[u8]) -> Option<PathBuf> {
        let path = Path::new(std::ffi::OsStr::from_bytes(path));
        if let Ok(below) = path.strip_prefix("~") {
            return Some(self.home?.join(below));
        }
        if path.as_os_str().is_empty() || path.starts_with("%(prefix)") {
            return None;
        }
        Some(file.parent()?.join(path))
    }
}

#[cfg(test)]
mod tests;
