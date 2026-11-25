# Contributing

Problems and ideas go into an [issue](https://github.com/sventmueller/tapa/issues/new/choose). Security problems follow the [security policy](SECURITY.md).

## Pull requests

Small fixes can go straight to a pull request. For anything larger, describe the idea in an issue first, so that no work goes into a change Tapa will not take.

- You need checkouts of [splat](https://github.com/sventmueller/splat) and [lowspeak](https://github.com/sventmueller/lowspeak) next to this one, and Rust as `rust-toolchain.toml` names it. `make help` lists the targets.
- `cargo xtask run` builds and opens the debug app. `make selftest STEPS="…"` drives it and leaves window pictures in `target/selftest/`; agents in the self-test are stand-ins, never the real CLIs.
- `make check` must pass. It covers formatting, clippy without warnings, the shape of the files (no Rust file over 1000 lines), the interface texts and all tests. `make check-dictation` covers the build with dictation, which needs cmake.
- Interface texts use the German text as the key, through `t("…")` and `tf("… {}", …)` from `splat_i18n`. Every new key needs an entry in the crate's `strings/en.strings` and a translation in every language; `cargo xtask strings` fails without them.
- AppKit callbacks run inside `guarded(..)`, and every `unsafe` block carries a `// SAFETY:` comment.
- Nothing from a project folder runs: read Git state from the `.git` files, start programs only from the user's `PATH` outside the project, and decode untrusted pictures through `Decoder.xpc`, never with ImageIO or NSImage in the app.
- Keep to one change per pull request.

## License

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Tapa by you, as defined in the Apache-2.0 license, shall be dual licensed under Apache-2.0 and MIT, without any additional terms or conditions.
