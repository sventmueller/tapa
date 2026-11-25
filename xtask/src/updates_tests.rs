//! Sparkle's download, framework and keys and the update feed, with a shell that stands in for curl, tar and
//! Sparkle's tools.

use super::*;
use crate::shell::fake::{Fake, arg, scratch};

fn notes() -> Vec<(String, String)> {
    vec![("en".into(), "- New.\n".into()), ("de".into(), "- Neu.\n".into())]
}

#[test]
fn the_feed_offers_the_signed_image_with_its_notes_in_each_language() {
    let signature = "sparkle:edSignature=\"c2ln\" length=\"42\"";
    let url = image_url("https://h.example/releases", "1.0.4");
    let notes = notes();
    let entry = Entry {
        version: "1.0.4",
        url: &url,
        signature,
        date: "Sat, 07 Nov 2026 12:00:00 +0000",
        notes: &notes,
    };
    let feed = appcast(&entry).unwrap();
    assert!(feed.contains(&format!(
        "<enclosure url=\"https://h.example/releases/download/v1.0.4/{}\" \
         sparkle:edSignature=\"c2ln\" length=\"42\" type=\"application/octet-stream\"/>",
        image_name("1.0.4")
    )));
    assert!(feed.contains("<sparkle:version>1.0.4</sparkle:version>"));
    assert!(feed.contains("<sparkle:minimumSystemVersion>14.0</sparkle:minimumSystemVersion>"));
    assert!(feed.contains(
        "      <description sparkle:format=\"markdown\" xml:lang=\"en\"><![CDATA[- New.]]></description>\n\
         \x20     <description sparkle:format=\"markdown\" xml:lang=\"de\"><![CDATA[- Neu.]]></description>\n"
    ));
    assert!(
        appcast(&Entry {
            signature: "ERROR: no key",
            ..entry
        })
        .is_err()
    );
    let broken = vec![("en".to_string(), "a ]]> b".to_string())];
    assert!(
        appcast(&Entry {
            notes: &broken,
            ..entry
        })
        .is_err()
    );
}

#[test]
fn the_app_signs_its_updates_with_the_key_splat_and_lowspeak_use() {
    assert_eq!(
        UPDATE_KEY_ACCOUNT, "ed25519",
        "Sparkle's default account, shared with Splat and Lowspeak"
    );
    assert_eq!(
        UPDATE_PUBLIC_KEY, "21sUy80rKTKlgQLPk+T7UrAkoqVQ+VV+6rfHKVAL4EU=",
        "Splat's and Lowspeak's key"
    );
    assert_eq!(update_key(&mut System, true).unwrap(), UPDATE_PUBLIC_KEY);
    let import = missing_key(false);
    assert!(
        import.contains("generate_keys --account ed25519 -f <file>`"),
        "{import}"
    );
    assert!(import.contains("UPDATE_PUBLIC_KEY in crates/term-product/src/lib.rs"));
    assert!(!missing_key(true).contains("import"));
}

#[test]
fn the_updater_is_on_only_with_a_feed() {
    assert!(info_plist_keys(None).is_empty());
    let keys = info_plist_keys(Some(&Feed {
        url: "https://h.example/appcast.xml",
        key: "KEY",
    }));
    assert_eq!(
        keys,
        "\t<key>SUFeedURL</key>\n\t<string>https://h.example/appcast.xml</string>\n\t<key>SUPublicEDKey</key>\n\t\
         <string>KEY</string>\n"
    );
}

/// curl, shasum and tar as they behave for Sparkle's archive whose checksum is `checksum`.
fn download(checksum: &'static str) -> impl FnMut(&Command, &str) -> (i32, String) {
    move |cmd: &Command, line: &str| match line.split(' ').next() {
        Some("curl") => {
            std::fs::write(arg(cmd, Some("-o")), "archive").unwrap();
            (0, String::new())
        }
        Some("shasum") => (0, format!("{checksum}  archive\n")),
        Some("tar") => {
            std::fs::create_dir_all(arg(cmd, Some("-C")).join("Sparkle.framework")).unwrap();
            (0, String::new())
        }
        _ => (0, String::new()),
    }
}

#[test]
fn sparkle_is_downloaded_once_checked_against_its_checksum_and_unpacked() {
    let target = scratch("sparkle-download");
    let mut sh = Fake::new(download(SPARKLE_SHA256));
    let dir = sparkle_in(&mut sh, &target).unwrap();
    assert_eq!(dir, target.join(format!("sparkle-{SPARKLE_VERSION}")));
    assert!(dir.join("Sparkle.framework").is_dir());
    let archive = target.join(format!("Sparkle-{SPARKLE_VERSION}.tar.xz"));
    assert!(!archive.exists() && !target.join(format!("sparkle-{SPARKLE_VERSION}.partial")).exists());
    assert!(sh.commands[0].starts_with("curl -fsSL --proto =https --proto-redir =https -o"));
    assert!(sh.commands[0].ends_with(&format!("{SPARKLE_VERSION}/Sparkle-{SPARKLE_VERSION}.tar.xz")));
    assert_eq!(sh.commands.len(), 3);
    assert_eq!(sparkle_in(&mut sh, &target).unwrap(), dir);
    assert_eq!(sh.commands.len(), 3, "an unpacked release is not downloaded again");
    std::fs::remove_dir_all(target).unwrap();

    let target = scratch("sparkle-tampered");
    let mut sh = Fake::new(download("0bad"));
    let failed = sparkle_in(&mut sh, &target).unwrap_err();
    assert!(
        failed.ends_with(&format!("has checksum 0bad, expected {SPARKLE_SHA256}")),
        "{failed}"
    );
    assert!(!target.join(format!("Sparkle-{SPARKLE_VERSION}.tar.xz")).exists());
    assert!(sh.of("tar").is_empty());
    std::fs::remove_dir_all(target).unwrap();
}

