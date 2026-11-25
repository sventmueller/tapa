//! The Homebrew Cask (F75), which the owner's tap serves; Homebrew checks its checksum against the disk image.

use term_product::{BUNDLE_ID, CASK, NAME};

use crate::services::SERVICES;

const DRY_RUN_HOMEPAGE: &str = "https://example.invalid/";

/// The cask's description, which Homebrew's style wants without the app's name and the platform.
const DESCRIPTION: &str = "Fast, native terminal with workspaces and agents";

/// The homepage a run writes into the Cask, `term_product::HOMEPAGE`.
pub(crate) fn homepage(dry_run: bool) -> Result<&'static str, String> {
    homepage_of(term_product::HOMEPAGE, dry_run)
}

fn homepage_of(homepage: &'static str, dry_run: bool) -> Result<&'static str, String> {
    match (homepage.is_empty(), dry_run) {
        (false, _) => Ok(homepage),
        (true, true) => Ok(DRY_RUN_HOMEPAGE),
        (true, false) => Err("HOMEPAGE in crates/term-product/src/lib.rs is empty: name the product's web page".into()),
    }
}

/// The Cask's file name in a tap's `Casks/` folder.
pub(crate) fn file_name() -> String {
    format!("{CASK}.rb")
}

/// What the Cask of a release says.
pub(crate) struct Release<'a> {
    pub(crate) version: &'a str,
    /// The disk image's SHA-256 checksum.
    pub(crate) sha256: &'a str,
    /// The releases page, see `term_product::RELEASES`.
    pub(crate) releases: &'a str,
    pub(crate) homepage: &'a str,
}

/// The Cask. Sparkle updates the app in place, so Homebrew leaves updating to it (`auto_updates`); `zap` names what
/// [`zap`] lists.
pub(crate) fn cask(release: &Release) -> String {
    let Release {
        version,
        sha256,
        releases,
        homepage,
    } = release;
    let image = crate::updates::image_name("#{version}");
    let feed = crate::updates::feed_url(releases);
    let zap: String = zap().iter().map(|path| format!("    \"{path}\",\n")).collect();
    format!(
        r#"cask "{CASK}" do
  version "{version}"
  sha256 "{sha256}"

  url "{releases}/download/v#{{version}}/{image}"
  name "{NAME}"
  desc "{DESCRIPTION}"
  homepage "{homepage}"

  livecheck do
    url "{feed}"
    strategy :sparkle
  end

  auto_updates true
  depends_on macos: :sonoma

  app "{NAME}.app"

  zap trash: [
{zap}  ]
end
"#
    )
}

/// What the app and its XPC services leave below the home folder: the app's folders as `src/migrate.rs` lists them
/// with its defaults, and each sandboxed service's container, sorted as Homebrew's style wants.
fn zap() -> Vec<String> {
    let mut paths: Vec<String> = [
        "Application Support/{}",
        "Caches/{}",
        "HTTPStorages/{}",
        "HTTPStorages/{}.binarycookies",
        "Logs/{}",
        "Preferences/{}.plist",
        "Saved Application State/{}.savedState",
        "WebKit/{}",
    ]
    .iter()
    .map(|path| format!("~/Library/{}", path.replace("{}", BUNDLE_ID)))
    .collect();
    for service in SERVICES {
        let id = service.bundle_id(BUNDLE_ID);
        paths.push(format!("~/Library/Application Scripts/{id}"));
        paths.push(format!("~/Library/Containers/{id}"));
    }
    paths.sort();
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cask_downloads_the_image_of_its_version_and_lets_sparkle_update() {
        let cask = cask(&Release {
            version: "1.2.3",
            sha256: "ab12",
            releases: "https://h.example/releases",
            homepage: "https://h.example/",
        });
        assert!(cask.starts_with(&format!("cask \"{CASK}\" do\n  version \"1.2.3\"\n  sha256 \"ab12\"\n")));
        assert!(cask.contains(&format!(
            "  url \"https://h.example/releases/download/v#{{version}}/{}\"\n",
            crate::updates::image_name("#{version}")
        )));
        assert!(
            cask.replace("#{version}", "1.2.3")
                .contains(&crate::updates::image_url("https://h.example/releases", "1.2.3"))
        );
        assert!(
            cask.contains(
                "    url \"https://h.example/releases/latest/download/appcast.xml\"\n    strategy :sparkle\n"
            )
        );
        assert!(cask.contains("  auto_updates true\n  depends_on macos: :sonoma\n"));
        assert!(cask.contains(&format!("  app \"{NAME}.app\"\n")));
        for path in [
            format!("Application Support/{BUNDLE_ID}"),
            format!("HTTPStorages/{BUNDLE_ID}.binarycookies"),
            format!("WebKit/{BUNDLE_ID}"),
            format!("Containers/{BUNDLE_ID}.decoder"),
        ] {
            assert!(
                cask.contains(&format!("    \"~/Library/{path}\",\n")),
                "{path} missing in {cask}"
            );
        }
        assert!(
            !cask.contains("macOS") && !DESCRIPTION.contains(NAME),
            "Homebrew's audit refuses the platform and the name in the description"
        );
        assert_eq!(file_name(), format!("{CASK}.rb"));
    }

    #[test]
    fn a_release_is_built_for_real_only_once_the_homepage_is_known() {
        assert_eq!(homepage(false).unwrap(), term_product::HOMEPAGE);
        assert!(homepage_of("", false).is_err());
        assert_eq!(homepage_of("", true).unwrap(), DRY_RUN_HOMEPAGE);
    }
}
