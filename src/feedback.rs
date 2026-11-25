//! The Help menu's way from the app to the project on GitHub, as in Dango and Splat: the problem form filled in with
//! what the app knows about itself, a new idea among the discussions, and the repository.

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSBundle, NSProcessInfo, NSString, NSURL};
use term_product::REPOSITORY;
use term_ui::kit::ns;

/// A page of the project on GitHub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Page {
    Problem,
    Idea,
    Repository,
}

#[cfg(feature = "selftest")]
thread_local! {
    /// The pages the self-test asked for, which it records instead of starting a browser.
    static OPENED: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Opens `page` in the user's browser, where they are signed in to GitHub.
pub(crate) fn open(page: Page) {
    let address = address(page);
    #[cfg(feature = "selftest")]
    if std::env::var_os("TAPA_SELFTEST").is_some() {
        return OPENED.with(|opened| opened.borrow_mut().push(address));
    }
    if let Some(url) = NSURL::URLWithString(&ns(&address)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// The pages the self-test asked for since the last call.
#[cfg(feature = "selftest")]
pub(crate) fn take_opened() -> Vec<String> {
    OPENED.with(std::cell::RefCell::take)
}

fn address(page: Page) -> String {
    match page {
        Page::Problem => problem_form(),
        Page::Idea => format!("{REPOSITORY}/discussions/new?category=ideas"),
        Page::Repository => REPOSITORY.to_string(),
    }
}

/// The problem form with the app's version, the version of macOS and the kind of Mac filled in.
fn problem_form() -> String {
    let version = NSBundle::mainBundle()
        .objectForInfoDictionaryKey(&ns("CFBundleShortVersionString"))
        .and_then(|version| version.downcast::<NSString>().ok())
        .map(|version| version.to_string())
        .unwrap_or_default();
    let system = NSProcessInfo::processInfo().operatingSystemVersion();
    let macos = format!(
        "{}.{}.{}",
        system.majorVersion, system.minorVersion, system.patchVersion
    );
    problem_address(&version, &macos, cfg!(target_arch = "aarch64"))
}

/// GitHub fills the fields of `.github/ISSUE_TEMPLATE/bug.yml` from the query by their ids.
fn problem_address(version: &str, macos: &str, apple_silicon: bool) -> String {
    let mac = if apple_silicon { "Apple%20Silicon" } else { "Intel" };
    format!("{REPOSITORY}/issues/new?template=bug.yml&version={version}&macos={macos}&mac={mac}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_problem_form_is_filled_in_by_the_ids_of_its_fields() {
        assert_eq!(
            problem_address("0.2.2", "27.2.0", true),
            format!("{REPOSITORY}/issues/new?template=bug.yml&version=0.2.2&macos=27.2.0&mac=Apple%20Silicon")
        );
        assert!(problem_address("0.2.2", "27.2.0", false).ends_with("&mac=Intel"));
        let form = include_str!("../.github/ISSUE_TEMPLATE/bug.yml");
        for id in ["id: version", "id: macos", "id: mac"] {
            assert!(form.contains(id), "{id}");
        }
    }

    #[test]
    fn the_idea_form_is_a_category_the_discussion_templates_know() {
        assert!(address(Page::Idea).ends_with("?category=ideas"));
        assert!(include_str!("../.github/DISCUSSION_TEMPLATE/ideas.yml").contains("body:"));
    }
}
