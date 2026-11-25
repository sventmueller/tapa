//! The standard macOS About panel with the project's address and its linked licenses, as in Dango and Splat.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, MainThreadMarker};
use objc2_app_kit::{
    NSAboutPanelOptionCredits, NSAboutPanelOptionVersion, NSApplication, NSFont, NSFontAttributeName,
    NSLinkAttributeName, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextAlignment,
};
use objc2_foundation::{NSDictionary, NSMutableAttributedString, NSRange, NSURL};
use splat_i18n::t;
use term_product::{LICENSES, REPOSITORY};
use term_render::constant;
use term_ui::kit::ns;

/// Shows the About panel.
pub(crate) fn show(mtm: MainThreadMarker) {
    let keys = [
        constant!(NSAboutPanelOptionVersion),
        constant!(NSAboutPanelOptionCredits),
    ];
    // CFBundleShortVersionString already carries the build number, which AppKit would repeat in parentheses.
    let values: [Retained<AnyObject>; 2] = [ns("").into(), credits().into()];
    let values: Vec<&AnyObject> = values.iter().map(|value| &**value).collect();
    let options = NSDictionary::from_slices(&keys, &values);
    // SAFETY: The options hold a string for the version and an attributed string for the credits.
    unsafe { NSApplication::sharedApplication(mtm).orderFrontStandardAboutPanelWithOptions(&options) };
}

/// The repository and the licenses in small, centered type, each name linked to its page.
fn credits() -> Retained<NSMutableAttributedString> {
    let address = REPOSITORY.trim_start_matches("https://");
    let shown = format!("{address}\n{}", t("Lizenz: MIT oder Apache 2.0"));
    let paragraph = NSMutableParagraphStyle::new();
    paragraph.setAlignment(NSTextAlignment::Center);
    let attributes = NSDictionary::<_, AnyObject>::from_retained_objects(
        &[constant!(NSFontAttributeName), constant!(NSParagraphStyleAttributeName)],
        &[
            NSFont::systemFontOfSize(NSFont::smallSystemFontSize()).into(),
            paragraph.into(),
        ],
    );
    // SAFETY: The attributes map the font and paragraph style keys to a font and a paragraph style.
    let text = unsafe {
        NSMutableAttributedString::initWithString_attributes(
            NSMutableAttributedString::alloc(),
            &ns(&shown),
            Some(&attributes),
        )
    };
    let links = std::iter::once((address, REPOSITORY.to_string()))
        .chain(LICENSES.map(|(name, file)| (name, format!("{REPOSITORY}/blob/main/{file}"))));
    for (name, target) in links {
        let (Some(start), Some(url)) = (shown.find(name), NSURL::URLWithString(&ns(&target))) else {
            continue;
        };
        let range = NSRange::new(shown[..start].encode_utf16().count(), name.encode_utf16().count());
        // SAFETY: The range covers the link's label in UTF-16, and the link attribute takes an NSURL.
        unsafe { text.addAttribute_value_range(constant!(NSLinkAttributeName), &url, range) };
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The page the link at the UTF-16 index `at` of `text` leads to.
    fn link_at(text: &NSMutableAttributedString, at: usize) -> Option<String> {
        // SAFETY: A null effective range is allowed, and `at` lies inside the text.
        let value =
            unsafe { text.attribute_atIndex_effectiveRange(constant!(NSLinkAttributeName), at, std::ptr::null_mut()) }?;
        let url = value.downcast_ref::<NSURL>()?;
        url.absoluteString().map(|s| s.to_string())
    }

    #[test]
    fn the_credits_name_the_repository_and_both_licenses_each_linked_to_its_page() {
        let text = credits();
        let shown = text.string().to_string();
        assert_eq!(
            shown,
            format!("github.com/sventmueller/tapa\n{}", t("Lizenz: MIT oder Apache 2.0"))
        );
        let at = |name: &str| shown[..shown.find(name).unwrap()].encode_utf16().count();
        assert_eq!(link_at(&text, 0).as_deref(), Some(REPOSITORY));
        assert_eq!(
            link_at(&text, at("MIT") + 2).as_deref(),
            Some("https://github.com/sventmueller/tapa/blob/main/LICENSE-MIT")
        );
        assert_eq!(
            link_at(&text, at("Apache 2.0") + "Apache 2.0".len() - 1).as_deref(),
            Some("https://github.com/sventmueller/tapa/blob/main/LICENSE-APACHE")
        );
        assert_eq!(link_at(&text, at("\n")), None);
    }
}
