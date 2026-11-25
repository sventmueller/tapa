//! The app bundle: the program, its property list with the services programs in the terminal may ask for (F217), the
//! addresses and files it opens (F216), its texts and themes, and an ad-hoc signature for this Mac.

use std::path::{Path, PathBuf};
use std::process::Command;

use term_product::{BUNDLE_ID, NAME, URL_SCHEME};

use crate::services::SERVICES;
use crate::universal::universal_programs;
use crate::*;

/// The oldest macOS the app runs on (A2).
const MINIMUM_SYSTEM: &str = "14.0";

/// What macOS shows when a program in the terminal asks for a protected service; macOS charges the terminal with
/// whatever its children do, so without these keys their requests fail silently (F217).
pub(crate) const USAGE: &[(&str, &str)] = &[
    (
        "NSAppleEventsUsageDescription",
        "A program running in the terminal wants to control another app.",
    ),
    (
        "NSBluetoothAlwaysUsageDescription",
        "A program running in the terminal wants to use Bluetooth.",
    ),
    (
        "NSCalendarsUsageDescription",
        "A program running in the terminal wants to read your calendars.",
    ),
    (
        "NSCameraUsageDescription",
        "A program running in the terminal wants to use the camera.",
    ),
    (
        "NSContactsUsageDescription",
        "A program running in the terminal wants to read your contacts.",
    ),
    (
        "NSDesktopFolderUsageDescription",
        "A program running in the terminal wants to read files on the desktop.",
    ),
    (
        "NSDocumentsFolderUsageDescription",
        "A program running in the terminal wants to read your documents.",
    ),
    (
        "NSDownloadsFolderUsageDescription",
        "A program running in the terminal wants to read your downloads.",
    ),
    (
        "NSLocalNetworkUsageDescription",
        "A program running in the terminal wants to reach devices on your network.",
    ),
    (
        "NSLocationUsageDescription",
        "A program running in the terminal wants to know your location.",
    ),
    (
        "NSMicrophoneUsageDescription",
        "Dictation, and programs running in the terminal, use the microphone.",
    ),
    (
        "NSNetworkVolumesUsageDescription",
        "A program running in the terminal wants to read files on network volumes.",
    ),
    (
        "NSPhotoLibraryUsageDescription",
        "A program running in the terminal wants to read your photos.",
    ),
    (
        "NSRemindersUsageDescription",
        "A program running in the terminal wants to read your reminders.",
    ),
    (
        "NSRemovableVolumesUsageDescription",
        "A program running in the terminal wants to read files on removable volumes.",
    ),
    (
        "NSSpeechRecognitionUsageDescription",
        "A program running in the terminal wants to recognise speech.",
    ),
    (
        "NSSystemAdministrationUsageDescription",
        "A program running in the terminal wants to change system settings.",
    ),
];

/// The interface languages the app ships, each a `<language>.lproj` folder in `resources`, sorted.
pub(crate) fn languages(resources: &Path) -> Vec<String> {
    let mut found: Vec<String> = std::fs::read_dir(resources)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.strip_suffix(".lproj").map(str::to_string))
        .collect();
    found.sort();
    found
}

