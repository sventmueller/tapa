//! The register of agents (E21): the ACP registry's entries, fetched only when the user asks and kept in a cache, and
//! the agents this build knows itself, each run from its own command on the login shell's PATH outside the workspace
//! (E16) or else from the version the app installed on the user's word.

use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use term_net::Net;

use crate::install::Installs;
use crate::sandbox::{Effect, Sandbox};

/// Where the registry publishes all its entries in one file.
pub(crate) const REGISTRY_URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The most the registry file may weigh; it is a few hundred kilobytes.
const MAX_REGISTRY_BYTES: u64 = 8 * 1024 * 1024;

/// The registry's entries as far as this Mac can use them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Register {
    /// The version of the registry's format.
    pub(crate) version: String,
    /// The agents.
    pub entries: Vec<Entry>,
}

/// One agent of the registry.
#[derive(Debug, Clone, PartialEq, Default)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Entry {
    /// The registry's id, such as `codex-acp`.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// The version the entry offers.
    pub version: String,
    pub description: String,
    /// Its source code.
    pub repository: Option<String>,
    /// Its web site.
    pub website: Option<String>,
    /// Its license, an SPDX id or `proprietary`.
    pub license: Option<String>,
    /// Who makes it.
    pub authors: Vec<String>,
    /// Its icon, a URL of a monochrome SVG.
    pub icon: Option<String>,
    /// The ways to get it on this Mac, in the registry's order.
    pub distributions: Vec<Distribution>,
    /// Ways of distribution this build does not know, by name, shown rather than guessed at.
    pub unknown: Vec<String>,
}

/// One way to get and run an agent.
#[derive(Debug, Clone, PartialEq)]
#[expect(missing_docs, reason = "the names say it")]
pub enum Distribution {
    /// An archive to download for this Mac.
    Binary {
        /// Where the archive is.
        archive: String,
        /// The program inside the unpacked archive.
        cmd: String,
        /// Its arguments.
        args: Vec<String>,
        env: Vec<(String, String)>,
        /// The archive's SHA-256, when the entry gives one.
        sha256: Option<String>,
    },
    /// A Node package.
    Npm(Package),
    /// A Python package, run with `uv`.
    Uvx(Package),
}

/// A package of a package manager, as an entry names it.
#[derive(Debug, Clone, PartialEq)]
pub struct Package {
    /// The package's name.
    pub name: String,
    /// The version the entry asks for, when it names one.
    pub version: Option<String>,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

/// Why the registry could not be read.
#[derive(Debug)]
pub enum FetchError {
    /// The network failed.
    Net(io::Error),
    /// The file is not a registry.
    Format(String),
}

impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FetchError::Net(e) => write!(f, "{e}"),
            FetchError::Format(why) => write!(f, "the registry cannot be read: {why}"),
        }
    }
}

impl std::error::Error for FetchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            FetchError::Net(error) => Some(error),
            FetchError::Format(_) => None,
        }
    }
}

/// This Mac's target as the registry names it.
pub(crate) fn target() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "darwin-aarch64"
    } else {
        "darwin-x86_64"
    }
}

