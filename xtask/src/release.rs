//! The release (F75): the universal distribution, signed with the hardened runtime, packed into a disk image with a
//! link to /Applications, offered by the update feed and the Homebrew Cask.
//!
//! A dry run, the default, does all of it on this Mac: it signs as local builds are signed, with the Developer ID or
//! ad hoc, and without the secure timestamp, leaves notarization, stapling and the EdDSA signature out, writes into
//! `target/release-dry-run/` and leaves the version alone; nothing goes to Apple or anywhere else. `--notarize` signs with
//! the Developer ID, has Apple notarize the app and the image, staples both, signs the image for Sparkle with the
//! keychain's key and writes the release into `dist/`. Only deploy raises the version and uploads the release; adding the
//! Cask to a tap stays manual. The steps are Dango's; the update key is the one Splat and Lowspeak share.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use term_product::NAME;

use crate::bundle::{developer_id, languages, local_signer};
use crate::services::SERVICES;
use crate::universal::{check_universal, universal_dsym};
use crate::updates::{Entry, appcast, image_name, image_url};
use crate::*;

/// What a run does.
pub(crate) struct Options {
    /// Signs with the Developer ID and sends the app and the image to Apple's notary service.
    pub(crate) notarize: bool,
    /// Has Finder lay out the image's window, which opens it on screen; a notarized release always does.
    pub(crate) layout: bool,
}

/// Created once with `xcrun notarytool store-credentials notary --apple-id <Apple ID> --team-id <team ID>`, the same
/// profile Splat, Lowspeak and Dango use.
pub(crate) const NOTARY_PROFILE: &str = "notary";

/// What the release brings, written by hand before a notarized run.
const NOTES: &str = "RELEASE_NOTES.md";

/// The folder with one subfolder per other interface language, which holds `NOTES` in that language beside the README.
const TRANSLATED_NOTES: &str = "assets/readme";

/// Where the notes in `language`, other than English, are written.
fn translated_notes(language: &str) -> String {
    format!("{TRANSLATED_NOTES}/{language}/{NOTES}")
}

/// How notes waiting for the owner's review begin: an HTML comment, which a notarized release refuses and a dry run
/// leaves out of the feed.
const DRAFT: &str = "<!-- DRAFT";

/// Builds the release and returns the folder that holds the disk image, its notes, the feed and the Cask.
pub(crate) fn release(options: Options) -> Result<PathBuf, String> {
    release_version(options, &version()?)
}

/// Builds a particular version, the one in `Cargo.toml`, which the programs carry too; deploy writes the next one there
/// first and commits it only once notarization succeeded.
pub(crate) fn release_version(options: Options, version: &str) -> Result<PathBuf, String> {
    let compiled = crate::version()?;
    if compiled != version {
        return Err(format!(
            "Cargo.toml holds {compiled}, so the programs could not carry {version}"
        ));
    }
    let sh: &mut dyn Shell = &mut System;
    let dry_run = !options.notarize;
    let releases = updates::releases(dry_run)?;
    let homepage = cask::homepage(dry_run)?;
    let out = if dry_run {
        target_dir().join("release-dry-run")
    } else {
        root().join("dist")
    };
    // The notes must be new against the release before, which only `dist/` holds; a dry run may repeat itself.
    let previous_feed = (!dry_run)
        .then(|| std::fs::read_to_string(out.join(updates::FEED)).ok())
        .flatten();
    let notes = notes(&root(), dry_run, previous_feed.as_deref())?;
    let (identity, name) = if dry_run {
        print!("{}", preflight(sh));
        local_signer(sh)
    } else {
        developer_id(sh)?
    };
    let key = updates::update_key(sh, dry_run)?;
    if !dry_run {
        check_notary_profile(sh)?;
    }
    check_translations(sh)?;
    let feed = updates::feed_url(releases);
    let app = bundle::distribution(version, &updates::Feed { url: &feed, key })?;
    println!("signing {version} with {name}");
    let sparkle = updates::sparkle(sh)?;
    let (image, signature) = package(
        sh,
        &app,
        &Signing {
            identity: &identity,
            notarize: options.notarize,
            layout: options.layout,
            sparkle: &sparkle,
            mount: &target_dir().join("release-dry-run-mount"),
        },
    )?;
    publish(
        sh,
        &out,
        dry_run,
        &Published {
            version,
            image: &image,
            signature: signature.trim(),
            releases,
            homepage,
            notes: &notes,
        },
    )?;
    let symbols = keep_dsyms(sh, &out, version)?;
    println!("debug symbols in {}", symbols.display());
    Ok(out)
}

