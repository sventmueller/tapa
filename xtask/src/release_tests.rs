//! The release's steps with a shell that stands in for codesign, hdiutil, ditto, lipo, Apple's notary service and
//! Sparkle's tools, leaving the files behind the real ones would.

use std::path::{Path, PathBuf};
use std::process::Command;

use term_product::NAME;

use super::*;
use crate::shell::fake::{Answer, Fake, arg, scratch};

const SIGNATURE: &str = "sparkle:edSignature=\"c2ln\" length=\"11\"\n";

fn write(path: &Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

/// What the programs a release runs do, as far as the next step can tell.
fn stand_in(cmd: &Command, line: &str) -> Answer {
    let words: Vec<&str> = line.split(' ').collect();
    let program = Path::new(words[0])
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    match (program, words.get(1).copied().unwrap_or_default()) {
        ("lipo", "-archs") => (0, "x86_64 arm64\n".into()),
        ("hdiutil", "create") => {
            let staging = arg(cmd, Some("-srcfolder"));
            let link = std::fs::read_link(staging.join("Applications"));
            if link.ok().as_deref() != Some(Path::new("/Applications")) {
                return (1, String::new());
            }
            write(&arg(cmd, None), "writable");
            (0, String::new())
        }
        ("hdiutil", "convert") => {
            write(&arg(cmd, Some("-o")), "image bytes");
            (0, String::new())
        }
        ("hdiutil", "attach") if line.contains(" -mountpoint ") => {
            let mount = arg(cmd, Some("-mountpoint"));
            let _ = std::os::unix::fs::symlink("/Applications", mount.join("Applications"));
            (0, String::new())
        }
        ("hdiutil", "detach") => {
            let _ = std::fs::remove_file(arg(cmd, None).join("Applications"));
            (0, String::new())
        }
        ("ditto", "-c") => {
            write(&arg(cmd, None), "zip");
            (0, String::new())
        }
        ("xcrun", "notarytool") => (0, r#"{"id":"5e1f-07","status":"Accepted"}"#.into()),
        ("sign_update", _) => (0, SIGNATURE.into()),
        _ => (0, String::new()),
    }
}

/// An app as `bundle::distribution` leaves it, in a folder of its own.
fn assembled(test: &str) -> (PathBuf, PathBuf) {
    let dir = scratch(test);
    let app = dir.join(format!("{NAME}.app"));
    std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
    (dir, app)
}

fn signing<'a>(dir: &'a Path, notarize: bool, mount: &'a Path) -> Signing<'a> {
    Signing {
        identity: "HASH",
        notarize,
        layout: false,
        sparkle: dir,
        mount,
    }
}

/// The second word of each command of `program`, which says what hdiutil or xcrun was asked to do.
fn verbs<'a>(sh: &'a Fake<impl FnMut(&Command, &str) -> Answer>, program: &str) -> Vec<&'a str> {
    sh.of(program)
        .iter()
        .filter_map(|line| line.split(' ').nth(1))
        .collect()
}

