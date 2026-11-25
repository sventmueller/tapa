//! The register read from a copy of the ACP registry, what installing an entry would do, and installing, checking,
//! updating and going back with a stand-in for the network.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::collections::HashMap;
use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};
use term_base::testing::scratch;

use sha2::{Digest, Sha256};
use term_acp::install::{InstallError, Refusal, Runtime, Source};
use term_acp::register::{self, Distribution, PRESETS};
use term_acp::{Installer, Installs, Launch, Plan, Register};
use term_net::{Download, Net};
use term_process::Supervisor;

const REGISTRY: &str = include_str!("fixtures/registry.json");

#[derive(Default)]
struct FakeNet {
    files: Mutex<HashMap<String, Vec<u8>>>,
}

impl Net for FakeNet {
    fn get(&self, url: &str, _: &str) -> io::Result<Download> {
        let files = self.files.lock().unwrap();
        let bytes = files
            .get(url)
            .cloned()
            .ok_or_else(|| io::Error::other(format!("404 {url}")))?;
        Ok(Download {
            length: None,
            body: Box::new(io::Cursor::new(bytes)),
        })
    }
}

/// A zip holding an executable script `agent` that prints `version`.
fn archive(dir: &Path, version: &str) -> Vec<u8> {
    let content = dir.join(format!("content-{version}"));
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("agent"), format!("#!/bin/sh\necho {version}\n")).unwrap();
    let zip = dir.join(format!("agent-{version}.zip"));
    let status = std::process::Command::new("/usr/bin/ditto")
        .args(["-c", "-k"])
        .arg(&content)
        .arg(&zip)
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(zip).unwrap()
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn plan(version: &str, sha256: Option<String>, verified: bool) -> Plan {
    Plan {
        id: "fake".into(),
        name: "Fake".into(),
        version: version.into(),
        source: Source::Archive {
            url: format!("https://example.test/agent-{version}.zip"),
            sha256,
            cmd: "./agent".into(),
            args: vec!["acp".into()],
            env: vec![("FAKE".into(), "1".into())],
        },
        verified,
        checksum: true,
        runtime: Runtime::None,
    }
}

#[test]
fn the_registry_is_read_for_this_mac() {
    let register = Register::parse(REGISTRY).unwrap();
    assert_eq!(register.entries.len(), 9);
    let opencode = register.entry("opencode").unwrap();
    let Distribution::Binary {
        archive, sha256, args, ..
    } = &opencode.distributions[0]
    else {
        panic!("{opencode:?}")
    };
    assert!(archive.contains("darwin"), "only this Mac's build is kept");
    assert_eq!(sha256.as_ref().map(String::len), Some(64));
    assert_eq!(args, &["acp"]);
    let gemini = register.entry("gemini").unwrap();
    let Distribution::Npm(package) = &gemini.distributions[0] else {
        panic!()
    };
    assert_eq!(
        (package.name.as_str(), package.version.as_deref()),
        ("@google/gemini-cli", Some("0.67.0"))
    );
    assert_eq!(register.entry("container-agent").unwrap().unknown, ["docker"]);
    for preset in PRESETS {
        assert!(register.entry(preset.id).is_some(), "the registry knows {}", preset.id);
    }
}

#[test]
fn entries_install_only_with_an_exact_version_and_ask_without_a_checksum() {
    let register = Register::parse(REGISTRY).unwrap();
    let gemini = Plan::of_entry(register.entry("gemini").unwrap()).unwrap();
    assert!(matches!(&gemini.source, Source::Npm { version, .. } if version == "0.67.0"));
    assert_eq!(gemini.runtime, Runtime::Node(None));
    assert!(
        gemini.needs_confirmation(),
        "a registry entry is not one this build was tried with"
    );
    let cursor = Plan::of_entry(register.entry("cursor").unwrap()).unwrap();
    assert!(!cursor.checksum && cursor.needs_confirmation());
    let fast = Plan::of_entry(register.entry("fast-agent").unwrap()).unwrap();
    assert!(
        matches!(&fast.source, Source::Uv { package, version, .. } if package == "fast-agent-acp" && version == "0.10.1")
    );
    assert_eq!(
        Plan::of_entry(register.entry("unpinned-agent").unwrap()),
        Err(Refusal::Unversioned("unpinned-agent".into()))
    );
    assert_eq!(
        Plan::of_entry(register.entry("container-agent").unwrap()),
        Err(Refusal::Nothing {
            unknown: vec!["docker".into()]
        })
    );
    let preset = Plan::of_preset(register::preset("opencode").unwrap());
    assert!(
        !preset.verified && preset.checksum && preset.needs_confirmation(),
        "a preset asks until Dango's make test-integration verified it"
    );
    assert!(register::PRESETS.iter().all(|p| !p.verified));
}

