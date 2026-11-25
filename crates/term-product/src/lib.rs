//! The product's name and the identifiers its stored data hangs on, in one place for the app and the build tool
//! (E19). Only the app depends on this crate; the other crates get the identity handed over at start (E3).
//!
//! The display name may change at any time. The identifiers below it name settings, keychain items and folders on
//! users' Macs; changing one of them after a release strands that data.

#![forbid(unsafe_code)]

/// What users read: menu titles, the About panel, the app bundle, the disk image and `TERM_PROGRAM`.
pub const NAME: &str = "Tapa";

/// The bundle identifier; also the defaults domain, the keychain service and the name of the data folders.
///
/// ```
/// use term_product::{BUNDLE_ID, NAME};
///
/// assert!(!BUNDLE_ID.to_lowercase().contains(&NAME.to_lowercase()));
/// ```
pub const BUNDLE_ID: &str = "com.muellermails.terminal";

/// The bundle identifier builds carried before `BUNDLE_ID`, whose data and keychain items the app takes over at its first start.
pub const FORMER_BUNDLE_ID: &str = "local.vt-terminal";

/// The token of the Homebrew Cask, which users install with.
pub const CASK: &str = "tapa";

/// How the cask and the website describe the app, always with "terminal" for the name alone is hard to find (R20).
pub const TAGLINE: &str = "Tapa terminal: fast, native, comfortable";

/// The command line tool the app installs (F172).
pub const COMMAND: &str = "tapa";

/// The URL scheme the app answers (F177).
pub const URL_SCHEME: &str = "tapa";

/// The product's web page, which the cask names.
pub const HOMEPAGE: &str = "https://github.com/sventmueller/tapa";

/// The project on GitHub, where the Help menu leads: its problem form, its ideas and the repository itself.
pub const REPOSITORY: &str = "https://github.com/sventmueller/tapa";

/// The licenses the code is under, each with its file in the repository, as the About panel links them.
pub const LICENSES: [(&str, &str); 2] = [("MIT", "LICENSE-MIT"), ("Apache 2.0", "LICENSE-APACHE")];

/// The releases page in GitHub's layout: each release's files under `/download/v<version>/`, the newest one's under
/// `/latest/download/`; while empty, builds carry no updater.
pub const RELEASES: &str = "https://github.com/sventmueller/tapa/releases";

/// The public half of the EdDSA key every update must be signed with, as Sparkle's `generate_keys -p` prints it for
/// its default account, shared with Splat and Lowspeak; installed copies accept nothing else, so losing the private
/// half ends their updates.
pub const UPDATE_PUBLIC_KEY: &str = "21sUy80rKTKlgQLPk+T7UrAkoqVQ+VV+6rfHKVAL4EU=";

/// Where installed copies look for updates: the feed is a file of the newest release, so publishing a release
/// publishes the update. None while `RELEASES` or `UPDATE_PUBLIC_KEY` is empty.
pub fn feed_url() -> Option<String> {
    (!RELEASES.is_empty() && !UPDATE_PUBLIC_KEY.is_empty()).then(|| format!("{RELEASES}/latest/download/appcast.xml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_do_not_carry_the_display_name() {
        assert!(!BUNDLE_ID.to_lowercase().contains(&NAME.to_lowercase()));
    }

    #[test]
    fn releases_are_found_only_once_their_address_and_key_are_known() {
        assert_eq!(
            feed_url().is_some(),
            !RELEASES.is_empty() && !UPDATE_PUBLIC_KEY.is_empty()
        );
    }

    #[test]
    fn the_cask_token_is_what_homebrew_accepts() {
        assert!(!CASK.is_empty());
        assert!(
            CASK.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        );
    }
}
