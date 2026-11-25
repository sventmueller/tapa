//! The self-test's steps for installing agents (E21), with a stand-in registry and network, never the real ones:
//! `agentregister:<version>` has the registry read as if fetched, with the stand-in agent `selftest-agent` at that
//! version (an archive with its checksum), one entry without a fixed version and one only for another system, and
//! opens its window from the Agents pane; `agentinstall:<id>` installs a preset or an entry as its button does,
//! without the question before it, into a folder of the self-test (a preset fails: the stand-in network has none of
//! its downloads, and npm, node and uv are kept off the PATH); `agentrollback:<id>` goes back to the version before;
//! `agentrows:<name>` prints the Agents pane's lines and the registry window's entries. Open the pane with
//! `settings:agents` first. In an agent tab (`agent:acp:selftest-agent` once the registry is read),
//! `agentproblem:<name>` prints what keeps its session from running, and `agentpress:<title>` clicks that overlay's
//! button (`modal:Installieren=agentpress:Installieren …` answers the question it asks); the stand-in agent speaks ACP.
use std::path::{Path, PathBuf};
use std::process::Command;

use term_acp::Plan;
use term_acp::register::{self, Register};

use super::stand_in;

/// The stand-in agent's id in the registry.
const AGENT: &str = "selftest-agent";

/// Points installations at an empty folder of the self-test and the network at a stand-in that has no files yet.
pub(crate) fn prepare() {
    let _ = std::fs::remove_dir_all(folder());
    term_ui_agent::set_test_installs(folder().join("agents"), Vec::new());
}

fn folder() -> PathBuf {
    stand_in::dir().with_file_name("agent-installs")
}

/// Runs `step` if it is one of these steps; whether it was.
pub(crate) fn run(step: &str) -> bool {
    let Some((name, argument)) = step.split_once(':') else {
        return false;
    };
    match name {
        "agentregister" => registry(argument),
        "agentinstall" => install(argument),
        "agentrollback" => rollback(argument),
        "agentrows" => rows(argument),
        "agentproblem" => problem(argument),
        "agentpress" => press(argument),
        _ => return false,
    }
    true
}

/// The address the stand-in agent's archive of `version` is served at.
fn address(version: &str) -> String {
    format!("https://selftest.invalid/{AGENT}-{version}.tar.gz")
}

/// A tar archive with the stand-in agent, a script that says its version; its bytes.
fn archive(version: &str) -> std::io::Result<Vec<u8>> {
    let content = folder().join(format!("content-{version}"));
    std::fs::create_dir_all(&content)?;
    let program = content.join("agent");
    let exe = std::env::current_exe()?;
    let env: Vec<String> = stand_in::env("acp:basic")
        .into_iter()
        .map(|(key, value)| format!("'{key}={value}'"))
        .collect();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = acp ]; then exec /usr/bin/env {} '{}'; fi\necho {AGENT} {version}\n",
        env.join(" "),
        exe.display()
    );
    std::fs::write(&program, script)?;
    std::fs::set_permissions(&program, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    let file = folder().join(format!("{AGENT}-{version}.tar.gz"));
    let made = Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&file)
        .arg("-C")
        .arg(&content)
        .arg("agent")
        .status()?;
    if !made.success() {
        return Err(std::io::Error::other("tar failed"));
    }
    std::fs::read(&file)
}

fn sha256(file: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/shasum")
        .args(["-a", "256"])
        .arg(file)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
}

fn registry(version: &str) {
    let bytes = match archive(version) {
        Ok(bytes) => bytes,
        Err(error) => return eprintln!("selftest: the stand-in agent's archive: {error}"),
    };
    let sha = sha256(&folder().join(format!("{AGENT}-{version}.tar.gz"))).unwrap_or_default();
    let target = if cfg!(target_arch = "aarch64") {
        "darwin-aarch64"
    } else {
        "darwin-x86_64"
    };
    let json = serde_json::json!({
        "version": "1.0.0",
        "agents": [
            {"id": AGENT, "name": "Selftest Agent", "version": version, "description": "The self-test's stand-in.",
             "distribution": {"binary": {target: {"archive": address(version), "cmd": "./agent", "args": ["acp"],
                                                   "sha256": sha}}}},
            {"id": "unpinned-agent", "name": "Unpinned", "version": "1.0.0", "description": "No fixed version.",
             "distribution": {"npx": {"package": "unpinned-agent@latest"}}},
            {"id": "elsewhere-agent", "name": "Elsewhere", "version": "1.0.0", "description": "Not for this Mac.",
             "distribution": {"binary": {"linux-x86_64": {"archive": "https://selftest.invalid/l.tar.gz", "cmd": "a"}}}}
        ]
    });
    match Register::parse(&json.to_string()) {
        Ok(register) => term_ui_agent::set_test_register(register),
        Err(error) => return eprintln!("selftest: the stand-in registry: {error}"),
    }
    term_ui_agent::set_test_installs(folder().join("agents"), vec![(address(version), bytes)]);
    let Some(target) = crate::settings::installs_target() else {
        return eprintln!("selftest: agentregister needs settings:agents first");
    };
    target.show_register_window();
    rows(&format!("register-{version}"));
}

fn install(id: &str) {
    let Some(target) = crate::settings::installs_target() else {
        return eprintln!("selftest: agentinstall needs settings:agents first");
    };
    if let Some(preset) = register::preset(id) {
        return target.install(preset.id, preset.name, Plan::of_preset(preset), false);
    }
    let entry = term_ui_agent::cached_register().and_then(|(register, _)| register.entry(id).cloned());
    match entry.map(|entry| (Plan::of_entry(&entry), entry)) {
        Some((Ok(plan), entry)) => target.install(&entry.id, &entry.name, plan, false),
        Some((Err(refusal), _)) => println!("agentinstall refused {id}: {}", term_ui_agent::refusal_text(&refusal)),
        None => println!("agentinstall {id}: not in the registry"),
    }
}

fn rollback(id: &str) {
    let Some(target) = crate::settings::installs_target() else {
        return eprintln!("selftest: agentrollback needs settings:agents first");
    };
    target.rollback(id, id);
    let current = term_ui_agent::installs().current(id).map(|(installed, program)| {
        let said = Command::new(&program)
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        (installed.version, said.unwrap_or_default())
    });
    println!("agentrollback {id}: now {current:?}");
}

fn rows(name: &str) {
    let Some(target) = crate::settings::installs_target() else {
        return eprintln!("selftest: agentrows needs settings:agents first");
    };
    println!("agentrows {name}: {:?}", target.lines_text());
    println!("agentrows {name} window: {:?}", crate::settings::register_text());
}

fn problem(name: &str) {
    let Some(tab) = super::agent::front_agent() else {
        return println!("agentproblem {name}: no agent tab");
    };
    println!(
        "agentproblem {name}: {:?} {:?}, installed {:?}",
        tab.problem_name(),
        tab.problem_texts(),
        term_ui_agent::installs()
            .current(AGENT)
            .map(|(installed, _)| installed.version)
    );
}

/// Clicks the overlay's button `title`; `modal:` around this step answers the alert the button asks.
fn press(title: &str) {
    let pressed = super::agent::front_agent().map(|tab| tab.press_problem(title));
    println!("agentpress {title}: pressed {pressed:?}");
}