#[test]
fn a_dry_run_packs_the_signed_app_into_a_checked_image_without_asking_apple() {
    let (dir, app) = assembled("release-dry-run");
    let mount = dir.join("mount");
    let mut sh = Fake::new(stand_in);
    let (image, signature) = package(&mut sh, &app, &signing(&dir, false, &mount)).unwrap();

    assert_eq!(image, dir.join(format!("{NAME}.dmg")));
    assert_eq!(signature, "sparkle:edSignature=\"DRY-RUN-NOT-SIGNED\" length=\"11\"");
    assert!(sh.of("xcrun").is_empty() && sh.of("spctl").is_empty());
    assert_eq!(
        verbs(&sh, "hdiutil"),
        ["create", "convert", "verify", "attach", "detach"]
    );
    let signed: Vec<&str> = sh
        .of("codesign")
        .into_iter()
        .filter(|l| l.contains("--sign HASH"))
        .collect();
    assert!(signed.iter().all(|l| l.contains("--timestamp=none")), "{signed:?}");
    assert_eq!(
        signed.last().copied(),
        Some(format!("codesign --force --sign HASH --timestamp=none {}", image.display()).as_str())
    );
    let verified = format!(
        "codesign --verify --deep --strict {}",
        mount.join(format!("{NAME}.app")).display()
    );
    assert!(sh.commands.contains(&verified), "{:?}", sh.commands);
    let checked_twice = 2 * (1 + SERVICES.len());
    assert_eq!(sh.of("lipo").len(), checked_twice);
    assert!(!dir.join("dmg-staging").exists() && !dir.join(format!("{NAME}-writable.dmg")).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_notarized_release_staples_the_app_and_the_image_and_signs_the_image_for_sparkle() {
    let (dir, app) = assembled("release-notarized");
    let mount = dir.join("mount");
    let mut sh = Fake::new(stand_in);
    let (image, signature) = package(&mut sh, &app, &signing(&dir, true, &mount)).unwrap();

    assert_eq!(signature, SIGNATURE);
    let archive = app.with_extension("zip");
    let xcrun = sh.of("xcrun");
    assert_eq!(xcrun.len(), 4, "{xcrun:?}");
    assert!(xcrun[0].starts_with("xcrun notarytool submit --keychain-profile notary --wait --output-format json"));
    assert!(xcrun[0].ends_with(&archive.display().to_string()));
    assert_eq!(xcrun[1], format!("xcrun stapler staple {}", app.display()));
    assert!(xcrun[2].ends_with(&image.display().to_string()) && xcrun[2].contains("submit"));
    assert_eq!(xcrun[3], format!("xcrun stapler staple {}", image.display()));
    assert!(!archive.exists(), "the archive only carries the app to Apple");
    let signed: Vec<&str> = sh
        .of("codesign")
        .into_iter()
        .filter(|l| l.contains("--sign HASH"))
        .collect();
    assert!(signed.iter().all(|l| l.contains("--timestamp ")), "{signed:?}");
    let assessed = sh.of("spctl");
    assert_eq!(assessed.len(), 1);
    assert!(assessed[0].contains("--type open") && assessed[0].ends_with(&image.display().to_string()));
    let last = sh.commands.last().unwrap();
    assert_eq!(
        last,
        &format!(
            "{} --account {} {}",
            dir.join("bin/sign_update").display(),
            updates::UPDATE_KEY_ACCOUNT,
            image.display()
        )
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_rejected_notarization_fetches_the_log_and_fails() {
    let mut sh = Fake::new(|_: &Command, line: &str| {
        if line.contains(" submit ") {
            (
                0,
                r#"{"id": "77-ab", "message": "Processing complete", "status": "Invalid"}"#.into(),
            )
        } else {
            (0, String::new())
        }
    });
    let failed = notarize(&mut sh, Path::new("/x/T.dmg")).unwrap_err();
    assert!(failed.contains("status Invalid"), "{failed}");
    assert_eq!(sh.commands[1], "xcrun notarytool log 77-ab --keychain-profile notary");

    let mut refused = Fake::new(|_: &Command, _: &str| (1, r#"{"status":"Accepted"}"#.into()));
    let failed = notarize(&mut refused, Path::new("/x/T.dmg")).unwrap_err();
    assert!(
        failed.contains("status Accepted, notarytool exit status: 1"),
        "{failed}"
    );
    assert_eq!(refused.commands.len(), 1, "no id, no log to fetch");
}

#[test]
fn an_image_whose_applications_link_leads_elsewhere_fails_and_is_still_unmounted() {
    let dir = scratch("release-check-image");
    let mount = dir.join("mount");
    let mut sh = Fake::new(|cmd: &Command, line: &str| {
        if line.starts_with("hdiutil attach") {
            let _ = std::os::unix::fs::symlink("/Users", arg(cmd, Some("-mountpoint")).join("Applications"));
        }
        stand_in(cmd, line)
    });
    let failed = check_image(&mut sh, &dir.join("T.dmg"), &mount).unwrap_err();
    assert_eq!(failed, "the Applications link leads to /Users");
    assert_eq!(verbs(&sh, "hdiutil").last(), Some(&"detach"));

    let mut sh = Fake::new(stand_in);
    std::fs::create_dir_all(&mount).unwrap();
    assert!(check_image(&mut sh, &dir.join("T.dmg"), &mount).is_ok());
    assert_eq!(
        verbs(&sh, "hdiutil"),
        ["verify", "detach", "attach", "detach"],
        "a mount left from before is detached first"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn publishing_writes_the_image_its_notes_the_feed_and_the_cask() {
    let dir = scratch("release-publish");
    let out = dir.join("out");
    write(&out.join("stale.txt"), "from the run before");
    let image = dir.join(format!("{NAME}.dmg"));
    write(&image, "image bytes");
    let notes = vec![
        ("en".to_string(), "- New.\n".to_string()),
        ("de".to_string(), "- Neu.\n".to_string()),
    ];
    let mut sh = Fake::new(|_: &Command, line: &str| match line.split(' ').next() {
        Some("date") => (0, "Fri, 13 Nov 2026 12:00:00 +0000\n".into()),
        Some("shasum") => (0, "abc123  some/path\n".into()),
        _ => (0, String::new()),
    });
    let release = Published {
        version: "1.2.3",
        image: &image,
        signature: SIGNATURE.trim(),
        releases: "https://h.example/r",
        homepage: "https://h.example/",
        notes: &notes,
    };
    publish(&mut sh, &out, true, &release).unwrap();

    assert!(!out.join("stale.txt").exists() && !image.exists());
    let published = out.join(updates::image_name("1.2.3"));
    assert_eq!(std::fs::read_to_string(&published).unwrap(), "image bytes");
    assert_eq!(
        std::fs::read_to_string(out.join(format!("{NAME}-1.2.3.md"))).unwrap(),
        "- New.\n"
    );
    let feed = std::fs::read_to_string(out.join(updates::FEED)).unwrap();
    assert!(
        feed.contains("<pubDate>Fri, 13 Nov 2026 12:00:00 +0000</pubDate>"),
        "{feed}"
    );
    assert!(feed.contains(&updates::image_url("https://h.example/r", "1.2.3")));
    assert!(feed.contains("xml:lang=\"de\"><![CDATA[- Neu.]]>"));
    let cask = std::fs::read_to_string(out.join(cask::file_name())).unwrap();
    assert!(
        cask.contains("sha256 \"abc123\"") && cask.contains("version \"1.2.3\""),
        "{cask}"
    );
    assert_eq!(
        sh.of("xmllint"),
        [format!("xmllint --noout {}", out.join(updates::FEED).display())]
    );
    assert_eq!(
        sh.of("ruby"),
        [format!("ruby -c {}", out.join(cask::file_name()).display())]
    );
    assert_eq!(sh.of("shasum"), [format!("shasum -a 256 {}", published.display())]);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A workspace with three interface languages, notes for English and a draft for German.
fn workspace(test: &str) -> PathBuf {
    let root = scratch(test);
    for language in ["de", "en", "fr"] {
        std::fs::create_dir_all(root.join(format!("resources/{language}.lproj"))).unwrap();
    }
    write(&root.join(NOTES), "- New.\n");
    write(&root.join(translated_notes("de")), "<!-- DRAFT -->\n- Neu.\n");
    root
}

#[test]
fn a_dry_run_takes_the_notes_there_are_and_a_release_wants_all_of_them_reviewed() {
    let root = workspace("release-notes");
    assert_eq!(
        notes(&root, true, None).unwrap(),
        [
            ("en".to_string(), "- New.\n".to_string()),
            ("de".to_string(), "- Neu.\n".to_string())
        ]
    );
    let draft = notes(&root, false, None).unwrap_err();
    assert!(
        draft.starts_with("assets/readme/de/RELEASE_NOTES.md is still a draft"),
        "{draft}"
    );

    write(&root.join(translated_notes("de")), "- Neu.\n");
    let missing = notes(&root, false, None).unwrap_err();
    assert!(missing.starts_with("release notes missing for fr;"), "{missing}");
    write(&root.join(translated_notes("fr")), "- Nouveau.\n");
    assert_eq!(notes(&root, false, None).unwrap().len(), 3);
    let stale = notes(&root, false, Some("<![CDATA[- Nouveau.]]>")).unwrap_err();
    assert!(
        stale.contains("assets/readme/fr/RELEASE_NOTES.md still holds"),
        "{stale}"
    );

    std::fs::remove_file(root.join(NOTES)).unwrap();
    let without_english = notes(&root, true, None).unwrap();
    assert_eq!(without_english[0], ("en".to_string(), "Dry run.\n".to_string()));
    assert_eq!(without_english.len(), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_dry_run_lists_what_a_notarized_release_still_lacks() {
    let mut sh = Fake::new(|_: &Command, line: &str| {
        if line.starts_with("security find-identity") {
            (0, "  1) 2222 \"Developer ID Application: Someone (TEAM)\"\n".into())
        } else {
            (44, String::new())
        }
    });
    let listed = preflight(&mut sh);
    assert!(listed.starts_with("dry run; a release with --notarize needs:\n"));
    assert!(
        listed.contains("  ok      Developer ID Application certificate: Developer ID Application: Someone (TEAM)\n")
    );
    assert!(listed.contains(&format!(
        "  MISSING Sparkle update key of Splat and Lowspeak under the keychain account {}",
        updates::UPDATE_KEY_ACCOUNT
    )));
    assert_eq!(
        sh.commands[1],
        format!(
            "security find-generic-password -s https://sparkle-project.org -a {}",
            updates::UPDATE_KEY_ACCOUNT
        )
    );

    let mut bare = Fake::new(|_: &Command, _: &str| (0, "     0 valid identities found\n".into()));
    let listed = preflight(&mut bare);
    assert!(
        listed.contains("  MISSING Developer ID Application certificate\n"),
        "{listed}"
    );
    assert!(listed.contains("  ok      Sparkle update key"), "{listed}");
    assert_eq!(
        developer_id(&mut bare).unwrap_err(),
        "no Developer ID Application certificate in the keychain"
    );
    assert_eq!(local_signer(&mut bare).0, "-", "a dry run without one signs ad hoc");
}

#[test]
fn a_failed_check_before_the_build_says_how_to_put_it_right() {
    let mut failing = Fake::new(|_: &Command, _: &str| (1, String::new()));
    let untranslated = check_translations(&mut failing).unwrap_err();
    assert!(
        untranslated.contains("the interface is not translated completely"),
        "{untranslated}"
    );
    assert!(failing.commands[0].ends_with(" strings"), "{}", failing.commands[0]);
    let login = check_notary_profile(&mut failing).unwrap_err();
    assert!(
        login.contains("xcrun notarytool store-credentials notary --apple-id"),
        "{login}"
    );
    assert_eq!(
        failing.commands[1],
        "xcrun notarytool history --keychain-profile notary"
    );
    assert!(check_notary_profile(&mut Fake::new(stand_in)).is_ok());
}

#[test]
fn finder_lays_out_only_an_image_it_can_see_mounted() {
    let attached = "/dev/disk4\tGUID_partition_scheme\t\n/dev/disk4s1\tApple_HFS\t/Volumes/T 1\n";
    assert_eq!(mount_point(attached), Some(PathBuf::from("/Volumes/T 1")));
    assert_eq!(mount_point("/dev/disk4\tGUID_partition_scheme\t\n"), None);

    let (dir, app) = assembled("release-layout");
    let mut sh = Fake::new(stand_in);
    let failed = disk_image(&mut sh, &app, true).unwrap_err();
    assert_eq!(failed, "hdiutil attach reported no mount point");
    assert_eq!(verbs(&sh, "hdiutil"), ["create", "attach"]);
    assert!(sh.of("hdiutil")[1].starts_with("hdiutil attach -readwrite -noverify -noautoopen"));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_debug_symbols_of_every_program_stay_beside_the_release() {
    let mut sh = Fake::new(|_: &Command, _: &str| (0, String::new()));
    let kept = keep_dsyms(&mut sh, Path::new("/dist"), "1.2.3").unwrap();
    assert_eq!(kept, PathBuf::from(format!("/dist/{NAME}-1.2.3-dSYMs")));
    let names: Vec<&str> = std::iter::once(PROGRAM)
        .chain(SERVICES.iter().map(|service| service.program))
        .collect();
    assert_eq!(sh.commands.len(), names.len());
    for (line, name) in sh.commands.iter().zip(names) {
        let copied = format!(
            "ditto {} {}",
            universal_dsym(name).display(),
            kept.join(format!("{name}.dSYM")).display()
        );
        assert_eq!(*line, copied);
    }
}