/// Whether `version` names one release, like `1.2.3` or `1.2.3-beta.1`, not a range or a tag such as `latest`.
pub(crate) fn is_exact(version: &str) -> bool {
    let core = version.split(['-', '+']).next().unwrap_or_default();
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// A package spec such as `@scope/name@1.2.3` or `name==1.2.3` split into name and version.
fn split_package(spec: &str) -> (String, Option<String>) {
    if let Some((name, version)) = spec.split_once("==") {
        return (name.to_string(), Some(version.to_string()));
    }
    let at = spec.char_indices().skip(1).find(|(_, c)| *c == '@').map(|(n, _)| n);
    match at {
        Some(n) => (spec[..n].to_string(), Some(spec[n + 1..].to_string())),
        None => (spec.to_string(), None),
    }
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default()
}

fn env_of(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

fn package(value: &Value) -> Option<Package> {
    let (name, version) = split_package(value.get("package")?.as_str()?);
    Some(Package {
        name,
        version,
        args: strings(&value["args"]),
        env: env_of(&value["env"]),
    })
}

fn distributions(value: &Value) -> (Vec<Distribution>, Vec<String>) {
    let mut found = Vec::new();
    let mut unknown = Vec::new();
    let Some(map) = value.as_object() else {
        return (found, unknown);
    };
    for (kind, spec) in map {
        match kind.as_str() {
            "binary" => {
                if let Some(build) = spec.get(target())
                    && let (Some(archive), Some(cmd)) = (text(build, "archive"), text(build, "cmd"))
                {
                    found.push(Distribution::Binary {
                        archive,
                        cmd,
                        args: strings(&build["args"]),
                        env: env_of(&build["env"]),
                        sha256: text(build, "sha256").map(|s| s.to_ascii_lowercase()),
                    });
                }
            }
            "npx" => found.extend(package(spec).map(Distribution::Npm)),
            "uvx" => found.extend(package(spec).map(Distribution::Uvx)),
            other => unknown.push(other.to_string()),
        }
    }
    (found, unknown)
}

impl Entry {
    fn read(value: &Value) -> Option<Entry> {
        let (distributions, unknown) = distributions(&value["distribution"]);
        Some(Entry {
            id: text(value, "id")?,
            name: text(value, "name")?,
            version: text(value, "version").unwrap_or_default(),
            description: text(value, "description").unwrap_or_default(),
            repository: text(value, "repository"),
            website: text(value, "website"),
            license: text(value, "license"),
            authors: strings(&value["authors"]),
            icon: text(value, "icon"),
            distributions,
            unknown,
        })
    }
}

impl Register {
    /// Reads the registry's JSON; entries it cannot read are left out.
    pub fn parse(json: &str) -> Result<Register, FetchError> {
        let value: Value = serde_json::from_str(json).map_err(|e| FetchError::Format(e.to_string()))?;
        let agents = value["agents"]
            .as_array()
            .ok_or_else(|| FetchError::Format("no agents".into()))?;
        Ok(Register {
            version: text(&value, "version").unwrap_or_default(),
            entries: agents.iter().filter_map(Entry::read).collect(),
        })
    }

    /// The entry with `id`.
    pub fn entry(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Where the last fetched registry is kept.
    pub(crate) fn cache_file() -> PathBuf {
        term_base::cache_dir().join("acp-registry.json")
    }

    /// The registry as last fetched, and when; nothing before the user first asked for it.
    pub fn cached() -> Option<(Register, SystemTime)> {
        let file = Register::cache_file();
        let json = std::fs::read_to_string(&file).ok()?;
        let at = std::fs::metadata(&file).and_then(|m| m.modified()).ok()?;
        Some((Register::parse(&json).ok()?, at))
    }

    /// Fetches the registry and keeps it in the cache; only on the user's request.
    pub fn fetch(net: &dyn Net) -> Result<Register, FetchError> {
        let json = net
            .get_text(REGISTRY_URL, MAX_REGISTRY_BYTES)
            .map_err(FetchError::Net)?;
        let register = Register::parse(&json)?;
        let _ = term_base::atomic_write::write(&Register::cache_file(), json.as_bytes());
        Ok(register)
    }
}

/// The version a preset installs, with what it was checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pinned {
    /// A Node package installed with npm.
    Npm {
        /// The package.
        package: &'static str,
        /// Its version.
        version: &'static str,
        /// The package's `dist.integrity` in the npm registry, which the installed lock file must name.
        integrity: &'static str,
        /// The program the package installs.
        bin: &'static str,
        /// The program's arguments.
        args: &'static [&'static str],
        /// The lowest Node major version the package asks for, when it names one.
        node_major: Option<u32>,
    },
    /// An archive for each Mac.
    Binary {
        /// Its version.
        version: &'static str,
        /// The archive for Apple silicon and its SHA-256.
        arm64: (&'static str, &'static str),
        /// The archive for Intel and its SHA-256.
        x86_64: (&'static str, &'static str),
        /// The program inside the unpacked archive.
        cmd: &'static str,
        /// Its arguments.
        args: &'static [&'static str],
    },
}

