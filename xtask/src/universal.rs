//! The universal programs the disk image carries, for Apple Silicon and Intel in one file each.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::bundle::executable;
use crate::services::SERVICES;
use crate::*;

/// The cargo profile of the distributed programs, in `Cargo.toml`.
const PROFILE: &str = "dist";

/// Each processor's target for cargo and its architecture for `lipo`.
pub(crate) const UNIVERSAL: [(&str, &str); 2] = [("aarch64-apple-darwin", "arm64"), ("x86_64-apple-darwin", "x86_64")];

/// Builds the release of the programs `names` of the app's package with `features` for each processor and joins each
/// into `target/universal/<name>`, through rustup with the toolchain rust-toolchain.toml names, since Homebrew's Rust
/// ships the standard library for the host processor alone.
pub(crate) fn universal_programs(
    sh: &mut dyn Shell,
    names: &[&str],
    features: &[&str],
) -> Result<Vec<PathBuf>, String> {
    let dir = universal_dir();
    for (target, _) in UNIVERSAL {
        let mut build = cargo_for(sh, target)?;
        build.args([
            "build",
            "--locked",
            "--profile",
            PROFILE,
            "--package",
            PACKAGE,
            "--target",
            target,
        ]);
        for name in names {
            build.args(["--bin", name]);
        }
        if !features.is_empty() {
            build.args(["--features", &features.join(",")]);
        }
        sh.run(&mut build)?;
    }
    names
        .iter()
        .map(|name| {
            let mut join = Command::new("lipo");
            join.arg("-create");
            for (target, _) in UNIVERSAL {
                join.arg(dir.join(target).join(PROFILE).join(name));
            }
            let program = dir.join(name);
            sh.run(join.arg("-output").arg(&program))?;
            join_dsyms(sh, &dir, name)?;
            Ok(program)
        })
        .collect()
}

/// Where `universal_programs` leaves the debug symbols of the program `name` for both processors.
pub(crate) fn universal_dsym(name: &str) -> PathBuf {
    universal_dir().join(format!("{name}.dSYM"))
}

/// Joins the dSYMs of `name` that the profile's `split-debuginfo` has cargo leave beside each processor's program.
fn join_dsyms(sh: &mut dyn Shell, dir: &Path, name: &str) -> Result<(), String> {
    let dsym = universal_dsym(name);
    // rustc names the DWARF file after the crate, whose name has underscores where the program's has hyphens.
    let file = name.replace('-', "_");
    let dwarf = |bundle: &Path| bundle.join("Contents/Resources/DWARF").join(&file);
    let mut join = Command::new("lipo");
    join.arg("-create");
    for (target, _) in UNIVERSAL {
        let built = dir.join(target).join(PROFILE).join(format!("{name}.dSYM"));
        // ditto merges into the bundle of an earlier build; lipo then replaces its one DWARF file.
        sh.run(Command::new("ditto").arg(&built).arg(&dsym))?;
        join.arg(dwarf(&built));
    }
    sh.run(join.arg("-output").arg(dwarf(&dsym)))
}

/// The universal programs build in a target folder of their own: whisper-rs-sys, under dictation, hands the `GGML_*`
/// settings to whisper.cpp but does not rebuild when they change, so other builds for these targets must not share
/// its output.
fn universal_dir() -> PathBuf {
    target_dir().join("universal")
}

/// cargo of rust-toolchain.toml with what a build of the distributed program for `target` needs, as Lowspeak's and
/// Dango's.
fn cargo_for(sh: &mut dyn Shell, target: &str) -> Result<Command, String> {
    let mut cargo = rustup_cargo(sh)?;
    cargo
        .env("CARGO_TARGET_DIR", universal_dir())
        // whisper.cpp otherwise tunes itself for the processor it is built on, which other Macs may lack.
        .env("GGML_NATIVE", "OFF");
    if target.starts_with("x86_64") {
        // Every Intel Mac that runs macOS 14 has these.
        for flag in [
            "GGML_SSE42",
            "GGML_AVX",
            "GGML_AVX2",
            "GGML_BMI2",
            "GGML_FMA",
            "GGML_F16C",
        ] {
            cargo.env(flag, "ON");
        }
    }
    Ok(cargo)
}

