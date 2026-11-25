//! The sidebar's place for one panel of a workspace (E22), laid out as Dango's: the way back to the workspaces at the
//! top, the panel's title with its buttons at the end of that row, and the panel below it down to the sidebar's foot.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2_app_kit::{
    NSCellImagePosition, NSColor, NSFont, NSFontWeightSemibold, NSImageSymbolConfiguration, NSLineBreakMode,
    NSStackView, NSTextField, NSUserInterfaceLayoutOrientation, NSView,
};
use splat_i18n::t;
use term_ui::kit::{self, ns, ui_size};

/// How far the title and the way back keep from the sidebar's leading edge.
const HEADER_INSET: f64 = 8.0;
/// How far the way back's middle sits below the toolbar: where the first workspace's name sits in the list.
const BACK_MIDDLE: f64 = 14.0;
/// How far the title's middle sits below the way back's.
const TITLE_DROP: f64 = 24.0;
/// How far a borderless button sets its picture in from its own edge.
const BUTTON_BEARING: f64 = 2.0;
/// The size of the way back's chevron, which is as heavy as a toolbar's.
const CHEVRON_POINTS: f64 = 10.0;
/// How far the title's buttons sit below its centre, so that symbols and capitals start at the same height.
const ACCESSORY_DROP: f64 = 1.0;
/// How far the panel keeps below its title.
const PAGE_GAP: f64 = 8.0;

/// The views of the panel's place.
pub(crate) struct Chrome {
    pub(crate) view: Retained<NSView>,
    heading: Retained<NSTextField>,
    accessory: Retained<NSStackView>,
    page: Retained<NSView>,
}

impl Chrome {
    /// The empty place, whose way back sends `back` to `target`.
    pub(crate) fn new(mtm: MainThreadMarker, target: &AnyObject, back: Sel) -> Chrome {
        let view = NSView::new(mtm);
        let button = kit::borderless_button(mtm, t("Arbeitsbereiche"), Some(target), back);
        button.setImage(kit::symbol("chevron.left", "").as_deref());
        ui_size::keep(&*button, ui_size::Aspect::Symbol, |button| {
            button.setSymbolConfiguration(Some(&NSImageSymbolConfiguration::configurationWithPointSize_weight(
                ui_size::scaled(CHEVRON_POINTS),
                // SAFETY: AppKit sets its exported font weight constants when it loads and never changes them.
                unsafe { NSFontWeightSemibold },
            )));
        });
        button.setImagePosition(NSCellImagePosition::ImageLeading);
        button.setImageHugsTitle(true);
        button.setContentTintColor(Some(&NSColor::controlAccentColor()));
        ui_size::set_system_font(&button, NSFont::smallSystemFontSize());
        button.setToolTip(Some(&ns(t("Zurück zu den Arbeitsbereichen"))));
        kit::set_label(&button, Some(t("Zurück zu den Arbeitsbereichen")));
        let heading = kit::panel::heading(mtm, "");
        heading.setLineBreakMode(NSLineBreakMode::ByTruncatingMiddle);
        let accessory = NSStackView::new(mtm);
        accessory.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        let page = NSView::new(mtm);
        for part in [&*button as &NSView, &heading, &accessory, &page] {
            part.setTranslatesAutoresizingMaskIntoConstraints(false);
            view.addSubview(part);
        }
        let top = view.safeAreaLayoutGuide().topAnchor();
        let lengths = [
            (
                button
                    .centerYAnchor()
                    .constraintEqualToAnchor_constant(&top, BACK_MIDDLE),
                BACK_MIDDLE,
            ),
            (
                heading
                    .centerYAnchor()
                    .constraintEqualToAnchor_constant(&button.centerYAnchor(), TITLE_DROP),
                TITLE_DROP,
            ),
            (
                accessory
                    .centerYAnchor()
                    .constraintEqualToAnchor_constant(&heading.centerYAnchor(), ACCESSORY_DROP),
                ACCESSORY_DROP,
            ),
            (
                page.topAnchor()
                    .constraintEqualToAnchor_constant(&heading.bottomAnchor(), PAGE_GAP),
                PAGE_GAP,
            ),
        ];
        for (constraint, base) in &lengths {
            ui_size::follow_length(constraint, *base);
        }
        kit::activate(lengths.into_iter().map(|(constraint, _)| constraint));
        kit::activate([
            button
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&view.leadingAnchor(), HEADER_INSET - BUTTON_BEARING),
            button
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -HEADER_INSET),
            heading
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&view.leadingAnchor(), HEADER_INSET),
            accessory
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&view.trailingAnchor(), -kit::panel::ACCESSORY_INSET),
            heading
                .trailingAnchor()
                .constraintLessThanOrEqualToAnchor_constant(&accessory.leadingAnchor(), -kit::space::S),
            page.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            page.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            page.bottomAnchor().constraintEqualToAnchor(&view.bottomAnchor()),
        ]);
        Chrome {
            view,
            heading,
            accessory,
            page,
        }
    }

    /// Shows `content` under `title`, with `accessory` at the end of the title's row; what it showed goes.
    pub(crate) fn show(&self, title: &str, content: Option<&NSView>, accessory: Option<&NSView>) {
        self.heading.setStringValue(&ns(title));
        for view in self.accessory.arrangedSubviews().iter() {
            if accessory.is_none_or(|accessory| !std::ptr::eq(&*view as &NSView, accessory)) {
                self.accessory.removeView(&view);
            }
        }
        if let Some(accessory) = accessory
            && self.accessory.arrangedSubviews().is_empty()
        {
            self.accessory.addArrangedSubview(accessory);
        }
        for view in self.page.subviews().iter() {
            if content.is_none_or(|content| !std::ptr::eq(&*view as &NSView, content)) {
                view.removeFromSuperview();
            }
        }
        let Some(content) = content else { return };
        // SAFETY: Asking a view for its superview on the main thread, where views live.
        let placed = unsafe { content.superview() }.is_some_and(|parent| std::ptr::eq(&*parent, &*self.page));
        if !placed {
            content.removeFromSuperview();
            self.page.addSubview(content);
            kit::pin_edges(content, &self.page);
        }
    }

    /// Lets go of the panel it showed, which may then show in another window.
    pub(crate) fn clear(&self) {
        for view in self.page.subviews().iter() {
            view.removeFromSuperview();
        }
        for view in self.accessory.arrangedSubviews().iter() {
            self.accessory.removeView(&view);
        }
    }

    /// The title it shows, for the self-test.
    pub(crate) fn title(&self) -> String {
        self.heading.stringValue().to_string()
    }
}