impl Pinned {
    /// The version installed.
    pub fn version(&self) -> &'static str {
        match self {
            Pinned::Npm { version, .. } | Pinned::Binary { version, .. } => version,
        }
    }
}

/// How an agent starts in a workspace that is not trusted (E16).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Untrusted {
    /// Variables that make it skip the project's configuration, hooks and MCP servers.
    Env(&'static [(&'static str, &'static str)]),
    /// It has no switch to skip the project's configuration, so it starts only in a trusted workspace.
    Refused,
}

/// An agent this build knows: how to run it if it is already there, and which version it installs otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// The registry's id, which threads name as their agent.
    pub id: &'static str,
    /// The name shown.
    pub name: &'static str,
    /// The agent's colour as `#RRGGBB`, where it has one of its own.
    pub colour: Option<&'static str>,
    /// The agent's own command and its arguments, found on the login shell's PATH and preferred when it is there.
    pub on_path: (&'static str, &'static [&'static str]),
    /// The agent's command-line program the adapter needs beside it, when the adapter is a program of its own: the
    /// user may have the program and still lack the adapter.
    pub cli: Option<&'static str>,
    /// What this build installs.
    pub pinned: Pinned,
    /// Whether Dango's `make test-integration` verified the pinned version; until then installing it asks for
    /// confirmation like any registry entry.
    pub verified: bool,
    /// How its sandbox is switched on.
    pub sandbox: Sandbox,
    /// What that sandbox limits.
    pub effect: Effect,
    /// How it starts in a workspace that is not trusted.
    pub untrusted: Untrusted,
    /// Whether new threads are offered with it; its threads read either way.
    pub offered: bool,
}

/// The agents of the plan (E21) with the versions Dango pins, the registry's of 2026-11-09.
pub const PRESETS: &[Preset] = &[
    Preset {
        id: "codex-acp",
        name: "Codex",
        colour: None,
        on_path: ("codex-acp", &[]),
        cli: Some("codex"),
        pinned: Pinned::Npm {
            package: "@agentclientprotocol/codex-acp",
            version: "2.6.0",
            integrity: "sha512-/PBsd0H6URy4bhXhAeXqwb7RqctbXfNceRihAh6nDmn5HJ873uzoHvHxF45YmeWfgOKKRrNRLZjYsket2a7EsQ==",
            bin: "codex-acp",
            args: &[],
            node_major: None,
        },
        verified: false,
        // codex-acp has no `-s` flag; the mode it starts in carries the sandbox policy of `codex -s workspace-write`.
        sandbox: Sandbox::Env(&[("INITIAL_AGENT_MODE", "workspace-write")]),
        effect: Effect::WorkspaceWrite,
        untrusted: Untrusted::Refused,
        offered: true,
    },
    Preset {
        id: "gemini",
        name: "Gemini",
        colour: Some("#4796E3"),
        on_path: ("gemini", &["--acp"]),
        cli: None,
        pinned: Pinned::Npm {
            package: "@google/gemini-cli",
            version: "0.67.0",
            integrity: "sha512-lDJfCcYem6/6QZdwZFiGfOu1ZxJJ+j4wsr4HlmcYBfqUHTDXLKDv1MPUXYL2Ok1ygdfMve0F5KrW/ORNZRSHIg==",
            bin: "gemini",
            args: &["--acp"],
            node_major: Some(20),
        },
        verified: false,
        // On macOS the default profile, permissive-open, limits writes to the project and leaves reading and the
        // network open.
        sandbox: Sandbox::Args(&["--sandbox"]),
        effect: Effect::ProjectWritesOnly,
        untrusted: Untrusted::Refused,
        // Google ended the sign-in with a personal account for Gemini CLI and points to Antigravity instead.
        offered: false,
    },
    Preset {
        id: "antigravity-acp",
        name: "Antigravity",
        colour: None,
        on_path: ("agy", &["acp"]),
        cli: None,
        pinned: Pinned::Binary {
            version: "1.4.5",
            arm64: (
                "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.4.5-darwin-arm64.zip",
                "886fff7767d08994ca39eb02c3d01a7532c7325a94aa33aaf52d52068733284f",
            ),
            x86_64: (
                "https://dl.google.com/agy-extensions/releases/macos/agy-acp-server-1.4.5-darwin-x86_64.zip",
                "b39f3573c03b9acb9df028853f647527f1f3133f32bd77e78b796e48a205114a",
            ),
            cmd: "./agy_acp_server.par",
            args: &[],
        },
        verified: false,
        sandbox: Sandbox::Unavailable,
        effect: Effect::None,
        // `agy acp` 1.4 gates the workspace's customizations on its own folder trust, which the app cannot set.
        untrusted: Untrusted::Refused,
        offered: true,
    },
    Preset {
        id: "opencode",
        name: "OpenCode",
        colour: None,
        on_path: ("opencode", &["acp"]),
        cli: None,
        pinned: Pinned::Binary {
            version: "2.2.6",
            arm64: (
                "https://github.com/anomalyco/opencode/releases/download/v2.2.6/opencode-darwin-arm64.zip",
                "15491608f6b0bc4debe5eac35eab629a2b07ea9a1fbac68ad53bcea59fcc4b45",
            ),
            x86_64: (
                "https://github.com/anomalyco/opencode/releases/download/v2.2.6/opencode-darwin-x64.zip",
                "b2962dc445d873df56ae7d3ba7e66502b046bbc4dafa820eca18d0303a98a29c",
            ),
            cmd: "./opencode",
            args: &["acp"],
        },
        verified: false,
        sandbox: Sandbox::Unavailable,
        effect: Effect::None,
        untrusted: Untrusted::Env(&OPENCODE_UNTRUSTED),
        offered: true,
    },
    Preset {
        id: "claude-acp",
        name: "Claude (ACP)",
        colour: Some("#D97757"),
        on_path: ("claude-agent-acp", &[]),
        cli: Some("claude"),
        pinned: Pinned::Binary {
            version: "0.102.0",
            arm64: (
                "https://github.com/agentclientprotocol/claude-agent-acp/releases/download/v0.102.0/claude-agent-acp-darwin-arm64.zip",
                "3a1d50a9b04b1c1db2d5263eb9c0d3f2ba141e0d55279e60921900b85ed2bd9a",
            ),
            x86_64: (
                "https://github.com/agentclientprotocol/claude-agent-acp/releases/download/v0.102.0/claude-agent-acp-darwin-x64.zip",
                "1cfad2bccd586f7988d4ce4a309fb55ebc06103e9ac01ab8fe8fae01567a9da1",
            ),
            cmd: "./claude-agent-acp",
            args: &[],
        },
        verified: false,
        // claude-agent-acp hands `_meta.claudeCode.options` to the Agent SDK; its sandbox covers shell commands only.
        sandbox: Sandbox::SessionMeta(
            r#"{"claudeCode":{"options":{"sandbox":{"enabled":true,"failIfUnavailable":true}}}}"#,
        ),
        effect: Effect::ShellCommands,
        untrusted: Untrusted::Refused,
        offered: true,
    },
];

/// The variables OpenCode 2.4 reads to skip the project's `opencode.json`, `.opencode/`, `AGENTS.md` and `.claude/`,
/// as `term-opencode` sets them.
const OPENCODE_UNTRUSTED: [(&str, &str); 2] = [
    ("OPENCODE_DISABLE_PROJECT_CONFIG", "true"),
    ("OPENCODE_DISABLE_CLAUDE_CODE", "true"),
];

impl Preset {
    /// Where the agent's own command is on the login shell's PATH in `env`, outside the workspace at `base`.
    pub fn command(&self, env: &[(String, String)], base: &Path) -> Option<PathBuf> {
        term_process::tool::which_outside(self.on_path.0, term_process::tool::var(env, "PATH"), base)
    }

    /// Where the program `cli` names is on the login shell's PATH in `env`, outside the workspace at `base`.
    pub fn cli_on_path(&self, env: &[(String, String)], base: &Path) -> Option<PathBuf> {
        term_process::tool::which_outside(self.cli?, term_process::tool::var(env, "PATH"), base)
    }

    /// Where the agent's program is: its own command on the PATH outside `base`, else the version installed in
    /// `installs`.
    pub fn program(&self, env: &[(String, String)], base: &Path, installs: &Installs) -> Option<PathBuf> {
        self.command(env, base).or_else(|| {
            installs
                .current(self.id)
                .map(|(_, program)| program)
                .filter(|p| !term_process::tool::lies_in(p, base))
        })
    }
}

/// The preset with `id`.
pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

/// The presets offered for new threads whose program is on the login shell's PATH in `env` outside `base`, or
/// installed in `installs`.
pub fn available(env: &[(String, String)], base: &Path, installs: &Installs) -> Vec<&'static Preset> {
    PRESETS
        .iter()
        .filter(|p| p.offered && p.program(env, base, installs).is_some())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    fn program(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn an_adapter_s_own_program_is_found_without_the_adapter() {
        let dir = term_base::testing::scratch("acp-cli");
        let codex = program(&dir, "codex");
        let env = [("PATH".to_string(), format!("/nonexistent:{}", dir.display()))];
        let adapter = preset("codex-acp").unwrap();
        let elsewhere = Path::new("/nowhere");
        assert_eq!(adapter.cli_on_path(&env, elsewhere), Some(codex));
        assert_eq!(adapter.command(&env, elsewhere), None, "codex-acp itself is not there");
        assert_eq!(preset("gemini").unwrap().cli_on_path(&env, elsewhere), None);
        assert_eq!(preset("claude-acp").unwrap().cli, Some("claude"));
    }

    #[test]
    fn only_offered_agents_on_the_path_outside_the_workspace_are_available() {
        let dir = term_base::testing::scratch("acp-available");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        program(&bin, "agy");
        program(&bin, "gemini");
        let env = [("PATH".to_string(), bin.display().to_string())];
        let installs = Installs::new(dir.join("agents"));
        let ids: Vec<&str> = available(&env, Path::new("/nowhere"), &installs)
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, ["antigravity-acp"], "gemini is not offered");
        assert!(
            available(&env, &dir, &installs).is_empty(),
            "a command inside the workspace never runs"
        );
    }

    #[test]
    fn package_specs_split_into_name_and_version() {
        assert_eq!(
            split_package("@google/gemini-cli@0.67.0"),
            ("@google/gemini-cli".into(), Some("0.67.0".into()))
        );
        assert_eq!(
            split_package("pyagent==2.1.0"),
            ("pyagent".into(), Some("2.1.0".into()))
        );
        assert_eq!(
            split_package("pyagent@latest"),
            ("pyagent".into(), Some("latest".into()))
        );
        assert_eq!(split_package("@scope/tool"), ("@scope/tool".into(), None));
    }

    #[test]
    fn only_a_single_release_counts_as_exact() {
        assert!(is_exact("2.6.0") && is_exact("1.0.0-beta.2"));
        assert!(!is_exact("latest") && !is_exact("^1.2.0") && !is_exact("1.2") && !is_exact("1.x.0"));
    }

    #[test]
    fn every_preset_pins_an_exact_version_with_a_checksum() {
        for preset in PRESETS {
            assert!(is_exact(preset.pinned.version()), "{}", preset.id);
            match &preset.pinned {
                Pinned::Npm { integrity, .. } => assert!(integrity.starts_with("sha512-"), "{}", preset.id),
                Pinned::Binary { arm64, x86_64, .. } => {
                    for (url, sha) in [arm64, x86_64] {
                        assert!(url.starts_with("https://") && url.contains(preset.pinned.version()));
                        assert_eq!(sha.len(), 64, "{}", preset.id);
                    }
                }
            }
        }
    }
}