/// Copies the debug symbols of the distributed programs beside the release, outside the image, where crash reports of
/// this version are symbolicated from: `<out>/<NAME>-<version>-dSYMs/<program>.dSYM`.
fn keep_dsyms(sh: &mut dyn Shell, out: &Path, version: &str) -> Result<PathBuf, String> {
    let kept = out.join(format!("{NAME}-{version}-dSYMs"));
    for name in std::iter::once(PROGRAM).chain(SERVICES.iter().map(|service| service.program)) {
        sh.run(
            Command::new("ditto")
                .arg(universal_dsym(name))
                .arg(kept.join(format!("{name}.dSYM"))),
        )?;
    }
    Ok(kept)
}

/// How `package` signs and checks the release.
struct Signing<'a> {
    identity: &'a str,
    /// Signs with the secure timestamp, has Apple notarize the app and the image and signs the image for Sparkle.
    notarize: bool,
    layout: bool,
    /// The unpacked Sparkle release, whose `sign_update` signs the image.
    sparkle: &'a Path,
    /// Where `check_image` mounts the image.
    mount: &'a Path,
}

/// Signs the assembled app, packs it into the disk image, signs and checks that, and returns it with the signature
/// and length the feed offers it with.
fn package(sh: &mut dyn Shell, app: &Path, signing: &Signing) -> Result<(PathBuf, String), String> {
    check_universal(sh, app)?;
    bundle::sign(sh, app, signing.identity, signing.notarize)?;
    if signing.notarize {
        notarize_app(sh, app)?;
    }
    let image = disk_image(sh, app, signing.layout)?;
    sh.run(
        Command::new("codesign")
            .args(["--force", "--sign", signing.identity])
            .arg(if signing.notarize {
                "--timestamp"
            } else {
                "--timestamp=none"
            })
            .arg(&image),
    )?;
    check_image(sh, &image, signing.mount)?;
    if !signing.notarize {
        // Sparkle's tool would need the private key from the keychain; the length is real, the signature not.
        let length = std::fs::metadata(&image)
            .map_err(|e| format!("{}: {e}", image.display()))?
            .len();
        return Ok((
            image,
            format!("sparkle:edSignature=\"DRY-RUN-NOT-SIGNED\" length=\"{length}\""),
        ));
    }
    notarize(sh, &image)?;
    sh.run(Command::new("xcrun").args(["stapler", "staple"]).arg(&image))?;
    assess(sh, &image)?;
    let signature = updates::sign_update(sh, signing.sparkle, &image)?;
    Ok((image, signature))
}

/// What a notarized release needs from this Mac and the owner, and whether it is there, for a dry run to print. Asks
/// the keychain only for items' attributes and Apple for nothing.
fn preflight(sh: &mut dyn Shell) -> String {
    let mark = |ok: bool| if ok { "ok     " } else { "MISSING" };
    let developer = developer_id(sh);
    let key_present = updates::update_key_present(sh);
    [
        "dry run; a release with --notarize needs:".to_string(),
        format!(
            "  {} Developer ID Application certificate{}",
            mark(developer.is_ok()),
            developer.map(|(_, name)| format!(": {name}")).unwrap_or_default()
        ),
        format!("  ?       notarytool profile \"{NOTARY_PROFILE}\" (checked by a notarized run, which asks Apple)"),
        format!(
            "  {} Sparkle update key of Splat and Lowspeak under the keychain account {}",
            mark(key_present),
            updates::UPDATE_KEY_ACCOUNT,
        ),
        format!(
            "  {} UPDATE_PUBLIC_KEY in crates/term-product/src/lib.rs, the public half of that key",
            mark(!term_product::UPDATE_PUBLIC_KEY.is_empty())
        ),
        format!(
            "  {} RELEASES in crates/term-product/src/lib.rs",
            mark(!term_product::RELEASES.is_empty())
        ),
        format!(
            "  {} HOMEPAGE in crates/term-product/src/lib.rs",
            mark(!term_product::HOMEPAGE.is_empty())
        ),
        format!(
            "  {} {NOTES} and {TRANSLATED_NOTES}/<language>/{NOTES}",
            mark(root().join(NOTES).exists())
        ),
    ]
    .map(|line| line + "\n")
    .concat()
}