/// cargo of the toolchain rust-toolchain.toml names, with its targets.
fn rustup_cargo(sh: &mut dyn Shell) -> Result<Command, String> {
    let root = root();
    let rustup = rustup(sh)?;
    sh.run(Command::new(&rustup).current_dir(&root).args(["toolchain", "install"]))?;
    let mut tool = |name: &str| {
        sh.stdout(Command::new(&rustup).current_dir(&root).args(["which", name]))
            .map(|path| path.trim().to_string())
    };
    let mut cargo = Command::new(tool("cargo")?);
    // Cargo would otherwise run the rustc first on the PATH, which may be Homebrew's.
    cargo.current_dir(&root).env("RUSTC", tool("rustc")?);
    Ok(cargo)
}

/// rustup from the PATH, or from Homebrew, which keeps it off the PATH beside its own Rust.
fn rustup(sh: &mut dyn Shell) -> Result<PathBuf, String> {
    let on_path = sh
        .output(Command::new("rustup").arg("--version"))
        .is_ok_and(|out| out.status.success());
    if on_path {
        return Ok("rustup".into());
    }
    sh.stdout(Command::new("brew").args(["--prefix", "rustup"]))
        .map(|prefix| Path::new(prefix.trim()).join("bin/rustup"))
        .ok()
        .filter(|path| path.exists())
        .ok_or_else(|| "this needs rustup; install it with `brew install rustup`".into())
}

/// The architectures of `UNIVERSAL` that are not in the output of `lipo -archs`.
pub(crate) fn missing_architectures(listed: &str) -> Vec<&'static str> {
    UNIVERSAL
        .iter()
        .map(|(_, arch)| *arch)
        .filter(|arch| !listed.split_whitespace().any(|name| name == *arch))
        .collect()
}