#[test]
fn the_embedded_framework_leaves_out_the_xpc_services_of_sandboxed_apps() {
    let dir = scratch("sparkle-embed");
    let app = dir.join("A.app");
    std::fs::create_dir_all(app.join("Contents/Resources")).unwrap();
    std::fs::create_dir_all(dir.join("sparkle")).unwrap();
    std::fs::write(dir.join("sparkle/LICENSE"), "MIT").unwrap();
    let mut sh = Fake::new(|cmd: &Command, _: &str| {
        let framework = arg(cmd, None);
        std::fs::create_dir_all(framework.join("Versions/B/XPCServices/Installer.xpc")).unwrap();
        std::fs::create_dir_all(framework.join("Versions/B/Autoupdate")).unwrap();
        std::fs::write(framework.join("XPCServices"), "link").unwrap();
        (0, String::new())
    });
    embed_sparkle(&mut sh, &app, &dir.join("sparkle")).unwrap();
    let framework = framework(&app);
    assert_eq!(
        sh.commands,
        [format!(
            "ditto {} {}",
            dir.join("sparkle/Sparkle.framework").display(),
            framework.display()
        )]
    );
    assert!(framework.join("Versions/B/Autoupdate").exists());
    assert!(!framework.join("Versions/B/XPCServices").exists() && !framework.join("XPCServices").exists());
    assert!(app.join("Contents/Resources/LICENSE-sparkle").is_file());
    assert_eq!(sparkle_programs(&app)[0], framework.join("Versions/B/Sparkle"));
    assert_eq!(nested_code(&app).last(), Some(&framework));
    assert!(nested_code(&dir.join("B.app")).is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_notarized_release_needs_the_keychains_key_to_be_the_published_one() {
    let target = scratch("sparkle-key");
    std::fs::create_dir_all(target.join(format!("sparkle-{SPARKLE_VERSION}/Sparkle.framework"))).unwrap();
    let answering = |present: i32, key: &'static str| {
        move |_: &Command, line: &str| {
            if line.starts_with("security find-generic-password") {
                (present, String::new())
            } else {
                (0, format!("{key}\n"))
            }
        }
    };
    let mut sh = Fake::new(answering(0, "PUBLIC"));
    assert_eq!(update_key_of(&mut sh, false, "PUBLIC", &target).unwrap(), "PUBLIC");
    assert_eq!(
        sh.commands[0],
        format!("security find-generic-password -s {UPDATE_KEY_SERVICE} -a {UPDATE_KEY_ACCOUNT}")
    );
    assert_eq!(
        sh.commands[1],
        format!("{} -p --account {UPDATE_KEY_ACCOUNT}", generate_keys(&target).display())
    );
    let mut other = Fake::new(answering(0, "OTHER"));
    let wrong = update_key_of(&mut other, false, "PUBLIC", &target).unwrap_err();
    assert!(
        wrong.starts_with("the keychain's update signing key OTHER under"),
        "{wrong}"
    );
    let mut absent = Fake::new(answering(44, ""));
    assert_eq!(
        update_key_of(&mut absent, false, "PUBLIC", &target).unwrap_err(),
        missing_key(false)
    );
    let mut unpublished = Fake::new(answering(0, ""));
    assert_eq!(
        update_key_of(&mut unpublished, false, "", &target).unwrap_err(),
        missing_key(true)
    );
    assert_eq!(
        update_key_of(&mut unpublished, true, "", &target).unwrap(),
        DRY_RUN_UPDATE_KEY
    );
    let mut asked = Fake::new(answering(0, ""));
    assert_eq!(update_key_of(&mut asked, true, "PUBLIC", &target).unwrap(), "PUBLIC");
    assert!(asked.commands.is_empty(), "a dry run asks the keychain nothing");

    let mut signer = Fake::new(|_: &Command, _: &str| (0, "sparkle:edSignature=\"x\" length=\"1\"\n".into()));
    let signed = sign_update(&mut signer, &target, Path::new("/i.dmg")).unwrap();
    assert_eq!(signed, "sparkle:edSignature=\"x\" length=\"1\"\n");
    assert_eq!(
        signer.commands,
        [format!(
            "{} --account {UPDATE_KEY_ACCOUNT} /i.dmg",
            target.join("bin/sign_update").display()
        )]
    );
    let mut empty = Fake::new(|_: &Command, _: &str| (0, String::new()));
    assert_eq!(
        sha256(&mut empty, Path::new("/i.dmg")).unwrap_err(),
        "no checksum for /i.dmg"
    );
    std::fs::remove_dir_all(target).unwrap();
}

#[test]
fn a_release_is_built_for_real_only_once_its_address_is_known() {
    assert_eq!(releases(true).unwrap(), term_product::RELEASES);
    assert_eq!(releases_of("", true).unwrap(), DRY_RUN_RELEASES);
    assert!(releases_of("", false).is_err());
    assert_eq!(
        feed_url("https://h.example/r"),
        "https://h.example/r/latest/download/appcast.xml"
    );
    assert_eq!(
        term_product::feed_url().as_deref(),
        Some(feed_url(term_product::RELEASES).as_str())
    );
    assert!(image_name("1.2.3").ends_with("-1.2.3.dmg") && !image_name("1.2.3").contains(' '));
    assert_eq!(latest_image_name(), format!("{NAME}.dmg"));
}
