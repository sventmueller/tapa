//! Sparkle (F75), the updater the release builds carry, where releases are downloaded from, and the update feed
//! installed copies read.

use std::path::{Path, PathBuf};
use std::process::Command;

use term_product::{NAME, UPDATE_PUBLIC_KEY};

use crate::*;

/// Sparkle's GitHub release, pinned by the checksum of its archive; the same as Splat's and Dango's.
pub(crate) const SPARKLE_VERSION: &str = "2.10.1";

const SPARKLE_SHA256: &str = "1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27";

/// Where Sparkle's tools keep the private key in the login keychain: their service and their default account, whose
/// key Splat and Lowspeak sign with too (the owner's decision of 2026-11-24), so one backup covers the three.
const UPDATE_KEY_SERVICE: &str = "https://sparkle-project.org";
pub(crate) const UPDATE_KEY_ACCOUNT: &str = "ed25519";

/// Where Sparkle looks for updates, and the public key every update must be signed with.
pub(crate) struct Feed<'a> {
    pub(crate) url: &'a str,
    pub(crate) key: &'a str,
}

/// The public key of a dry run, while `UPDATE_PUBLIC_KEY` is empty.
const DRY_RUN_UPDATE_KEY: &str = "DRY-RUN-NO-UPDATE-KEY";

const DRY_RUN_RELEASES: &str = "https://example.invalid/releases";

/// The feed's file name among a release's files.
pub(crate) const FEED: &str = "appcast.xml";

/// The oldest macOS the feed offers updates to (A2).
const MINIMUM_SYSTEM: &str = "14.0";

/// The releases page a run builds for, `term_product::RELEASES`.
pub(crate) fn releases(dry_run: bool) -> Result<&'static str, String> {
    releases_of(term_product::RELEASES, dry_run)
}

fn releases_of(releases: &'static str, dry_run: bool) -> Result<&'static str, String> {
    match (releases.is_empty(), dry_run) {
        (false, _) => Ok(releases),
        (true, true) => Ok(DRY_RUN_RELEASES),
        (true, false) => {
            Err("RELEASES in crates/term-product/src/lib.rs is empty: name where releases are downloaded".into())
        }
    }
}

/// Where installed copies look for updates: the feed is a file of the newest release, so publishing a release
/// publishes the update.
pub(crate) fn feed_url(releases: &str) -> String {
    format!("{releases}/latest/download/{FEED}")
}

/// The disk image's file name, without spaces, which a download address would have to escape.
pub(crate) fn image_name(version: &str) -> String {
    format!("{}-{version}.dmg", NAME.replace(' ', "-"))
}

/// The file name of the newest release's image, the same file under a name without the version, which the README's
/// download link stays on.
pub(crate) fn latest_image_name() -> String {
    format!("{}.dmg", NAME.replace(' ', "-"))
}

/// Where the disk image of `version` is downloaded from.
pub(crate) fn image_url(releases: &str, version: &str) -> String {
    format!("{releases}/download/v{version}/{}", image_name(version))
}

/// Sparkle's release archive, downloaded once into `~/.cache/tapa-tools`, checked against its pinned checksum and
/// unpacked: the framework and the `bin/` tools that sign updates.
pub(crate) fn sparkle(sh: &mut dyn Shell) -> Result<PathBuf, String> {
    sparkle_in(sh, &tools())
}

/// The folder `sparkle` unpacks into.
fn tools() -> PathBuf {
    home().join(".cache/tapa-tools")
}

/// `sparkle` with `target` in place of the tools folder.
fn sparkle_in(sh: &mut dyn Shell, target: &Path) -> Result<PathBuf, String> {
    let dir = target.join(format!("sparkle-{SPARKLE_VERSION}"));
    if dir.join("Sparkle.framework").exists() {
        return Ok(dir);
    }
    files::create_dir_all(target)?;
    let archive = target.join(format!("Sparkle-{SPARKLE_VERSION}.tar.xz"));
    let url = format!(
        "https://github.com/sparkle-project/Sparkle/releases/download/{SPARKLE_VERSION}/Sparkle-{SPARKLE_VERSION}.tar.xz"
    );
    sh.run(
        Command::new("curl")
            .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "-o"])
            .arg(&archive)
            .arg(&url),
    )?;
    let actual = sha256(sh, &archive)?;
    if actual != SPARKLE_SHA256 {
        files::remove_file(&archive)?;
        return Err(format!("{url} has checksum {actual}, expected {SPARKLE_SHA256}"));
    }
    let unpacking = target.join(format!("sparkle-{SPARKLE_VERSION}.partial"));
    if unpacking.exists() {
        files::remove_dir_all(&unpacking)?;
    }
    files::create_dir_all(&unpacking)?;
    sh.run(Command::new("tar").arg("-xf").arg(&archive).arg("-C").arg(&unpacking))?;
    files::rename(&unpacking, &dir)?;
    files::remove_file(&archive)?;
    Ok(dir)
}

