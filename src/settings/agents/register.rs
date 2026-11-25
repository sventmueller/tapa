//! The window of the ACP registry as the user last fetched it, as Dango shows it: each entry with what it is, what
//! installing it would fetch and a button to install it, or to update it to the entry's version where the app has an
//! older one.

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSBackingStoreType, NSBezelStyle, NSButton, NSFont, NSLayoutAttribute, NSPanel, NSScrollView, NSStackView,
    NSTextField, NSUserInterfaceLayoutOrientation, NSView, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_acp::install::Refusal;
use term_acp::register::{Distribution, Entry, Register};
use term_acp::{Installs, Plan};
use term_ui::kit::{ns, set_action, set_label, space};

use super::super::form::secondary;
use super::install::Target;

/// The width of an entry's description.
const TEXT_WIDTH: f64 = 460.0;

thread_local! {
    /// The window, kept while the app runs, and the entries it lists in order, which its buttons' tags count.
    static WINDOW: RefCell<Option<Retained<NSPanel>>> = const { RefCell::new(None) };
    static SHOWN: RefCell<Vec<Entry>> = const { RefCell::new(Vec::new()) };
}

/// The entry the button with tag `index` stands for.
pub(super) fn shown_entry(index: usize) -> Option<Entry> {
    SHOWN.with(|shown| shown.borrow().get(index).cloned())
}

/// What `entry` offers this Mac given the version installed of it, and the title of its button, none where it has
/// none.
pub(crate) fn offer(entry: &Entry, installed: Option<&str>) -> (String, Option<String>) {
    let plan = Plan::of_entry(entry);
    if let Some(version) = installed {
        let update = plan
            .ok()
            .filter(|plan| plan.version != version)
            .map(|plan| tf("Auf {} aktualisieren", &[&plan.version]));
        return (tf("Installiert: Version {}", &[version]), update);
    }
    match plan {
        Ok(plan) => {
            let how = match entry.distributions.first() {
                Some(Distribution::Binary { sha256: Some(_), .. }) => t("Archiv mit Prüfsumme"),
                Some(Distribution::Binary { sha256: None, .. }) => t("Archiv ohne Prüfsumme"),
                Some(Distribution::Npm(_)) => t("npm-Paket"),
                Some(Distribution::Uvx(_)) => t("Python-Paket über uv"),
                None => "",
            };
            (
                tf("Version {} · {}", &[&plan.version, how]),
                Some(t("Installieren …").to_string()),
            )
        }
        Err(Refusal::Unversioned(package)) => (tf("{} nennt keine feste Version", &[&package]), None),
        Err(Refusal::Nothing { unknown }) if unknown.is_empty() => {
            (t("Für diesen Mac gibt es keine Ausgabe").to_string(), None)
        }
        Err(Refusal::Nothing { unknown }) => (tf("Nicht für diesen Mac ({})", &[&unknown.join(", ")]), None),
    }
}

fn entry_view(target: &Target, entry: &Entry, at: usize, installs: &Installs) -> Retained<NSStackView> {
    let mtm = target.mtm();
    let name = NSTextField::labelWithString(&ns(&entry.name), mtm);
    name.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
    let description = NSTextField::wrappingLabelWithString(&ns(&entry.description), mtm);
    description.setFont(Some(&NSFont::systemFontOfSize(NSFont::smallSystemFontSize())));
    description.setPreferredMaxLayoutWidth(TEXT_WIDTH);
    let installed = installs.current(&entry.id).map(|(installed, _)| installed.version);
    let (state, action) = offer(entry, installed.as_deref());
    let mut head: Vec<Retained<NSView>> = vec![Retained::into_super(Retained::into_super(name))];
    if let Some(title) = action {
        // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
        let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(&title), None, None, mtm) };
        set_action(&button, target, sel!(termAgentsInstallEntry:));
        button.setTag(isize::try_from(at).unwrap_or(-1));
        button.setBezelStyle(NSBezelStyle::Push);
        button.setEnabled(!target.busy(&entry.id));
        set_label(&button, Some(&format!("{title}: {}", entry.name)));
        head.push(Retained::into_super(Retained::into_super(button)));
    }
    let head = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&head), mtm);
    head.setSpacing(space::M);
    let views: [Retained<NSView>; 3] = [
        Retained::into_super(head),
        Retained::into_super(Retained::into_super(description)),
        Retained::into_super(Retained::into_super(secondary(mtm, &state))),
    ];
    let block = NSStackView::stackViewWithViews(&NSArray::from_retained_slice(&views), mtm);
    block.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    block.setAlignment(NSLayoutAttribute::Leading);
    block.setSpacing(space::XXS);
    block
}

