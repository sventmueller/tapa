//! The native page a browser tab shows instead of a web page: before the first address, and when a page could not
//! load.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::{
    NSBezelStyle, NSButton, NSColor, NSFont, NSImageSymbolConfiguration, NSImageView, NSLayoutAttribute, NSStackView,
    NSTextAlignment, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::NSError;
use splat_i18n::{t, tf};

use super::address;
use term_ui::kit::{activate, label, ns, set_action, symbol};
#[cfg(feature = "selftest")]
mod selftest_probe;

/// What keeps a tab from showing a web page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Failure {
    /// Nothing was asked for yet.
    Start,
    /// No server answers at the address.
    Unreachable,
    /// The host name does not resolve.
    UnknownHost,
    Offline,
    TimedOut,
    /// The server's certificate, or the connection's security, is not acceptable.
    Insecure,
    /// A scheme the browser does not open.
    Unsupported,
    /// A local file outside the workspace's folder.
    OutsideProject,
    /// The page's web content process ended.
    Crashed,
    /// Anything else, with WebKit's description.
    Other(String),
}

/// The domain of the URL loading system's errors, with the codes of `NSURLError.h`.
const URL_DOMAIN: &str = "NSURLErrorDomain";
/// The domain of WebKit's policy errors, with the codes of `WebKitErrors.h`.
const WEBKIT_DOMAIN: &str = "WebKitErrorDomain";

impl Failure {
    /// The failure an error of `domain` with `code` means; `None` for errors that are no failure, such as a load the
    /// user replaced with another or one that became a download.
    pub(crate) fn of(domain: &str, code: isize, description: &str) -> Option<Failure> {
        Some(match (domain, code) {
            (URL_DOMAIN, -999) | (WEBKIT_DOMAIN, 102 | 204) => return None,
            (URL_DOMAIN, -1004 | -1005) => Failure::Unreachable,
            (URL_DOMAIN, -1003 | -1006) => Failure::UnknownHost,
            (URL_DOMAIN, -1009 | -1018 | -1020) => Failure::Offline,
            (URL_DOMAIN, -1001) => Failure::TimedOut,
            (URL_DOMAIN, -1206..=-1200 | -1022) => Failure::Insecure,
            (URL_DOMAIN, -1002 | -1000) | (WEBKIT_DOMAIN, 101) => Failure::Unsupported,
            _ => Failure::Other(description.to_string()),
        })
    }

    pub(crate) fn of_error(error: &NSError) -> Option<Failure> {
        Failure::of(
            &error.domain().to_string(),
            error.code(),
            &error.localizedDescription().to_string(),
        )
    }

    /// The SF Symbol, the heading and the explanation for a page at `url`.
    fn texts(&self, url: Option<&str>) -> (&'static str, String, String) {
        let host = url.and_then(address::host_of).unwrap_or_default();
        let shown = url.unwrap_or_default();
        match self {
            Failure::Start => (
                "globe",
                t("Browser").to_string(),
                t("Gib oben eine Adresse oder Suchbegriffe ein.").to_string(),
            ),
            Failure::Unreachable => (
                "bolt.horizontal.circle",
                tf("Keine Verbindung zu {}", &[&host]),
                if url.is_some_and(address::is_local) {
                    t("Auf dieser Adresse antwortet kein Server. Läuft der Dev-Server?").to_string()
                } else {
                    t("Der Server antwortet nicht.").to_string()
                },
            ),
            Failure::UnknownHost => (
                "questionmark.circle",
                t("Server nicht gefunden").to_string(),
                tf("Die Adresse „{}“ ließ sich nicht auflösen.", &[&host]),
            ),
            Failure::Offline => (
                "wifi.slash",
                t("Keine Internetverbindung").to_string(),
                t("Der Mac ist nicht mit dem Internet verbunden.").to_string(),
            ),
            Failure::TimedOut => (
                "clock.badge.exclamationmark",
                t("Zeitüberschreitung").to_string(),
                t("Der Server hat zu lange nicht geantwortet.").to_string(),
            ),
            Failure::Insecure => (
                "lock.trianglebadge.exclamationmark",
                t("Keine sichere Verbindung").to_string(),
                tf(
                    "Das Zertifikat von {} ist ungültig oder nicht vertrauenswürdig, deshalb lädt die Seite nicht.",
                    &[&host],
                ),
            ),
            Failure::Unsupported => (
                "nosign",
                t("Adresse nicht unterstützt").to_string(),
                tf(
                    "Der Browser öffnet Webseiten und Dateien des Projekts, nicht „{}“.",
                    &[shown],
                ),
            ),
            Failure::OutsideProject => (
                "folder.badge.minus",
                t("Datei außerhalb des Projekts").to_string(),
                t("Der Browser öffnet nur Dateien im Ordner dieses Fensters.").to_string(),
            ),
            Failure::Crashed => (
                "exclamationmark.triangle",
                t("Die Seite wurde unerwartet beendet").to_string(),
                t("Der Prozess, der die Seite anzeigte, ist abgestürzt.").to_string(),
            ),
            Failure::Other(description) => (
                "exclamationmark.triangle",
                t("Die Seite ließ sich nicht laden").to_string(),
                description.clone(),
            ),
        }
    }
}