/// The property list of the bundle; `feed` switches Sparkle on.
pub(crate) fn info_plist(
    bundle_id: &str,
    name: &str,
    version: &str,
    scheme: &str,
    feed: Option<&updates::Feed>,
) -> String {
    let updates = updates::info_plist_keys(feed);
    let mut usage = String::new();
    for (key, text) in USAGE {
        usage.push_str(&format!("\t<key>{key}</key>\n\t<string>{text}</string>\n"));
    }
    let languages: String = languages(&root().join("resources"))
        .iter()
        .map(|language| format!("\t\t<string>{language}</string>\n"))
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleDevelopmentRegion</key>
	<string>de</string>
	<key>CFBundleLocalizations</key>
	<array>
{languages}	</array>
	<key>CFBundleExecutable</key>
	<string>tapa</string>
	<key>CFBundleIdentifier</key>
	<string>{bundle_id}</string>
	<key>CFBundleName</key>
	<string>{name}</string>
	<key>CFBundleDisplayName</key>
	<string>{name}</string>
	<key>CFBundleIconFile</key><string>AppIcon</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
	<key>CFBundleVersion</key>
	<string>{version}</string>
	<key>LSMinimumSystemVersion</key>
	<string>{MINIMUM_SYSTEM}</string>
	<key>LSApplicationCategoryType</key>
	<string>public.app-category.developer-tools</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSPrincipalClass</key>
	<string>NSApplication</string>
	<key>NSSupportsAutomaticTermination</key>
	<false/>
	<key>NSSupportsSuddenTermination</key>
	<false/>
	<key>CFBundleURLTypes</key>
	<array>
		<dict>
			<key>CFBundleURLName</key>
			<string>Secure Shell</string>
			<key>CFBundleURLSchemes</key>
			<array>
				<string>ssh</string>
			</array>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
		</dict>
		<dict>
			<key>CFBundleURLName</key>
			<string>{name}</string>
			<key>CFBundleURLSchemes</key>
			<array>
				<string>{scheme}</string>
			</array>
		</dict>
	</array>
	<key>CFBundleDocumentTypes</key>
	<array>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Shell Script</string>
			<key>CFBundleTypeRole</key>
			<string>Shell</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
				<string>com.apple.terminal.shell-script</string>
				<string>public.unix-executable</string>
				<string>public.shell-script</string>
			</array>
		</dict>
		<dict>
			<key>CFBundleTypeName</key>
			<string>Folder</string>
			<key>CFBundleTypeRole</key>
			<string>Shell</string>
			<key>LSHandlerRank</key>
			<string>Alternate</string>
			<key>LSItemContentTypes</key>
			<array>
				<string>public.folder</string>
			</array>
		</dict>
	</array>
{usage}{updates}</dict>
</plist>
"#
    )
}

/// What a bundle is called and answers to.
pub(crate) struct Identity {
    pub(crate) name: String,
    pub(crate) bundle_id: String,
    pub(crate) scheme: String,
}

impl Identity {
    /// The release's, which the distribution shares, or the debug build's of its own.
    pub(crate) fn of(release: bool) -> Identity {
        if release {
            Identity {
                name: NAME.to_string(),
                bundle_id: BUNDLE_ID.to_string(),
                scheme: URL_SCHEME.to_string(),
            }
        } else {
            Identity {
                name: format!("{NAME} Debug"),
                bundle_id: format!("{BUNDLE_ID}.debug"),
                scheme: format!("{URL_SCHEME}-debug"),
            }
        }
    }
}

/// The app's program inside a bundle.
pub(crate) fn executable(app: &Path) -> PathBuf {
    app.join("Contents/MacOS").join(PROGRAM)
}

/// The app's program and its services' programs, in the order `lay_out` takes them.
pub(crate) fn program_names() -> Vec<&'static str> {
    std::iter::once(PROGRAM)
        .chain(SERVICES.iter().map(|service| service.program))
        .collect()
}

/// Builds the program and puts it into `target/<profile>/<name>.app`; the bundle's path.
pub(crate) fn bundle(release: bool) -> Result<PathBuf, String> {
    let mut cargo = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cargo.current_dir(root()).args(["build", "-p", PACKAGE]);
    for name in program_names() {
        cargo.args(["--bin", name]);
    }
    if release {
        // Dictation builds whisper.cpp with cmake, which the debug app and its self-test leave out.
        cargo.args(["--release", "--features", "dictation"]);
    }
    if std::env::args().any(|arg| matches!(arg.as_str(), "--selftest" | "selftest" | "esctest" | "vttest")) {
        cargo.args(["--features", "selftest"]);
    }
    run(&mut cargo)?;
    let profile = if release { "release" } else { "debug" };
    let identity = Identity::of(release);
    let app = target_dir().join(profile).join(format!("{}.app", identity.name));
    let programs: Vec<PathBuf> = program_names()
        .iter()
        .map(|name| target_dir().join(profile).join(name))
        .collect();
    let url = term_product::feed_url().filter(|_| release);
    let feed = url.as_deref().map(|url| updates::Feed {
        url,
        key: term_product::UPDATE_PUBLIC_KEY,
    });
    lay_out(&app, &programs, &identity, &version()?, feed.as_ref())?;
    let signer = local_identity();
    for service in SERVICES {
        run(Command::new("codesign")
            .args(["--force", "--sign", &signer, "--timestamp=none", "--entitlements"])
            .arg(service.write_entitlements()?)
            .arg(service.bundle(&app)))?;
    }
    for part in updates::nested_code(&app).into_iter().chain([app.clone()]) {
        run(Command::new("codesign")
            .args(["--force", "--sign", &signer, "--timestamp=none"])
            .arg(&part))?;
    }
    Ok(app)
}