/// Runs `cargo xtask strings`, which joins the crates' texts into `resources/` and fails on a key without an English
/// entry or without a translation in one of the interface languages; a release stops on it before the universal build.
fn check_translations(sh: &mut dyn Shell) -> Result<(), String> {
    println!("checking that every interface language translates every text");
    sh.run(&mut translation_check()).map_err(|e| {
        format!(
            "{e}\nthe interface is not translated completely; the output above names each text, and `cargo xtask \
             strings` runs the check again"
        )
    })
}

/// This build tool's own `strings` task.
fn translation_check() -> Command {
    let xtask = std::env::current_exe().unwrap_or_else(|_| "xtask".into());
    let mut command = Command::new(xtask);
    command.current_dir(root()).arg("strings");
    command
}

/// The notes in English from `NOTES`, then in each other interface language from `TRANSLATED_NOTES`. A notarized
/// release needs every one of them, reviewed and new since `previous_feed`, and names all that are missing at once; a
/// dry run makes do with what there is and says what is missing or still a draft.
fn notes(root: &Path, dry_run: bool, previous_feed: Option<&str>) -> Result<Vec<(String, String)>, String> {
    let languages = languages(&root.join("resources"));
    let others = languages
        .iter()
        .filter(|l| *l != "en")
        .map(|l| (l.clone(), translated_notes(l)));
    let (mut notes, mut missing) = (Vec::new(), Vec::new());
    for (language, file) in std::iter::once(("en".to_string(), NOTES.to_string())).chain(others) {
        let Ok(written) = std::fs::read_to_string(root.join(&file)) else {
            missing.push(language);
            continue;
        };
        let (text, draft) = without_draft(&written);
        if draft && !dry_run {
            return Err(format!(
                "{file} is still a draft: review it, then delete the comment it begins with ({DRAFT} …)"
            ));
        }
        if draft {
            println!("dry run with {file}, still a draft");
        }
        notes.push((language, checked_notes(&file, text, previous_feed)?));
    }
    if !missing.is_empty() {
        let list = missing.join(", ");
        if !dry_run {
            return Err(format!(
                "release notes missing for {list}; the update dialog shows them in every interface language: write \
                 {NOTES} for en and {TRANSLATED_NOTES}/<language>/{NOTES} for the others"
            ));
        }
        println!("dry run without release notes for {list}");
    }
    if notes.first().is_none_or(|(language, _)| language != "en") {
        notes.insert(0, ("en".to_string(), "Dry run.\n".to_string()));
    }
    Ok(notes)
}

/// `written` without the draft comment it begins with, and whether it had one.
fn without_draft(written: &str) -> (&str, bool) {
    let trimmed = written.trim_start();
    if !trimmed.starts_with(DRAFT) {
        return (written, false);
    }
    match trimmed.find("-->") {
        Some(end) => (&trimmed[end + 3..], true),
        None => ("", true),
    }
}

/// The notes written in `file`, neither empty nor those of the release before.
fn checked_notes(file: &str, written: &str, previous_feed: Option<&str>) -> Result<String, String> {
    let written = written.trim();
    if written.is_empty() {
        return Err(format!("{file} is empty; write what this release brings"));
    }
    if previous_feed.is_some_and(|feed| feed.contains(written)) {
        return Err(format!("{file} still holds the notes of the previous release"));
    }
    Ok(format!("{written}\n"))
}

/// Checks the saved notary login before the universal build, which takes many minutes.
fn check_notary_profile(sh: &mut dyn Shell) -> Result<(), String> {
    sh.run(
        Command::new("xcrun")
            .args(["notarytool", "history", "--keychain-profile", NOTARY_PROFILE])
            .stdout(Stdio::null()),
    )
    .map_err(|e| {
        format!(
            "{e}\nno usable notarization login in the keychain profile \"{NOTARY_PROFILE}\"; create it with \
             `xcrun notarytool store-credentials {NOTARY_PROFILE} --apple-id <Apple ID> --team-id <team ID>`"
        )
    })
}