/// The page's views, which `show` fills.
pub(crate) struct FailurePage {
    view: Retained<NSView>,
    image: Retained<NSImageView>,
    heading: Retained<NSTextField>,
    detail: Retained<NSTextField>,
    retry: Retained<NSButton>,
    /// Accepts a development server's certificate macOS does not trust.
    proceed: Retained<NSButton>,
}

impl FailurePage {
    /// The page, empty; its buttons send `termBrowserRetry:` and `termBrowserProceed:` to `target`.
    pub(crate) fn new(mtm: MainThreadMarker, target: &AnyObject) -> FailurePage {
        let image = NSImageView::new(mtm);
        image.setSymbolConfiguration(Some(&NSImageSymbolConfiguration::configurationWithPointSize_weight(
            34.0,
            term_render::constant!(objc2_app_kit::NSFontWeightLight),
        )));
        image.setContentTintColor(Some(&NSColor::tertiaryLabelColor()));
        let heading = label(mtm, "");
        heading.setFont(Some(&NSFont::boldSystemFontOfSize(term_ui::kit::style::PAGE_HEADING)));
        heading.setAlignment(NSTextAlignment::Center);
        let detail = NSTextField::wrappingLabelWithString(&ns(""), mtm);
        detail.setTextColor(Some(&NSColor::secondaryLabelColor()));
        detail.setAlignment(NSTextAlignment::Center);
        detail.setPreferredMaxLayoutWidth(380.0);
        detail.setSelectable(true);
        let button = |title: &str, action| {
            // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
            let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
            button.setBezelStyle(NSBezelStyle::Push);
            set_action(&button, target, action);
            button
        };
        let retry = button(t("Erneut versuchen"), sel!(termBrowserRetry:));
        let proceed = button(t("Trotzdem öffnen"), sel!(termBrowserProceed:));
        let buttons = NSStackView::new(mtm);
        buttons.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        buttons.setSpacing(term_ui::kit::style::space::L);
        buttons.addArrangedSubview(&retry);
        buttons.addArrangedSubview(&proceed);
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::CenterX);
        stack.setSpacing(term_ui::kit::style::space::L);
        for view in [&*image as &NSView, &heading, &detail, &buttons] {
            stack.addArrangedSubview(view);
        }
        stack.setCustomSpacing_afterView(term_ui::kit::style::space::SECTION, &detail);
        let view = NSView::new(mtm);
        view.addSubview(&stack);
        stack.setTranslatesAutoresizingMaskIntoConstraints(false);
        activate([
            stack.centerXAnchor().constraintEqualToAnchor(&view.centerXAnchor()),
            stack
                .centerYAnchor()
                .constraintEqualToAnchor_constant(&view.centerYAnchor(), -30.0),
            stack
                .leadingAnchor()
                .constraintGreaterThanOrEqualToAnchor_constant(&view.leadingAnchor(), 24.0),
            stack
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -24.0),
        ]);
        view.setHidden(true);
        FailurePage {
            view,
            image,
            heading,
            detail,
            retry,
            proceed,
        }
    }

    pub(crate) fn view(&self) -> &NSView {
        &self.view
    }

    /// Fills the page for `failure` at `url` and shows it; `proceed` offers to accept the server's certificate.
    pub(crate) fn show(&self, failure: &Failure, url: Option<&str>, proceed: bool) {
        let (symbol_name, heading, mut detail) = failure.texts(url);
        if proceed {
            detail = format!(
                "{detail}\n\n{}",
                t("Bei einem Entwicklungsserver kannst du dem Zertifikat bis zum Beenden der App vertrauen.")
            );
        }
        self.image.setImage(symbol(symbol_name, &heading).as_deref());
        self.heading.setStringValue(&ns(&heading));
        self.detail.setStringValue(&ns(&detail));
        let retry = !matches!(failure, Failure::Start | Failure::Unsupported | Failure::OutsideProject);
        self.retry.setHidden(!retry);
        self.proceed.setHidden(!proceed);
        self.view.setHidden(false);
    }

    pub(crate) fn hide(&self) {
        self.view.setHidden(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_map_to_what_the_page_explains() {
        assert_eq!(Failure::of(URL_DOMAIN, -1004, ""), Some(Failure::Unreachable));
        assert_eq!(Failure::of(URL_DOMAIN, -1202, ""), Some(Failure::Insecure));
        assert_eq!(Failure::of(URL_DOMAIN, -1022, ""), Some(Failure::Insecure));
        assert_eq!(Failure::of(URL_DOMAIN, -1003, ""), Some(Failure::UnknownHost));
        assert_eq!(
            Failure::of("OtherDomain", 7, "Broken"),
            Some(Failure::Other("Broken".into()))
        );
    }

    #[test]
    fn the_rest_of_the_url_errors_and_webkits_policy_error_map_too() {
        assert_eq!(Failure::of(URL_DOMAIN, -1009, ""), Some(Failure::Offline));
        assert_eq!(Failure::of(URL_DOMAIN, -1020, ""), Some(Failure::Offline));
        assert_eq!(Failure::of(URL_DOMAIN, -1001, ""), Some(Failure::TimedOut));
        assert_eq!(Failure::of(URL_DOMAIN, -1002, ""), Some(Failure::Unsupported));
        assert_eq!(Failure::of(WEBKIT_DOMAIN, 101, ""), Some(Failure::Unsupported));
        assert_eq!(Failure::of(WEBKIT_DOMAIN, 204, ""), None);
    }

    #[test]
    fn each_failure_has_its_own_symbol_and_heading_and_names_the_address() {
        let url = Some("https://example.invalid/page");
        let failures = [
            Failure::Start,
            Failure::Unreachable,
            Failure::UnknownHost,
            Failure::Offline,
            Failure::TimedOut,
            Failure::Insecure,
            Failure::Unsupported,
            Failure::OutsideProject,
            Failure::Crashed,
        ];
        let texts: Vec<_> = failures.iter().map(|failure| failure.texts(url)).collect();
        for (i, (symbol, heading, detail)) in texts.iter().enumerate() {
            assert!(
                !symbol.is_empty() && !heading.is_empty() && !detail.is_empty(),
                "{:?}",
                failures[i]
            );
            for (j, (_, other, _)) in texts.iter().enumerate() {
                assert!(
                    i == j || heading != other,
                    "{:?} and {:?} share {heading}",
                    failures[i],
                    failures[j]
                );
            }
        }
        assert!(texts[2].2.contains("example.invalid"), "{}", texts[2].2);
        assert!(texts[5].2.contains("example.invalid"), "{}", texts[5].2);
        assert!(texts[6].2.contains("https://example.invalid/page"), "{}", texts[6].2);
        assert_eq!(texts[8].0, "exclamationmark.triangle");
        let (symbol, heading, detail) = Failure::Other("The disk is full.".into()).texts(None);
        assert_eq!(
            (symbol, detail.as_str()),
            ("exclamationmark.triangle", "The disk is full.")
        );
        assert_ne!(heading, texts[8].1);
    }

    #[test]
    fn an_unreachable_local_server_is_asked_about_as_a_dev_server() {
        let (_, local_heading, local) = Failure::Unreachable.texts(Some("http://localhost:5173/"));
        let (_, _, remote) = Failure::Unreachable.texts(Some("https://example.org/"));
        assert!(local_heading.contains("localhost"), "{local_heading}");
        assert_ne!(local, remote);
        assert!(local.contains("Dev-Server"), "{local}");
    }

    #[test]
    fn replaced_loads_and_downloads_are_no_failure() {
        assert_eq!(Failure::of(URL_DOMAIN, -999, ""), None);
        assert_eq!(Failure::of(WEBKIT_DOMAIN, 102, ""), None);
    }
}