/// The distribution in `target/distribution/`: the universal programs with dictation, Sparkle reading `feed`,
/// unsigned.
pub(crate) fn distribution(version: &str, feed: &updates::Feed) -> Result<PathBuf, String> {
    let programs = universal_programs(&mut System, &program_names(), &["dictation"])?;
    let identity = Identity::of(true);
    let app = target_dir().join("distribution").join(format!("{}.app", identity.name));
    lay_out(&app, &programs, &identity, version, Some(feed))?;
    Ok(app)
}

/// Lays the bundle out at `app` around `programs`, the app's first, then its services' as `SERVICES` lists them;
/// with a feed, Sparkle comes along.
fn lay_out(
    app: &Path,
    programs: &[PathBuf],
    identity: &Identity,
    version: &str,
    feed: Option<&updates::Feed>,
) -> Result<(), String> {
    let contents = app.join("Contents");
    if app.exists() {
        files::remove_dir_all(app)?;
    }
    for dir in ["MacOS", "Resources"] {
        files::create_dir_all(contents.join(dir))?;
    }
    let plist = info_plist(&identity.bundle_id, &identity.name, version, &identity.scheme, feed);
    files::write(contents.join("Info.plist"), plist)?;
    files::copy(&programs[0], executable(app))?;
    for (service, program) in SERVICES.iter().zip(&programs[1..]) {
        service.assemble(app, program, &identity.bundle_id, version, MINIMUM_SYSTEM)?;
    }
    copy_tree(&root().join("resources"), &contents.join("Resources"))?;
    crate::render_page::copy(&root().join("../splat/resources"), &contents.join("Resources"))?;
    if feed.is_some() {
        let sparkle = updates::sparkle(&mut System)?;
        updates::embed_sparkle(&mut System, app, &sparkle)?;
    }
    Ok(())
}

/// The hardened runtime withholds these services from the app and every program it starts unless the signature
/// claims them; each matches a usage text of the property list (F217).
const RUNTIME_EXCEPTIONS: [(&str, &str); 7] = [
    (
        "NSAppleEventsUsageDescription",
        "com.apple.security.automation.apple-events",
    ),
    ("NSCameraUsageDescription", "com.apple.security.device.camera"),
    ("NSMicrophoneUsageDescription", "com.apple.security.device.audio-input"),
    (
        "NSContactsUsageDescription",
        "com.apple.security.personal-information.addressbook",
    ),
    (
        "NSCalendarsUsageDescription",
        "com.apple.security.personal-information.calendars",
    ),
    (
        "NSLocationUsageDescription",
        "com.apple.security.personal-information.location",
    ),
    (
        "NSPhotoLibraryUsageDescription",
        "com.apple.security.personal-information.photos-library",
    ),
];

/// The entitlements of the distribution's app.
pub(crate) fn entitlements() -> String {
    let keys: String = RUNTIME_EXCEPTIONS
        .iter()
        .map(|(_, key)| format!("\t<key>{key}</key>\n\t<true/>\n"))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{keys}</dict>\n</plist>\n"
    )
}

