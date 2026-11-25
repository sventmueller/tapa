//! The disclosure chevron the panels' outline views draw themselves, in place of AppKit's disclosure button: on
//! macOS 27 `-[NSOutlineView _makeOutlineControl]` pins every button it makes with block observers it never removes,
//! so each reload of an expandable row would leave one behind. An outline view hands AppKit an empty outline cell
//! frame and calls [`place`] from `didAddRowView:forRow:` with the frame the triangle would have had.

use std::cell::RefCell;
use std::collections::HashMap;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAppearanceCustomization, NSBackgroundStyle, NSColor, NSEvent, NSEventModifierFlags, NSImage,
    NSImageSymbolConfiguration, NSOutlineView, NSTableRowView, NSUserInterfaceLayoutDirection, NSView,
};
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize};

use crate::kit::guarded;

/// The size of the chevron, as the folder tree has it.
const POINTS: f64 = 10.0;

/// What a chevron's image depends on: symbol, emphasis, appearance and the interface size.
type Look = (&'static str, bool, String, u64);

thread_local! {
    static IMAGES: RefCell<HashMap<Look, Retained<NSImage>>> = RefCell::new(HashMap::new());
}

/// The chevron `name`, made once per look, as every row's chevron draws one on each redraw.
fn image(name: &'static str, emphasized: bool, appearance: String) -> Option<Retained<NSImage>> {
    let points = crate::kit::ui_size::scaled(POINTS);
    let look = (name, emphasized, appearance, points.to_bits());
    if let Some(image) = IMAGES.with(|images| images.borrow().get(&look).cloned()) {
        return Some(image);
    }
    let color = if emphasized {
        NSColor::alternateSelectedControlTextColor()
    } else {
        NSColor::secondaryLabelColor()
    };
    let configuration = NSImageSymbolConfiguration::configurationWithPointSize_weight(
        points,
        term_render::constant!(objc2_app_kit::NSFontWeightSemibold),
    )
    .configurationByApplyingConfiguration(&NSImageSymbolConfiguration::configurationWithHierarchicalColor(&color));
    let image = crate::kit::symbol(name, "")?.imageWithSymbolConfiguration(&configuration)?;
    IMAGES.with(|images| images.borrow_mut().insert(look, image.clone()));
    Some(image)
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Chevron` does not implement Drop.
    /// A row's chevron: points down while its item is open, and opens or closes it on a click.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermOutlineChevron"]
    struct Chevron;

    unsafe impl NSObjectProtocol for Chevron {}

    impl Chevron {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || self.draw());
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            guarded((), || self.toggle(event));
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        /// The row view tells its subviews when its selection changes how they should look.
        #[unsafe(method(setBackgroundStyle:))]
        fn set_background_style(&self, _style: NSBackgroundStyle) {
            self.setNeedsDisplay(true);
        }

        #[unsafe(method(termItemOpenedOrClosed:))]
        fn item_opened_or_closed(&self, _note: &NSNotification) {
            self.setNeedsDisplay(true);
        }
    }
);

impl Chevron {
    fn new(mtm: MainThreadMarker, outline: &NSOutlineView) -> Retained<Chevron> {
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        let chevron: Retained<Chevron> = unsafe { msg_send![Chevron::alloc(mtm), init] };
        chevron.setAccessibilityElement(false);
        let center = NSNotificationCenter::defaultCenter();
        for name in [
            term_render::constant!(objc2_app_kit::NSOutlineViewItemDidExpandNotification),
            term_render::constant!(objc2_app_kit::NSOutlineViewItemDidCollapseNotification),
        ] {
            // SAFETY: The selector takes the notification; a selector-based observer goes with the chevron.
            unsafe {
                center.addObserver_selector_name_object(
                    &chevron,
                    sel!(termItemOpenedOrClosed:),
                    Some(name),
                    Some(outline),
                );
            }
        }
        chevron
    }

    fn row_view(&self) -> Option<Retained<NSTableRowView>> {
        // SAFETY: Asking for the superview only reads.
        unsafe { self.superview() }?.downcast::<NSTableRowView>().ok()
    }

    fn outline(&self) -> Option<Retained<NSOutlineView>> {
        // SAFETY: Asking for the superview only reads.
        unsafe { self.row_view()?.superview() }?
            .downcast::<NSOutlineView>()
            .ok()
    }

    /// The outline view and the item of the chevron's row.
    fn item(&self) -> Option<(Retained<NSOutlineView>, Retained<AnyObject>)> {
        let outline = self.outline()?;
        let row = outline.rowForView(self);
        let item = outline.itemAtRow(row)?;
        Some((outline, item))
    }

    fn draw(&self) {
        let Some((outline, item)) = self.item() else { return };
        // SAFETY: The item is one the outline view handed out for this row.
        let open = unsafe { outline.isItemExpanded(Some(&item)) };
        let rtl = outline.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft;
        let name = match (open, rtl) {
            (true, _) => "chevron.down",
            (false, false) => "chevron.right",
            (false, true) => "chevron.left",
        };
        let emphasized = self
            .row_view()
            .is_some_and(|row| row.interiorBackgroundStyle() == NSBackgroundStyle::Emphasized);
        let appearance = self.effectiveAppearance().name().to_string();
        let Some(image) = image(name, emphasized, appearance) else {
            return;
        };
        let size = image.size();
        let bounds = self.bounds();
        let at = NSPoint::new(
            (bounds.origin.x + (bounds.size.width - size.width) / 2.0).round(),
            (bounds.origin.y + (bounds.size.height - size.height) / 2.0).round(),
        );
        image.drawInRect(NSRect::new(at, size));
    }

    /// Opens or closes the row's item, with Option also everything below it, as AppKit's triangle does.
    fn toggle(&self, event: &NSEvent) {
        let Some((outline, item)) = self.item() else { return };
        let deep = event.modifierFlags().contains(NSEventModifierFlags::Option);
        // SAFETY: The item is one the outline view handed out for this row.
        unsafe {
            if outline.isItemExpanded(Some(&item)) {
                outline.collapseItem_collapseChildren(Some(&item), deep);
            } else {
                outline.expandItem_expandChildren(Some(&item), deep);
            }
        }
    }
}

/// Puts the chevron of `row` into its row view at `frame`, the outline view's frame of the triangle it would have
/// drawn; an empty frame, for a row that does not open, hides it.
pub fn place(outline: &NSOutlineView, row_view: &NSTableRowView, frame: NSRect) {
    let existing = row_view
        .subviews()
        .iter()
        .find_map(|view| view.downcast::<Chevron>().ok());
    if frame.size.width <= 0.0 || frame.size.height <= 0.0 {
        if let Some(chevron) = existing {
            chevron.setHidden(true);
        }
        return;
    }
    let chevron = existing.unwrap_or_else(|| {
        let chevron = Chevron::new(MainThreadMarker::from(outline), outline);
        row_view.addSubview(&chevron);
        chevron
    });
    let height = row_view.bounds().size.height;
    chevron.setFrame(NSRect::new(
        NSPoint::new(frame.origin.x, 0.0),
        NSSize::new(frame.size.width, height),
    ));
    chevron.setHidden(false);
    chevron.setNeedsDisplay(true);
}