/// The framework inside a bundle.
pub(crate) fn framework(app: &Path) -> PathBuf {
    app.join("Contents/Frameworks/Sparkle.framework")
}

/// Sparkle's programs inside a bundle that carries it, the framework's own code first.
pub(crate) fn sparkle_programs(app: &Path) -> Vec<PathBuf> {
    let code = framework(app).join("Versions/B");
    vec![
        code.join("Sparkle"),
        code.join("Autoupdate"),
        code.join("Updater.app/Contents/MacOS/Updater"),
    ]
}

/// Sparkle's code inside a bundle that carries it, innermost first, the order a signature must take instead of
/// `--deep`.
pub(crate) fn nested_code(app: &Path) -> Vec<PathBuf> {
    let framework = framework(app);
    if !framework.exists() {
        return Vec::new();
    }
    let code = framework.join("Versions/B");
    vec![code.join("Autoupdate"), code.join("Updater.app"), framework]
}

/// Puts the framework and its license into the bundle at `app`.
pub(crate) fn embed_sparkle(sh: &mut dyn Shell, app: &Path, sparkle: &Path) -> Result<(), String> {
    let framework = framework(app);
    files::create_dir_all(framework.parent().ok_or("no Frameworks folder")?)?;
    sh.run(
        Command::new("ditto")
            .arg(sparkle.join("Sparkle.framework"))
            .arg(&framework),
    )?;
    // Only a sandboxed app needs Sparkle's XPC services, and the terminal has no sandbox.
    files::remove_dir_all(framework.join("Versions/B/XPCServices"))?;
    files::remove_file(framework.join("XPCServices"))?;
    let license = sparkle.join("LICENSE");
    if license.exists() {
        files::copy(license, app.join("Contents/Resources/LICENSE-sparkle"))?;
    }
    Ok(())
}

/// The Info.plist keys that switch the updater on; none without a feed, and the app then starts no updater.
pub(crate) fn info_plist_keys(feed: Option<&Feed>) -> String {
    feed.map_or_else(String::new, |Feed { url, key }| {
        format!(
            "\t<key>SUFeedURL</key>\n\t<string>{url}</string>\n\t<key>SUPublicEDKey</key>\n\t<string>{key}</string>\n"
        )
    })
}

/// Whether the login keychain holds an update signing key; asks only for the item's attributes, which needs no
/// permission, so no dialog comes up.
pub(crate) fn update_key_present(sh: &mut dyn Shell) -> bool {
    sh.status(
        Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                UPDATE_KEY_SERVICE,
                "-a",
                UPDATE_KEY_ACCOUNT,
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
    )
    .is_ok_and(|status| status.success())
}

/// Sparkle's tool that creates and shows the update signing key, where `sparkle` unpacks it in `target`.
pub(crate) fn generate_keys(target: &Path) -> PathBuf {
    target.join(format!("sparkle-{SPARKLE_VERSION}/bin/generate_keys"))
}

/// What to do about a missing key: create it in the keychain, if `present` says it is not there, then copy its public
/// half into `UPDATE_PUBLIC_KEY`.
pub(crate) fn missing_key(present: bool) -> String {
    let tool = generate_keys(&tools()).display().to_string();
    let copy = format!(
        "put the public key it prints into UPDATE_PUBLIC_KEY in crates/term-product/src/lib.rs (`{tool} -p --account \
         {UPDATE_KEY_ACCOUNT}` prints it again)"
    );
    if present {
        format!("UPDATE_PUBLIC_KEY is empty: {copy}")
    } else {
        format!(
            "no update signing key under the keychain account {UPDATE_KEY_ACCOUNT}: import the one Splat and Lowspeak \
             sign with from its backup (`{tool} --account {UPDATE_KEY_ACCOUNT} -f <file>`), then {copy}"
        )
    }
}