/// Signs the distribution with the hardened runtime, inside out as Sparkle's documentation prescribes instead of
/// `--deep`: the services with their own entitlements, Sparkle's `Autoupdate`, `Updater.app` and framework, then the
/// app with `entitlements`; then has codesign check the result. Notarization needs the secure timestamp, a request to
/// Apple that a dry run skips.
pub(crate) fn sign(sh: &mut dyn Shell, app: &Path, identity: &str, timestamp: bool) -> Result<(), String> {
    let claims = target_dir().join(format!("{BUNDLE_ID}.entitlements"));
    files::write(&claims, entitlements())?;
    let mut parts: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    for service in SERVICES {
        parts.push((service.bundle(app), Some(service.write_entitlements()?)));
    }
    parts.extend(updates::nested_code(app).into_iter().map(|part| (part, None)));
    parts.push((app.to_path_buf(), Some(claims)));
    for (part, claims) in parts {
        let mut codesign = Command::new("codesign");
        codesign
            .args(["--force", "--sign", identity, "--options", "runtime"])
            .arg(if timestamp { "--timestamp" } else { "--timestamp=none" });
        if let Some(claims) = claims {
            codesign.arg("--entitlements").arg(claims);
        }
        sh.run(codesign.arg(part))?;
    }
    sh.run(
        Command::new("codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(app),
    )
}

/// Hash and name of the first valid Developer ID Application certificate, which notarization requires.
pub(crate) fn developer_id(sh: &mut dyn Shell) -> Result<(String, String), String> {
    let listed = sh.stdout(Command::new("security").args(["find-identity", "-v", "-p", "codesigning"]))?;
    first_developer_id(&listed).ok_or_else(|| "no Developer ID Application certificate in the keychain".into())
}

/// The first Developer ID Application identity in the output of `security find-identity -v`.
pub(crate) fn first_developer_id(listed: &str) -> Option<(String, String)> {
    listed.lines().find_map(|line| {
        let name = line.split('"').nth(1)?;
        let hash = line.split_whitespace().nth(1)?;
        name.starts_with("Developer ID Application:")
            .then(|| (hash.to_string(), name.to_string()))
    })
}

/// The identity local builds and dry runs are signed with, and its name: the Developer ID when the keychain holds
/// one, else none (`-`).
///
/// An ad-hoc signature changes with every build, so the keychain takes each build for another program and asks again
/// for the key of the restored state; a certificate's signature stays the same across builds.
pub(crate) fn local_signer(sh: &mut dyn Shell) -> (String, String) {
    developer_id(sh).unwrap_or_else(|_| ("-".to_string(), "an ad hoc signature".to_string()))
}

fn local_identity() -> String {
    local_signer(&mut System).0
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let Ok(entries) = std::fs::read_dir(from) else {
        return Ok(());
    };
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in entries.flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            std::fs::copy(&path, &target).map_err(|e| format!("{}: {e}", path.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_property_list_claims_ssh_scripts_and_folders_and_explains_every_service() {
        let plist = info_plist("local.x", "Tapa", "1.0", "tapa", None);
        assert!(plist.contains("<string>ssh</string>"));
        assert!(plist.contains("com.apple.terminal.shell-script"));
        assert!(plist.contains("public.folder"));
        for (key, _) in USAGE {
            assert!(plist.contains(key));
        }
    }

    #[test]
    fn each_runtime_exception_belongs_to_a_usage_text_of_the_property_list() {
        let plist = info_plist("local.x", "T", "1.0", "t", None);
        let text = entitlements();
        for (usage, key) in RUNTIME_EXCEPTIONS {
            assert!(plist.contains(&format!("<key>{usage}</key>")), "{usage}");
            assert!(text.contains(&format!("\t<key>{key}</key>\n\t<true/>\n")), "{key}");
        }
        assert!(!text.contains("disable-library-validation") && !text.contains("app-sandbox"));
    }

    #[test]
    fn the_developer_id_is_found_among_the_identities() {
        let listed = "  1) 1111111111111111111111111111111111111111 \"Apple Development: Someone (TEAM)\"\n  \
                      2) 2222222222222222222222222222222222222222 \"Developer ID Application: Someone (TEAM)\"\n     \
                      2 valid identities found\n";
        assert_eq!(
            first_developer_id(listed),
            Some((
                "2222222222222222222222222222222222222222".to_string(),
                "Developer ID Application: Someone (TEAM)".to_string()
            ))
        );
        assert_eq!(first_developer_id("     0 valid identities found\n"), None);
        let mut bare =
            crate::shell::fake::Fake::new(|_: &Command, _: &str| (0, "     0 valid identities found\n".into()));
        assert_eq!(local_signer(&mut bare).0, "-");
    }

    #[test]
    fn the_distribution_is_signed_inside_out_with_the_hardened_runtime() {
        let app = Path::new("/d/T.app");
        let mut sh = crate::shell::fake::Fake::new(|_: &Command, _: &str| (0, String::new()));
        sign(&mut sh, app, "HASH", true).unwrap();
        let signed = sh.of("codesign");
        assert_eq!(signed.len(), SERVICES.len() + 2);
        assert!(signed[0].ends_with(&SERVICES[0].bundle(app).display().to_string()));
        assert!(signed[0].contains("--sign HASH --options runtime --timestamp --entitlements"));
        let last_signed = signed[signed.len() - 2];
        assert!(last_signed.ends_with(" /d/T.app") && last_signed.contains(&format!("{BUNDLE_ID}.entitlements")));
        assert_eq!(signed[signed.len() - 1], "codesign --verify --deep --strict /d/T.app");
        let mut sh = crate::shell::fake::Fake::new(|_: &Command, _: &str| (0, String::new()));
        sign(&mut sh, app, "HASH", false).unwrap();
        assert!(sh.of("codesign")[0].contains("--timestamp=none"));
    }
}