fn rows(target: &Target, register: &Register) -> Retained<NSStackView> {
    let column = NSStackView::new(target.mtm());
    column.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    column.setAlignment(NSLayoutAttribute::Leading);
    column.setSpacing(space::L);
    let installs = term_ui_agent::installs();
    for (at, entry) in register.entries.iter().enumerate() {
        column.addArrangedSubview(&entry_view(target, entry, at, &installs));
    }
    SHOWN.with(|shown| *shown.borrow_mut() = register.entries.clone());
    column
}

fn panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
    WINDOW.with(|window| {
        window
            .borrow_mut()
            .get_or_insert_with(|| {
                let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
                    NSPanel::alloc(mtm),
                    NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(520.0, 560.0)),
                    NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Resizable,
                    NSBackingStoreType::Buffered,
                    false,
                );
                // SAFETY: Switching it off keeps AppKit from releasing the panel behind Rust's back; the thread-local
                // owns it.
                unsafe { panel.setReleasedWhenClosed(false) };
                panel.setTitle(&ns(t("ACP-Register")));
                panel.center();
                panel
            })
            .clone()
    })
}

/// Opens the window with the registry's entries, if the user fetched it.
pub(super) fn show(target: &Target) {
    let Some((register, _)) = term_ui_agent::cached_register() else {
        return;
    };
    let panel = panel(target.mtm());
    fill(target, &panel, &register);
    panel.makeKeyAndOrderFront(None);
}

fn fill(target: &Target, panel: &NSPanel, register: &Register) {
    let mtm = target.mtm();
    let rows = rows(target, register);
    let scroll = NSScrollView::new(mtm);
    scroll.setHasVerticalScroller(true);
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(&rows));
    rows.setTranslatesAutoresizingMaskIntoConstraints(false);
    rows.setEdgeInsets(objc2_foundation::NSEdgeInsets {
        top: 14.0,
        left: 16.0,
        bottom: 14.0,
        right: 16.0,
    });
    let clip = scroll.contentView();
    term_ui::kit::activate([
        rows.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
        rows.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
        rows.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()),
    ]);
    panel.setContentView(Some(&scroll));
}

/// Shows the window's entries again, if it is open, after an installation or a new fetch.
pub(super) fn refresh(target: &Target) {
    let open = WINDOW
        .with(|window| window.borrow().clone())
        .filter(|panel| panel.isVisible());
    if let (Some(panel), Some((register, _))) = (open, term_ui_agent::cached_register()) {
        fill(target, &panel, &register);
    }
}

/// The entries the window lists, each with its state and button, for the self-test.
#[cfg(feature = "selftest")]
pub(crate) fn window_text() -> Option<Vec<String>> {
    let open = WINDOW.with(|window| window.borrow().clone())?;
    let installs = term_ui_agent::installs();
    let lines = SHOWN.with(|shown| {
        shown
            .borrow()
            .iter()
            .map(|entry| {
                let installed = installs.current(&entry.id).map(|(installed, _)| installed.version);
                let (state, action) = offer(entry, installed.as_deref());
                let button = action.map(|a| format!(" [{a}]")).unwrap_or_default();
                format!("{}: {state}{button}", entry.name)
            })
            .collect()
    });
    open.isVisible().then_some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binary(sha256: Option<&str>) -> Distribution {
        Distribution::Binary {
            archive: "https://example.invalid/a.tar.gz".into(),
            cmd: "./agent".into(),
            args: Vec::new(),
            env: Vec::new(),
            sha256: sha256.map(str::to_string),
        }
    }

    fn entry(distributions: Vec<Distribution>, unknown: &[&str]) -> Entry {
        Entry {
            id: "agent".into(),
            name: "Agent".into(),
            version: "1.2.0".into(),
            distributions,
            unknown: unknown.iter().map(|u| u.to_string()).collect(),
            ..Entry::default()
        }
    }

    #[test]
    fn an_installed_agent_offers_only_a_newer_version() {
        let offered = offer(&entry(vec![binary(Some("ab"))], &[]), Some("1.1.0"));
        assert_eq!(
            offered,
            (
                "Installiert: Version 1.1.0".to_string(),
                Some("Auf 1.2.0 aktualisieren".to_string())
            )
        );
        assert_eq!(offer(&entry(vec![binary(Some("ab"))], &[]), Some("1.2.0")).1, None);
    }

    #[test]
    fn the_offer_says_whether_the_archive_is_checked() {
        assert_eq!(
            offer(&entry(vec![binary(Some("ab"))], &[]), None),
            (
                "Version 1.2.0 · Archiv mit Prüfsumme".to_string(),
                Some("Installieren …".to_string())
            )
        );
        assert_eq!(
            offer(&entry(vec![binary(None)], &[]), None).0,
            "Version 1.2.0 · Archiv ohne Prüfsumme"
        );
    }

    #[test]
    fn an_agent_without_a_way_onto_this_mac_names_the_ways_it_has() {
        let (text, action) = offer(&entry(Vec::new(), &["windows"]), None);
        assert!(action.is_none());
        assert_eq!(text, "Nicht für diesen Mac (windows)");
    }
}