/// The public key a run builds with: in a dry run `UPDATE_PUBLIC_KEY` or a placeholder; otherwise `UPDATE_PUBLIC_KEY`,
/// once the login keychain holds its private half. macOS may ask once whether this copy of `generate_keys` may read it.
pub(crate) fn update_key(sh: &mut dyn Shell, dry_run: bool) -> Result<&'static str, String> {
    update_key_of(sh, dry_run, UPDATE_PUBLIC_KEY, &tools())
}

/// `update_key` with `public` in place of `UPDATE_PUBLIC_KEY` and Sparkle unpacked in `target`.
fn update_key_of(
    sh: &mut dyn Shell,
    dry_run: bool,
    public: &'static str,
    target: &Path,
) -> Result<&'static str, String> {
    if dry_run {
        return Ok(if public.is_empty() { DRY_RUN_UPDATE_KEY } else { public });
    }
    let present = update_key_present(sh);
    if !present || public.is_empty() {
        return Err(missing_key(present));
    }
    sparkle_in(sh, target)?;
    let key = sh.stdout(Command::new(generate_keys(target)).args(["-p", "--account", UPDATE_KEY_ACCOUNT]))?;
    if key.trim() != public {
        return Err(format!(
            "the keychain's update signing key {} under {UPDATE_KEY_ACCOUNT} is not UPDATE_PUBLIC_KEY {public}",
            key.trim()
        ));
    }
    Ok(public)
}

/// The EdDSA signature and length of `image` for the feed, by the `sign_update` of the Sparkle release unpacked in
/// `sparkle`.
pub(crate) fn sign_update(sh: &mut dyn Shell, sparkle: &Path, image: &Path) -> Result<String, String> {
    let tool = sparkle.join("bin/sign_update");
    sh.stdout(Command::new(tool).args(["--account", UPDATE_KEY_ACCOUNT]).arg(image))
}

/// What a release's feed entry says.
pub(crate) struct Entry<'a> {
    pub(crate) version: &'a str,
    /// The disk image's download address.
    pub(crate) url: &'a str,
    /// The `sparkle:edSignature` and `length` attributes `sign_update` printed for the image.
    pub(crate) signature: &'a str,
    /// The release date as RFC 822 wants it.
    pub(crate) date: &'a str,
    /// The notes in each language, English first, as Markdown.
    pub(crate) notes: &'a [(String, String)],
}

/// The update feed: the release's disk image with its EdDSA signature and length, and its notes in every language
/// there are notes in; Sparkle shows them in the language macOS picks for the app. Macs older than `MINIMUM_SYSTEM`
/// are not offered it.
pub(crate) fn appcast(entry: &Entry) -> Result<String, String> {
    let signature = entry.signature;
    if !signature.starts_with("sparkle:edSignature=\"") || !signature.contains(" length=\"") {
        return Err(format!("sign_update printed {signature:?}"));
    }
    let mut descriptions = String::new();
    for (language, text) in entry.notes {
        if text.contains("]]>") {
            return Err(format!(
                "the {language} notes contain ]]>, which would end the feed's text early"
            ));
        }
        descriptions.push_str(&format!(
            "      <description sparkle:format=\"markdown\" xml:lang=\"{language}\"><![CDATA[{}]]></description>\n",
            text.trim_end()
        ));
    }
    let (version, date, url) = (entry.version, entry.date, entry.url);
    Ok(format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<rss version="2.0" xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
  <channel>
    <title>{NAME}</title>
    <item>
      <title>{NAME} {version}</title>
      <pubDate>{date}</pubDate>
      <sparkle:version>{version}</sparkle:version>
      <sparkle:shortVersionString>{version}</sparkle:shortVersionString>
      <sparkle:minimumSystemVersion>{MINIMUM_SYSTEM}</sparkle:minimumSystemVersion>
{descriptions}      <enclosure url="{url}" {signature} type="application/octet-stream"/>
    </item>
  </channel>
</rss>
"#
    ))
}

/// The SHA-256 checksum of a file, in hex.
pub(crate) fn sha256(sh: &mut dyn Shell, path: &Path) -> Result<String, String> {
    let listed = sh.stdout(Command::new("shasum").args(["-a", "256"]).arg(path))?;
    listed
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| format!("no checksum for {}", path.display()))
}

#[cfg(test)]
#[path = "updates_tests.rs"]
mod tests;
