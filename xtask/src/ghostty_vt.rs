//! libghostty-vt for the differential fuzzing (E11.3): the pinned tag cloned into the tools folder and built as a
//! static library with the Zig it names; `term-fuzz` links it from there.

use std::process::Command;

use crate::{home, run};

/// The tag the comparison was set up against; P0 found the grid API complete from `vt-1.0.x` on.
const TAG: &str = "vt-1.0.1";

/// Clones the tag if needed, fetches its Zig packages and builds the library into `~/.cache/tapa-tools/ghostty-vt-out`.
pub(crate) fn build() -> Result<(), String> {
    let tools = home().join(".cache/tapa-tools");
    let source = tools.join("src/ghostty-vt");
    if !source.join("build.zig").is_file() {
        run(Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--branch",
                TAG,
                "https://github.com/ghostty-org/ghostty",
            ])
            .arg(&source))?;
    }
    let packages = tools.join("zig-offline");
    run(Command::new("sh")
        .current_dir(&source)
        .env("ZIG_GLOBAL_CACHE_DIR", &packages)
        .args(["nix/build-support/fetch-zig-cache.sh", "deps/libghostty-vt.txt"]))?;
    run(Command::new("zig")
        .current_dir(&source)
        .env("ZIG_GLOBAL_CACHE_DIR", tools.join("zig-build"))
        .arg("build")
        .arg("--system")
        .arg(packages.join("p"))
        .args([
            "-Demit-lib-vt",
            "-Demit-xcframework=false",
            "-Doptimize=ReleaseFast",
            "--prefix",
        ])
        .arg(tools.join("ghostty-vt-out")))?;
    println!(
        "ghostty-vt: built {TAG} into {}",
        tools.join("ghostty-vt-out").display()
    );
    Ok(())
}