/// A program without the code for one processor goes unnoticed on the other and keeps the app from starting there:
/// checks the app's program, its services' and Sparkle's.
pub(crate) fn check_universal(sh: &mut dyn Shell, app: &Path) -> Result<(), String> {
    let mut programs = vec![executable(app)];
    programs.extend(SERVICES.iter().map(|service| service.executable(app)));
    if updates::framework(app).exists() {
        programs.extend(updates::sparkle_programs(app));
    }
    for program in programs {
        let missing = missing_architectures(&sh.stdout(Command::new("lipo").arg("-archs").arg(&program))?);
        if !missing.is_empty() {
            return Err(format!("{} lacks {}", program.display(), missing.join(" and ")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_universal_only_with_the_code_for_both_processors() {
        assert!(missing_architectures("x86_64 arm64\n").is_empty());
        assert_eq!(missing_architectures("arm64\n"), ["x86_64"]);
        assert_eq!(missing_architectures("arm64e x86_64h\n"), ["arm64", "x86_64"]);
    }

    use crate::shell::fake::{Fake, env};

    /// rustup's answers on a Mac that has it on the PATH.
    fn rustup_answer(line: &str) -> (i32, String) {
        match line {
            "rustup which cargo" => (0, "/tc/bin/cargo\n".into()),
            "rustup which rustc" => (0, "/tc/bin/rustc\n".into()),
            _ => (0, String::new()),
        }
    }

    #[test]
    fn each_processor_is_built_with_rustups_toolchain_and_joined_into_one_program() {
        let mut builds = Vec::new();
        let mut sh = Fake::new(|cmd: &Command, line: &str| {
            if line.starts_with("/tc/bin/cargo") {
                let flags = ["GGML_NATIVE", "GGML_AVX2", "RUSTC", "CARGO_TARGET_DIR"].map(|key| env(cmd, key));
                builds.push(flags);
            }
            rustup_answer(line)
        });
        let programs = universal_programs(&mut sh, &["tapa", "term-decoder"], &["dictation"]).unwrap();
        let dir = universal_dir();
        assert_eq!(programs, [dir.join("tapa"), dir.join("term-decoder")]);
        let built: Vec<&str> = sh.of("/tc/bin/cargo");
        assert_eq!(
            built[0],
            "/tc/bin/cargo build --locked --profile dist --package tapa --target aarch64-apple-darwin --bin tapa --bin \
             term-decoder --features dictation"
        );
        assert!(built[1].contains("--target x86_64-apple-darwin"));
        assert_eq!(sh.of("rustup")[..2], ["rustup --version", "rustup toolchain install"]);
        let joined = sh.of("lipo");
        assert_eq!(
            joined[0],
            format!(
                "lipo -create {} {} -output {}",
                dir.join("aarch64-apple-darwin/dist/tapa").display(),
                dir.join("x86_64-apple-darwin/dist/tapa").display(),
                dir.join("tapa").display()
            )
        );
        let dwarf = |bundle: &str| format!("{}/Contents/Resources/DWARF/tapa", dir.join(bundle).display());
        assert_eq!(
            joined[1],
            format!(
                "lipo -create {} {} -output {}",
                dwarf("aarch64-apple-darwin/dist/tapa.dSYM"),
                dwarf("x86_64-apple-darwin/dist/tapa.dSYM"),
                dwarf("tapa.dSYM")
            ),
            "the debug symbols of both processors in one dSYM"
        );
        let decoder = |bundle: &str| format!("{}/Contents/Resources/DWARF/term_decoder", dir.join(bundle).display());
        assert_eq!(
            joined[3],
            format!(
                "lipo -create {} {} -output {}",
                decoder("aarch64-apple-darwin/dist/term-decoder.dSYM"),
                decoder("x86_64-apple-darwin/dist/term-decoder.dSYM"),
                decoder("term-decoder.dSYM")
            ),
            "the DWARF file is named after the crate, with an underscore"
        );
        assert_eq!(joined.len(), 4);
        assert_eq!(
            sh.of("ditto")[..2],
            [
                format!(
                    "ditto {} {}",
                    dir.join("aarch64-apple-darwin/dist/tapa.dSYM").display(),
                    universal_dsym("tapa").display()
                ),
                format!(
                    "ditto {} {}",
                    dir.join("x86_64-apple-darwin/dist/tapa.dSYM").display(),
                    universal_dsym("tapa").display()
                )
            ]
        );
        drop(sh);
        let dir = Some(dir.display().to_string());
        let off = Some("OFF".to_string());
        let rustc = Some("/tc/bin/rustc".to_string());
        assert_eq!(builds[0], [off.clone(), None, rustc.clone(), dir.clone()]);
        assert_eq!(builds[1], [off, Some("ON".to_string()), rustc, dir]);
    }

    #[test]
    fn without_rustup_on_the_path_homebrews_is_looked_for() {
        let mut sh = Fake::new(|_: &Command, line: &str| match line {
            "rustup --version" => (127, String::new()),
            "brew --prefix rustup" => (0, "/nowhere/rustup\n".into()),
            _ => (0, String::new()),
        });
        let missing = universal_programs(&mut sh, &["tapa"], &[]).unwrap_err();
        assert_eq!(missing, "this needs rustup; install it with `brew install rustup`");
        assert_eq!(sh.commands, ["rustup --version", "brew --prefix rustup"]);
    }

    #[test]
    fn a_program_without_the_code_for_one_processor_stops_the_release() {
        let app = Path::new("/r/T.app");
        let mut sh = Fake::new(|_: &Command, line: &str| {
            if line.ends_with("/MacOS/tapa") {
                (0, "x86_64 arm64\n".into())
            } else {
                (0, "arm64\n".into())
            }
        });
        let failed = check_universal(&mut sh, app).unwrap_err();
        let service = SERVICES[0].executable(app);
        assert_eq!(failed, format!("{} lacks x86_64", service.display()));
        assert_eq!(sh.commands[0], format!("lipo -archs {}", executable(app).display()));
    }

    #[test]
    fn the_toolchain_file_names_both_processors() {
        let toolchain = files::read_to_string(root().join("rust-toolchain.toml")).unwrap();
        for (target, _) in UNIVERSAL {
            assert!(toolchain.contains(&format!("\"{target}\"")), "{target}");
        }
    }
}
