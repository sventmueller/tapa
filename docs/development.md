# Development

The commands are in the README's [For developers](../README.md#for-developers) and in `make help`; this document describes how a release is built, signed and published, as Dango's `docs/development.md` does for Dango.

## Builds
The display name and the identifiers come from the crate `term-product` (E19); the table shows their current values. The identifiers never contain the display name, so renaming the app strands no settings.

| Build | Bundle | Bundle identifier | URL scheme |
|---|---|---|---|
| Debug | `target/debug/Tapa Debug.app` | `com.muellermails.terminal.debug` | `tapa-debug` |
| Release | `target/release/Tapa.app` | `com.muellermails.terminal` | `tapa` |
| Distribution | `target/distribution/Tapa.app` | `com.muellermails.terminal` | `tapa` |

`version` in `[workspace.package]` of the root `Cargo.toml` holds major.minor.build and names the most recently deployed version; every package of the workspace takes it with `version.workspace = true`, so cargo builds each crate at that version and the programs report it (`TERM_PROGRAM_VERSION`, XTVERSION). Only `make deploy` raises it: it writes the next version into `Cargo.toml` and `Cargo.lock`, builds and notarizes that, then commits exactly these two files, or puts them back if the build fails. Normal builds, self-tests, dry runs and standalone `make release-notarized` keep the version unchanged, and a release refuses a version other than the one in `Cargo.toml`. The crates, programs, bundle, tag, release, update feed and committed `Cargo.toml` therefore all carry the same number; a test in xtask fails if a package does not take the workspace's version.

