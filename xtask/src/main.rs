//! The build tool: `cargo xtask <task>`. `make help` lists what the tasks are for.

mod bundle;
mod cask;
mod deploy;
mod esctest;
mod ghostty_vt;
mod icon;
mod perf;
#[cfg(test)]
mod readme;
mod release;
mod render_page;
mod services;
mod shell;
mod source;
mod strings;
mod structure;
mod themes;
mod universal;
mod updates;
mod version;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) use shell::{Shell, System};

/// The package of the app and its program, which the release builds universal.
pub(crate) const PACKAGE: &str = "tapa";
pub(crate) const PROGRAM: &str = "tapa";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|arg| arg == name);
    let result = match args.first().map(String::as_str) {
        Some("bundle") => bundle::bundle(flag("--release")).map(|app| println!("{}", app.display())),
        Some("run") => bundle::bundle(flag("--release")).and_then(|app| run(Command::new("open").arg("-n").arg(app))),
        Some("selftest") => selftest(
            args.get(1).map_or("shot:start", String::as_str),
            flag("--release"),
            std::env::var_os("TAPA_SELFTEST_CONFIG").map(PathBuf::from).as_deref(),
            std::time::Duration::from_secs(120),
        ),
        Some("esctest") => esctest::main(&args[1..]),
        Some("ghostty-vt") => ghostty_vt::build(),
        Some("icon") => icon::build(),
        Some("structure") => structure::check(),
        Some("strings") if args.get(1).is_some_and(|arg| arg == "alike") => strings::print_alike(),
        Some("strings") => strings::check(),
        Some("themes") => match args.get(1) {
            Some(source) => themes::import(Path::new(source)),
            None => Err("usage: cargo xtask themes <iTerm2-Color-Schemes/ghostty folder>".into()),
        },
        Some("perf") => perf::main(&args[1..]),
        Some("release") => flags(&args[1..], ["--notarize", "--layout"]).and_then(|[notarize, layout]| {
            let out = release::release(release::Options {
                notarize,
                layout: layout || notarize,
            })?;
            println!("{}", out.display());
            Ok(())
        }),
        Some("deploy") => deploy::deploy(&args[1..]),
        _ => Err("usage: cargo xtask bundle|run [--release] | selftest <steps> | esctest … | ghostty-vt | icon | structure | strings [alike] | themes <folder> | perf … | release [--notarize] [--layout] | deploy [--minor|--major]".into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Builds the app with its self-test and runs `steps` in it with the settings of `config`, for at most `limit`; the
/// pictures land in `target/selftest`.
pub(crate) fn selftest(
    steps: &str,
    release: bool,
    config: Option<&Path>,
    limit: std::time::Duration,
) -> Result<(), String> {
    let app = bundle::bundle(release)?;
    let dir = target_dir().join("selftest");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let settings = dir.join("config.toml");
    if let Some(own) = config {
        std::fs::copy(own, &settings).map_err(|e| format!("{}: {e}", own.display()))?;
    }
    let mut child = Command::new(app.join("Contents/MacOS/tapa"))
        .env("TAPA_SELFTEST", steps)
        .env("TAPA_SELFTEST_DIR", &dir)
        .env("TAPA_CONFIG", &settings)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(stdout) = child.stdout.take() {
        std::thread::spawn(move || {
            use std::io::BufRead;
            for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                let mut parts = line.splitn(3, ' ');
                if let (Some("shot"), Some(window), Some(path)) = (parts.next(), parts.next(), parts.next()) {
                    let _ = Command::new("screencapture")
                        .args(["-x", "-o", "-l", window, path])
                        .status();
                } else {
                    println!("{line}");
                }
            }
        });
    }
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            println!("selftest: done ({status}), pictures in {}", dir.display());
            return Ok(());
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            return Err(format!("the self-test took longer than {} s", limit.as_secs()));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The user's home folder.
pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

/// The workspace's root folder.
pub(crate) fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
}

/// Where cargo puts what it builds.
pub(crate) fn target_dir() -> PathBuf {
    std::env::var_os("CARGO_TARGET_DIR").map_or_else(|| root().join("target"), PathBuf::from)
}

/// Runs `command`; an error when it fails.
pub(crate) fn run(command: &mut Command) -> Result<(), String> {
    let status = command.status().map_err(|error| format!("{command:?}: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{command:?} failed: {status}"))
    }
}

/// Which of `known` the arguments name; any other argument is an error.
pub(crate) fn flags<const N: usize>(args: &[String], known: [&str; N]) -> Result<[bool; N], String> {
    if let Some(unknown) = args.iter().find(|a| !known.contains(&a.as_str())) {
        return Err(format!(
            "unknown argument {unknown}; this task knows {}",
            known.join(", ")
        ));
    }
    Ok(known.map(|flag| args.iter().any(|a| a == flag)))
}