/// Has Apple notarize the app and staples the ticket to it. The image's own ticket reaches Gatekeeper only when the
/// image is opened in the Finder; Homebrew copies the app out otherwise, which would need a connection on first launch.
fn notarize_app(sh: &mut dyn Shell, app: &Path) -> Result<(), String> {
    let archive = app.with_extension("zip");
    sh.run(
        Command::new("ditto")
            .args(["-c", "-k", "--keepParent"])
            .arg(app)
            .arg(&archive),
    )?;
    notarize(sh, &archive)?;
    files::remove_file(&archive)?;
    sh.run(Command::new("xcrun").args(["stapler", "staple"]).arg(app))
}

/// Uploads a disk image or zip archive to Apple's notary service and waits for the verdict; on a rejection it prints
/// the service's log, which names each problem.
fn notarize(sh: &mut dyn Shell, file: &Path) -> Result<(), String> {
    println!("waiting for Apple's notary service, usually a few minutes");
    let mut submit = Command::new("xcrun");
    submit
        .args([
            "notarytool",
            "submit",
            "--keychain-profile",
            NOTARY_PROFILE,
            "--wait",
            "--output-format",
            "json",
        ])
        .arg(file)
        .stderr(Stdio::inherit());
    let out = sh.output(&mut submit).map_err(|e| format!("{submit:?}: {e}"))?;
    let result = String::from_utf8_lossy(&out.stdout);
    let status = json_string(&result, "status");
    if status == Some("Accepted") && out.status.success() {
        return Ok(());
    }
    if let Some(id) = json_string(&result, "id") {
        let _ = sh.run(Command::new("xcrun").args(["notarytool", "log", id, "--keychain-profile", NOTARY_PROFILE]));
    }
    Err(format!(
        "notarization failed: status {}, notarytool {}",
        status.unwrap_or("unknown"),
        out.status
    ))
}

/// The value of a top-level string field in notarytool's flat JSON output.
fn json_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let rest = &json[json.find(&format!("\"{key}\""))? + key.len() + 2..];
    let rest = rest.trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
    rest.split('"').next()
}

/// Gatekeeper's verdict on the stapled image, as a Mac that downloaded it gives it.
fn assess(sh: &mut dyn Shell, image: &Path) -> Result<(), String> {
    sh.run(
        Command::new("spctl")
            .args([
                "--assess",
                "--type",
                "open",
                "--context",
                "context:primary-signature",
                "--verbose",
            ])
            .arg(image),
    )
}

/// Checks the image as far as this Mac can on its own: it is intact, and the app in it is signed, universal and beside
/// a link to /Applications. Mounted without showing up in the Finder.
fn check_image(sh: &mut dyn Shell, image: &Path, mount: &Path) -> Result<(), String> {
    sh.run(Command::new("hdiutil").arg("verify").arg(image).stdout(Stdio::null()))?;
    if mount.exists() {
        let _ = sh.run(
            Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(mount)
                .stdout(Stdio::null()),
        );
    }
    files::create_dir_all(mount)?;
    sh.run(
        Command::new("hdiutil")
            .args([
                "attach",
                "-readonly",
                "-nobrowse",
                "-noverify",
                "-noautoopen",
                "-mountpoint",
            ])
            .arg(mount)
            .arg(image)
            .stdout(Stdio::null()),
    )?;
    let app = mount.join(format!("{NAME}.app"));
    let mut checked = || {
        sh.run(
            Command::new("codesign")
                .args(["--verify", "--deep", "--strict"])
                .arg(&app),
        )?;
        check_universal(sh, &app)?;
        let link = std::fs::read_link(mount.join("Applications")).map_err(|e| format!("Applications link: {e}"))?;
        if link != Path::new("/Applications") {
            return Err(format!("the Applications link leads to {}", link.display()));
        }
        Ok(())
    };
    let checked = checked();
    sh.run(Command::new("hdiutil").arg("detach").arg(mount).stdout(Stdio::null()))?;
    checked
}

/// What goes into the release folder.
struct Published<'a> {
    version: &'a str,
    image: &'a Path,
    signature: &'a str,
    releases: &'a str,
    homepage: &'a str,
    notes: &'a [(String, String)],
}

