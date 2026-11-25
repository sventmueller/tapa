//! Links libghostty-vt where it is built, from `GHOSTTY_VT_DIR` or the tools folder `make ghostty-vt` fills, and
//! sets `cfg(ghostty)` then; without it the comparison with Ghostty is skipped.

use std::path::PathBuf;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(ghostty)");
    println!("cargo::rerun-if-env-changed=GHOSTTY_VT_DIR");
    let dir = std::env::var_os("GHOSTTY_VT_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache/tapa-tools/ghostty-vt-out")));
    let Some(lib) = dir.map(|dir| dir.join("lib")) else {
        return;
    };
    println!("cargo::rerun-if-changed={}", lib.join("libghostty-vt.a").display());
    let Some(out) = std::env::var_os("OUT_DIR").map(PathBuf::from) else {
        return;
    };
    // The linker takes the dylib next to the archive when it finds both, so the archive goes into a folder of its own.
    if std::fs::copy(lib.join("libghostty-vt.a"), out.join("libghostty-vt.a")).is_ok() {
        println!("cargo::rustc-link-search=native={}", out.display());
        println!("cargo::rustc-link-lib=static=ghostty-vt");
        println!("cargo::rustc-link-lib=c++");
        println!("cargo::rustc-cfg=ghostty");
    }
}