/// Major, minor and build number, digits only.
pub(crate) fn is_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// The product's version from the workspace's `Cargo.toml`, the most recently deployed one. Only deploy changes it.
pub(crate) fn version() -> Result<String, String> {
    let path = root().join(version::MANIFEST);
    let text = files::read_to_string(&path)?;
    match version::of_manifest(&text) {
        Some(version) if is_version(version) => Ok(version.to_string()),
        Some(version) => Err(format!("{}: {version:?} is not major.minor.build", path.display())),
        None => Err(format!("{}: no version in [workspace.package]", path.display())),
    }
}

/// The version after `version`: the same major and minor, the next build number.
pub(crate) fn following(version: &str) -> Result<String, String> {
    if !is_version(version) {
        return Err(format!("{version:?} is not major.minor.build"));
    }
    let (head, build) = version.rsplit_once('.').ok_or("no build number")?;
    let build: u64 = build.parse().map_err(|e| format!("build number {build}: {e}"))?;
    let next = build.checked_add(1).ok_or("the build number is at its maximum")?;
    Ok(format!("{head}.{next}"))
}

/// The version after `version` that raises the minor number and starts the build number at 0.
pub(crate) fn following_minor(version: &str) -> Result<String, String> {
    if !is_version(version) {
        return Err(format!("{version:?} is not major.minor.build"));
    }
    let mut parts = version.split('.');
    let (major, minor) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let minor: u64 = minor.parse().map_err(|e| format!("minor number {minor}: {e}"))?;
    let next = minor.checked_add(1).ok_or("the minor number is at its maximum")?;
    Ok(format!("{major}.{next}.0"))
}

/// The version after `version` that raises the major number and starts minor and build number at 0.
pub(crate) fn following_major(version: &str) -> Result<String, String> {
    if !is_version(version) {
        return Err(format!("{version:?} is not major.minor.build"));
    }
    let major = version.split('.').next().unwrap_or_default();
    let major: u64 = major.parse().map_err(|e| format!("major number {major}: {e}"))?;
    let next = major.checked_add(1).ok_or("the major number is at its maximum")?;
    Ok(format!("{next}.0.0"))
}

/// `std::fs` with the path in every error.
pub(crate) mod files {
    use std::fs;
    use std::path::Path;

    fn at<T>(path: &Path, result: std::io::Result<T>) -> Result<T, String> {
        result.map_err(|e| format!("{}: {e}", path.display()))
    }

    pub(crate) fn write(path: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Result<(), String> {
        at(path.as_ref(), fs::write(path.as_ref(), content))
    }

    pub(crate) fn read_to_string(path: impl AsRef<Path>) -> Result<String, String> {
        at(path.as_ref(), fs::read_to_string(path.as_ref()))
    }

    pub(crate) fn create_dir_all(path: impl AsRef<Path>) -> Result<(), String> {
        at(path.as_ref(), fs::create_dir_all(path.as_ref()))
    }

    pub(crate) fn remove_dir_all(path: impl AsRef<Path>) -> Result<(), String> {
        at(path.as_ref(), fs::remove_dir_all(path.as_ref()))
    }

    pub(crate) fn remove_file(path: impl AsRef<Path>) -> Result<(), String> {
        at(path.as_ref(), fs::remove_file(path.as_ref()))
    }

    pub(crate) fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), String> {
        at(from.as_ref(), fs::rename(from.as_ref(), to.as_ref()))
    }

    pub(crate) fn copy(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<u64, String> {
        at(from.as_ref(), fs::copy(from.as_ref(), to.as_ref()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_have_three_numbers() {
        assert!(is_version("0.1.0") && is_version("12.3.456"));
        assert!(!is_version("1.0") && !is_version("1.0.x") && !is_version("1..1") && !is_version("1.0.1.2"));
        assert!(!is_version("") && !is_version("1.0.-1") && !is_version(" 1.0.1"));
    }

    #[test]
    fn a_deploy_raises_only_the_build_number() {
        assert_eq!(following("0.1.0").unwrap(), "0.1.1");
        assert_eq!(following("2.3.99").unwrap(), "2.3.100");
        assert!(following("1.0").is_err() && following("1.0.x").is_err());
        assert!(following(&format!("1.0.{}", u64::MAX)).is_err());
        assert_eq!(following_minor("0.2.9").unwrap(), "0.3.0");
        assert_eq!(following_minor("1.9.0").unwrap(), "1.10.0");
        assert!(following_minor("1.0").is_err());
        assert_eq!(following_major("0.1.0").unwrap(), "1.0.0");
        assert_eq!(following_major("1.9.4").unwrap(), "2.0.0");
        assert!(following_major("1.0").is_err());
    }

    #[test]
    fn the_version_of_the_workspace_is_valid() {
        assert!(version().is_ok());
    }

    #[test]
    fn a_task_takes_only_the_options_it_knows() {
        let strings = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        assert_eq!(flags(&[], ["--notarize", "--layout"]).unwrap(), [false, false]);
        assert_eq!(
            flags(&strings(&["--layout"]), ["--notarize", "--layout"]).unwrap(),
            [false, true]
        );
        assert!(flags(&strings(&["--unnotarized"]), ["--notarize"]).is_err());
    }
}