/// Writes the image, the English notes, the feed and the Cask into `out`, a dry run's emptied first, and checks the
/// feed is well-formed XML and the Cask valid Ruby.
fn publish(sh: &mut dyn Shell, out: &Path, dry_run: bool, release: &Published) -> Result<(), String> {
    if dry_run && out.exists() {
        files::remove_dir_all(out)?;
    }
    files::create_dir_all(out)?;
    let image = out.join(image_name(release.version));
    // Copied rather than renamed: target/ may be a link to another volume.
    files::copy(release.image, &image)?;
    files::remove_file(release.image)?;
    let english = release.notes.first().map_or("", |(_, text)| text.as_str());
    files::write(out.join(notes_name(release.version)), english)?;
    let date = sh.stdout(
        Command::new("date")
            .env("LC_ALL", "C")
            .args(["-u", "+%a, %d %b %Y %H:%M:%S +0000"]),
    )?;
    let url = image_url(release.releases, release.version);
    let feed = appcast(&Entry {
        version: release.version,
        url: &url,
        signature: release.signature,
        date: date.trim(),
        notes: release.notes,
    })?;
    let feed_file = out.join(updates::FEED);
    files::write(&feed_file, feed)?;
    sh.run(Command::new("xmllint").arg("--noout").arg(&feed_file))?;
    let cask_file = out.join(cask::file_name());
    files::write(
        &cask_file,
        cask::cask(&cask::Release {
            version: release.version,
            sha256: &updates::sha256(sh, &image)?,
            releases: release.releases,
            homepage: release.homepage,
        }),
    )?;
    sh.run(Command::new("ruby").arg("-c").arg(&cask_file).stdout(Stdio::null()))
}

/// The English notes' file name among a release's files, which the GitHub release shows.
pub(crate) fn notes_name(version: &str) -> String {
    format!("{}-{version}.md", NAME.replace(' ', "-"))
}

/// Packs the app with a link to /Applications into a compressed, read-only image next to it; with `layout`, Finder
/// sets its window up first.
fn disk_image(sh: &mut dyn Shell, app: &Path, layout: bool) -> Result<PathBuf, String> {
    let out = app.parent().ok_or("the app has no parent folder")?;
    let staging = out.join("dmg-staging");
    let writable = out.join(format!("{NAME}-writable.dmg"));
    let image = out.join(format!("{NAME}.dmg"));
    for file in [&writable, &image] {
        if file.exists() {
            files::remove_file(file)?;
        }
    }
    if staging.exists() {
        files::remove_dir_all(&staging)?;
    }
    files::create_dir_all(&staging)?;
    sh.run(Command::new("ditto").arg(app).arg(staging.join(format!("{NAME}.app"))))?;
    std::os::unix::fs::symlink("/Applications", staging.join("Applications"))
        .map_err(|e| format!("{}: {e}", staging.display()))?;
    sh.run(
        Command::new("hdiutil")
            .args(["create", "-volname", NAME, "-format", "UDRW", "-ov", "-srcfolder"])
            .arg(&staging)
            .arg(&writable)
            .stdout(Stdio::null()),
    )?;
    if layout {
        lay_out_image(sh, &writable)?;
    }
    sh.run(
        Command::new("hdiutil")
            .args(["convert", "-format", "UDZO", "-imagekey", "zlib-level=9", "-o"])
            .arg(&image)
            .arg(&writable)
            .stdout(Stdio::null()),
    )?;
    files::remove_file(&writable)?;
    files::remove_dir_all(&staging)?;
    Ok(image)
}

/// Mounts the writable image where Finder sees it, has Finder lay out its window and unmounts it again.
fn lay_out_image(sh: &mut dyn Shell, writable: &Path) -> Result<(), String> {
    let attached = sh.stdout(
        Command::new("hdiutil")
            .args(["attach", "-readwrite", "-noverify", "-noautoopen"])
            .arg(writable),
    )?;
    let mount = mount_point(&attached).ok_or("hdiutil attach reported no mount point")?;
    let laid_out = lay_out(sh, &root().join("assets/dmg-background.tiff"), &mount);
    let detached = sh.run(Command::new("hdiutil").arg("detach").arg(&mount).stdout(Stdio::null()));
    if detached.is_err() {
        sh.run(
            Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(&mount)
                .stdout(Stdio::null()),
        )?;
    }
    laid_out.map_err(|e| format!("laying out the disk image window: {e}"))
}

/// Where `hdiutil attach` says it mounted the image.
fn mount_point(attached: &str) -> Option<PathBuf> {
    attached
        .lines()
        .find_map(|l| l.split('\t').map(str::trim).find(|f| f.starts_with("/Volumes/")))
        .map(PathBuf::from)
}