## Signing
- Local builds (`cargo xtask run`, `make build`) are signed with the Developer ID when the keychain holds one, else ad hoc, without the hardened runtime and without the secure timestamp. An ad hoc signature changes with every build, so the keychain asks again for the key of the restored state; the certificate's does not.
- The distribution is signed with the hardened runtime and without the App Sandbox, whose restrictions every program in the terminal would inherit. The decoding service `Decoder.xpc` is signed first, with the App Sandbox and nothing else, then Sparkle's `Autoupdate`, `Updater.app` and framework, then the app with the entitlements of `entitlements` in `xtask/src/bundle.rs`: the exceptions of the hardened runtime that the usage texts of its `Info.plist` explain (F217), such as the microphone for dictation and the programs in the terminal.
- A notarized release signs with the first valid Developer ID Application certificate and the secure timestamp, see [Release](#release).

## Release
`cargo xtask release` builds what a release publishes (F75), in `xtask/src/release.rs`:
1. The universal programs (`xtask/src/universal.rs`): the app with `dictation` and `term-decoder` for `aarch64-apple-darwin` and `x86_64-apple-darwin` through rustup, each in `target/universal/<target>` with `GGML_NATIVE=OFF` and, for Intel, the instruction sets every Intel Mac with macOS 14 has, as Lowspeak and Dango build it; joined with `lipo` into `target/universal/<program>`. It needs `../lowspeak` and cmake. The profile `dist` strips the programs but has `dsymutil` write their line tables first (`split-debuginfo = "packed"`); both processors' dSYMs are joined into `target/universal/<program>.dSYM`.
2. The distribution bundle in `target/distribution/`: the universal programs, Sparkle 2.10.1 without its XPC services (only a sandboxed app needs them) and with its license, `SUFeedURL` and `SUPublicEDKey` in `Info.plist`.
3. The signature, inside out (see [Signing](#signing)); then a check that every program has both architectures, Sparkle's included.
4. The disk image `Tapa.dmg`: the app and a link to `/Applications`, compressed, signed; with `--layout` Finder sets the window up over `assets/dmg-background.tiff` (Splat's and Dango's), which opens the image on screen and needs the Automation permission for Finder once.
5. The image mounted out of sight (`-nobrowse`) and checked: intact, the app signed and universal, the link in place.
6. Into the release folder: `Tapa-<version>.dmg`, `Tapa-<version>.md` with the English notes, `appcast.xml` (checked with `xmllint`) and the Homebrew Cask `tapa.rb` with the image's checksum (checked with `ruby -c`), and beside them, not in the image and not uploaded, `Tapa-<version>-dSYMs/` with the dSYM of every program, which crash reports of that version are symbolicated with (`atos`, or Spotlight finds them by UUID).

Without `--notarize` it is a dry run into `target/release-dry-run/`: signed as local builds are and without the secure timestamp, no notarization, no stapling, the feed's EdDSA signature a placeholder with the image's real length, the version unchanged. It asks Apple for nothing and the keychain only for items' attributes, and prints what a notarized release still lacks.

Both runs check the translations before the long build with `cargo xtask strings`: a key without an English entry or without a translation in one of the 35 languages stops the release, and the output names each one.

`--notarize` stops before the long build unless everything is there, then signs with the first valid Developer ID Application certificate, has Apple notarize the app (as a zip) and then the image, staples both tickets, has Gatekeeper assess the image, signs it for Sparkle with the keychain's key and writes into `dist/` without changing the version. It needs:
- `RELEASES` in `crates/term-product/src/lib.rs`: the releases page in GitHub's layout; the image is downloaded from `<RELEASES>/download/v<version>/`, the feed from `<RELEASES>/latest/download/appcast.xml`.
- `HOMEPAGE` in `crates/term-product/src/lib.rs`, for the Cask.
- The notarytool profile `notary`, as for Splat, Lowspeak and Dango: `xcrun notarytool store-credentials notary --apple-id <Apple ID> --team-id <team ID>`.
- The EdDSA key Splat and Lowspeak sign their updates with, under Sparkle's default keychain account `ed25519`, and its public half in `UPDATE_PUBLIC_KEY` in `crates/term-product/src/lib.rs`. Unlike Dango, which signs with a key of its own, Tapa shares this key by the owner's decision, so one backup covers the three apps. Installed copies accept no other key, so losing the private half ends their updates.
- `RELEASE_NOTES.md` and `assets/readme/<language>/RELEASE_NOTES.md` for every other language in `resources/`, new for the release and reviewed: notes that still begin with the comment `<!-- DRAFT` are refused, and the run names every language whose notes are missing. A dry run leaves the draft comment out of the feed and lists what is missing.

`dist/` is ignored by git; the disk images are GitHub release assets, not source commits.

## Deploy
`make deploy` (`cargo xtask deploy`, `xtask/src/deploy.rs`) releases from this Mac; no GitHub workflow takes part. It refuses unless `main` is checked out and everything is committed, origin is `sventmueller/tapa`, `gh` may push there and origin's `main` is contained in `HEAD`, and unless `../splat` and `../lowspeak` have no changes and, after a fetch, their HEAD is on their upstream; `dist/deployment.json` records both commits under `siblings`, so the release can be built again. It notarizes the next version (`ARGS=--minor` the next minor version with build number 0, `ARGS=--major` the next major version as x.0.0; Dango has no `--major`) as `--notarize --layout` does, checks that the source did not change meanwhile, commits only `Cargo.toml` and `Cargo.lock` with the raised version as `Release Tapa <version>`, creates its tag and uploads to a draft release the image, an identical `Tapa.dmg` for the README's stable download link, the notes, the feed, the Cask and `deployment.json` with the SHA-256 of each, after checking that the feed offers the signed image of that version. Only after the complete upload does it push `main` and the version tag atomically. Then it publishes the draft as latest itself, once origin's `main` is the release commit and GitHub's checksum of every file in the draft matches the manifest. An upload, push or publication failure leaves the draft unpublished; `dist/deployment.json` lets the same source resume without another version bump, and `make deploy` removes it only after publishing. Adding the Cask to the tap stays manual.

The fingerprint in the manifest (`xtask/src/source.rs`) is the git tree of the content with the version in `Cargo.toml` and in the workspace's entries of `Cargo.lock` replaced by a placeholder, with the HEAD commits of `../splat` and `../lowspeak` as gitlinks below `.siblings/` and, as `<name>-changes`, the tree of a sibling's working tree when that has changes.

Unlike Dango, Tapa has no `make ci` and no CI seals: Dango's deploy requires no seal either and only pushes a green one along when it has one, so Tapa's deploy pushes `main` and the tag alone. What a release was tested with is what `make check` and the self-tests showed in the session before it. A local agent skill `/deploy`, kept out of the repository, drives `make deploy`.