#[test]
fn an_archive_installs_only_with_its_checksum_and_an_update_can_be_taken_back() {
    let dir = scratch("install");
    let net = Arc::new(FakeNet::default());
    let installs = Installs::new(dir.join("agents"));
    let installer = Installer::new(
        &Supervisor::new(),
        vec![("PATH".into(), "/usr/bin:/bin".into())],
        installs.clone(),
        net.clone(),
    );
    let mut versions = HashMap::new();
    for version in ["1.0.0", "2.0.0", "3.0.0"] {
        let bytes = archive(&dir, version);
        versions.insert(version, sha(&bytes));
        net.files
            .lock()
            .unwrap()
            .insert(format!("https://example.test/agent-{version}.zip"), bytes);
    }
    let first = plan("1.0.0", Some(versions["1.0.0"].clone()), false);
    assert!(matches!(
        installer.install(&first, false, &|_| {}),
        Err(InstallError::NotConfirmed)
    ));
    let wrong = plan("1.0.0", Some("0".repeat(64)), true);
    assert!(matches!(
        installer.install(&wrong, false, &|_| {}),
        Err(InstallError::Checksum { .. })
    ));
    assert!(installs.current("fake").is_none() && !dir.join("agents/fake/1.0.0").exists());

    let installed = installer.install(&first, true, &|_| {}).unwrap();
    assert_eq!((installed.version.as_str(), installed.verified), ("1.0.0", false));
    let (_, program) = installs.current("fake").unwrap();
    let output = std::process::Command::new(&program).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "1.0.0");

    installer
        .install(&plan("2.0.0", Some(versions["2.0.0"].clone()), true), false, &|_| {})
        .unwrap();
    assert_eq!(installs.current("fake").unwrap().0.version, "2.0.0");
    assert_eq!(installs.previous("fake").unwrap().version, "1.0.0");
    assert_eq!(installs.rollback("fake").unwrap().version, "1.0.0");
    assert_eq!(installs.current("fake").unwrap().0.version, "1.0.0");
    assert_eq!(installs.previous("fake").unwrap().version, "2.0.0");

    installer
        .install(&plan("3.0.0", Some(versions["3.0.0"].clone()), true), false, &|_| {})
        .unwrap();
    assert_eq!(installs.previous("fake").unwrap().version, "1.0.0");
    assert!(
        !dir.join("agents/fake/2.0.0").exists(),
        "only the current and the previous version stay"
    );

    let env = [("PATH".to_string(), "/usr/bin".to_string())];
    let launch = Launch::installed("fake", "Fake", &installs, &env, &dir.join("project"), true).unwrap();
    assert_eq!(launch.args, ["acp"]);
    assert!(launch.env.contains(&("FAKE".into(), "1".into())));
    assert!(launch.may_start(), "it starts in a trusted workspace");
    let untrusted = Launch::installed("fake", "Fake", &installs, &env, &dir.join("project"), false).unwrap();
    assert!(
        !untrusted.may_start(),
        "an agent from the registry has no switch to skip the project's configuration"
    );
    assert!(
        Launch::installed("fake", "Fake", &installs, &env, &dir, true).is_none(),
        "a program inside the workspace never runs"
    );
}

#[test]
fn a_missing_runtime_is_named_before_anything_is_installed() {
    let dir = scratch("runtime");
    let installer = Installer::new(
        &Supervisor::new(),
        vec![("PATH".into(), dir.to_string_lossy().into_owned())],
        Installs::new(dir.join("agents")),
        Arc::new(FakeNet::default()),
    );
    let gemini = Plan::of_preset(register::preset("gemini").unwrap());
    assert!(matches!(
        installer.missing(&gemini),
        Some(InstallError::Runtime("Node.js", Some(20)))
    ));
}