/// The centres of the two icons in points from the top left of the window's content, either side of the arrow in
/// the background.
const APP_ICON_AT: (u32, u32) = (120, 150);
const APPLICATIONS_ICON_AT: (u32, u32) = (380, 150);

/// Finder keeps window size, icon positions and background in the image's .DS_Store, so only Finder itself can
/// write them, on the mounted image. Needs the Automation permission for Finder.
///
/// `assets/dmg-background.tiff` holds `assets/dmg-background.svg`, Splat's and Dango's, rendered by resvg at 500 × 300 and
/// 1000 × 600 pixels, joined with `tiffutil -cathidpicheck`; with a single 500 × 300 image, Retina screens show it
/// blurred.
fn lay_out(sh: &mut dyn Shell, background: &Path, mount: &Path) -> Result<(), String> {
    files::create_dir_all(mount.join(".background"))?;
    files::copy(background, mount.join(".background/background.tiff"))?;
    let disk = mount
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("mount point without a name")?;
    sh.run(Command::new("osascript").args(["-e", &finder_script(disk)]))?;
    // Finder saves the .DS_Store in the background once the window has closed.
    let store = mount.join(".DS_Store");
    let mut checked = Err(format!("Finder wrote no {}", store.display()));
    for _ in 0..20 {
        if let Ok(bytes) = std::fs::read(&store) {
            checked = check_layout(&bytes);
            if checked.is_ok() {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    checked
}

fn finder_script(disk: &str) -> String {
    format!(
        r#"tell application "Finder"
  tell disk "{disk}"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {{400, 200, 900, 500}}
    set viewOptions to the icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 96
    set background picture of viewOptions to file ".background:background.tiff"
    set position of item "{NAME}.app" of container window to {{{app_x}, {app_y}}}
    set position of item "Applications" of container window to {{{applications_x}, {applications_y}}}
    close
    open
    update without registering applications
    delay 1
    close
  end tell
end tell"#,
        app_x = APP_ICON_AT.0,
        app_y = APP_ICON_AT.1,
        applications_x = APPLICATIONS_ICON_AT.0,
        applications_y = APPLICATIONS_ICON_AT.1,
    )
}

/// Checks the layout Finder stored, which is what every Mac opening the image shows.
fn check_layout(store: &[u8]) -> Result<(), String> {
    for hidden in [".background", ".fseventsd"] {
        if icon_position(store, hidden).is_some() {
            return Err(format!(
                "Finder placed the hidden {hidden} in the window, which it does only while it shows hidden files; \
                 hide them with ⇧⌘. in a Finder window and build again"
            ));
        }
    }
    for (name, expected) in [
        (format!("{NAME}.app"), APP_ICON_AT),
        ("Applications".to_string(), APPLICATIONS_ICON_AT),
    ] {
        let found = icon_position(store, &name);
        if found != Some(expected) {
            return Err(format!("Finder placed {name} at {found:?} instead of {expected:?}"));
        }
    }
    Ok(())
}

/// An icon's place in a .DS_Store: an `Iloc` record after the file name in UTF-16, holding a 16-byte `blob` that
/// starts with x and y as big-endian integers.
fn icon_position(store: &[u8], name: &str) -> Option<(u32, u32)> {
    let mut key: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
    key.extend_from_slice(b"Ilocblob");
    let start = store.windows(key.len()).position(|window| window == key)? + key.len() + 4;
    let coordinate = |at: usize| {
        let bytes = store.get(at..at + 4)?;
        Some(u32::from_be_bytes(bytes.try_into().ok()?))
    };
    Some((coordinate(start)?, coordinate(start + 4)?))
}

#[cfg(test)]
#[path = "release_tests.rs"]
mod package_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notes_must_be_new_and_not_empty() {
        assert_eq!(checked_notes(NOTES, "- New.\n\n", None).unwrap(), "- New.\n");
        assert!(checked_notes(NOTES, " \n", None).is_err());
        let feed = "<![CDATA[- New.]]>";
        let stale = checked_notes(&translated_notes("de"), "- New.\n", Some(feed)).unwrap_err();
        assert!(stale.contains("assets/readme/de/RELEASE_NOTES.md"), "{stale}");
        assert!(checked_notes(NOTES, "- Newer.\n", Some(feed)).is_ok());
    }

    #[test]
    fn a_draft_comment_is_left_out_and_marked() {
        let draft = "<!-- DRAFT for review,\nover two lines. -->\nT 0.1.0 is the first release.\n";
        assert_eq!(without_draft(draft), ("\nT 0.1.0 is the first release.\n", true));
        assert_eq!(without_draft("- New.\n"), ("- New.\n", false));
        assert_eq!(without_draft("<!-- DRAFT never closed"), ("", true));
        assert!(checked_notes(NOTES, without_draft("<!-- DRAFT -->\n").0, None).is_err());
    }

    /// Every interface language has notes, and they read as notes once their draft comment is gone.
    #[test]
    fn the_drafted_notes_are_notes() {
        let languages = languages(&root().join("resources"));
        assert_eq!(languages.len(), 35);
        let files = languages.iter().map(|language| match language.as_str() {
            "en" => NOTES.to_string(),
            other => translated_notes(other),
        });
        for file in files {
            let written = files::read_to_string(root().join(&file)).unwrap();
            let (text, _) = without_draft(&written);
            let notes = checked_notes(&file, text, None).unwrap();
            assert!(!notes.contains("]]>") && !notes.contains("<!--"), "{file}");
        }
    }

    #[test]
    fn the_release_checks_every_language_with_this_tools_strings_task() {
        let command = translation_check();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(args, ["strings"]);
        assert_eq!(command.get_program(), std::env::current_exe().unwrap().as_os_str());
        assert_eq!(command.get_current_dir(), Some(root().as_path()));
    }

    #[test]
    fn the_notary_verdict_is_read_from_its_json() {
        let json = r#"{"id":"2b1c-77","message":"Processing complete","status":"Invalid"}"#;
        assert_eq!(json_string(json, "status"), Some("Invalid"));
        assert_eq!(json_string(json, "id"), Some("2b1c-77"));
        assert_eq!(json_string(json, "missing"), None);
    }

    fn placed(name: &str, (x, y): (u32, u32)) -> Vec<u8> {
        let mut record = u32::try_from(name.encode_utf16().count())
            .unwrap()
            .to_be_bytes()
            .to_vec();
        record.extend(name.encode_utf16().flat_map(u16::to_be_bytes));
        record.extend_from_slice(b"Ilocblob");
        record.extend_from_slice(&16u32.to_be_bytes());
        record.extend_from_slice(&x.to_be_bytes());
        record.extend_from_slice(&y.to_be_bytes());
        record.extend_from_slice(&[0xff; 8]);
        record
    }

    #[test]
    fn the_stored_layout_must_match_the_background() {
        let app = format!("{NAME}.app");
        let intended = [placed(&app, APP_ICON_AT), placed("Applications", APPLICATIONS_ICON_AT)].concat();
        assert_eq!(icon_position(&intended, &app), Some(APP_ICON_AT));
        assert!(check_layout(&intended).is_ok());
        let hidden = [placed(".background", (95, 64)), intended.clone()].concat();
        assert!(check_layout(&hidden).unwrap_err().contains("hidden .background"));
        let moved = [placed(&app, (120, 177)), placed("Applications", APPLICATIONS_ICON_AT)].concat();
        assert!(check_layout(&moved).is_err());
        assert!(finder_script("Disk").contains(&format!("item \"{app}\"")));
    }

    #[test]
    fn finder_lays_out_the_window_on_the_background_and_its_store_is_checked() {
        let mount = crate::shell::fake::scratch("release-lay-out");
        let background = root().join("assets/dmg-background.tiff");
        let store = mount.join(".DS_Store");
        let mut sh = crate::shell::fake::Fake::new(|_: &Command, _: &str| {
            let app = format!("{NAME}.app");
            let laid_out = [placed(&app, APP_ICON_AT), placed("Applications", APPLICATIONS_ICON_AT)].concat();
            std::fs::write(&store, laid_out).unwrap();
            (0, String::new())
        });
        lay_out(&mut sh, &background, &mount).unwrap();
        assert_eq!(
            std::fs::read(mount.join(".background/background.tiff")).unwrap(),
            std::fs::read(&background).unwrap()
        );
        let disk = mount.file_name().unwrap().to_str().unwrap();
        assert_eq!(sh.commands, [format!("osascript -e {}", finder_script(disk))]);
        std::fs::remove_dir_all(mount).unwrap();
    }

    #[test]
    fn the_background_the_layout_needs_is_there() {
        assert!(root().join("assets/dmg-background.tiff").is_file());
    }
}
