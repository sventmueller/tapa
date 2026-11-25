//! What lies over a session's screen for its command blocks: the gutter that marks each command, red for a failed one
//! (F82), bookmarks (F87), the line of a collapsed block's hidden output (F78), the header of the command whose
//! prompt scrolled away (F84) and, over the block under the pointer, its status with buttons to copy, save and
//! collapse its output (F79, F81). Clicks pass through to the terminal except on these.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSBezierPath, NSButton, NSColor, NSEvent, NSFont, NSStackView, NSTextField,
    NSTrackingArea, NSTrackingAreaOptions, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSEdgeInsets, NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_render::render::{INSET_X, INSET_Y};
use term_session::session::{BlocksView, ShownBlock};

use super::model::{self, Gutter};
use crate::kit::{guarded, label, ns, set_action, symbol_button};

/// The width of the bar in the gutter.
const BAR: f64 = 3.0;

/// Room kept free at the right edge for the scrollbar.
const RIGHT: f64 = 18.0;

/// What the overlay of one terminal view keeps.
pub(crate) struct Ivars {
    view: u64,
    shown: RefCell<BlocksView>,
    cell_height: Cell<f64>,
    header: Retained<NSTextField>,
    bar: Retained<NSStackView>,
    status: Retained<NSTextField>,
    copy: Retained<NSButton>,
    save: Retained<NSButton>,
    fold: Retained<NSButton>,
    chips: RefCell<Vec<Retained<NSTextField>>>,
    /// The block under the pointer, whose bar shows.
    hover: Cell<Option<u64>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `BlockOverlay` does not implement Drop.
    /// The command blocks over a terminal view's screen.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermBlockOverlay"]
    #[ivars = Ivars]
    pub(crate) struct BlockOverlay;

    unsafe impl NSObjectProtocol for BlockOverlay {}

    impl BlockOverlay {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || self.draw());
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            guarded(None, || self.hit(point))
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            guarded((), || self.clicked(event));
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            guarded((), || self.pointer_at(Some(event)));
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            guarded((), || self.pointer_at(None));
        }

        #[unsafe(method(termBlockCopy:))]
        fn copy_action(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                let copied = self.with_hovered(|view, id| view.copy_block_output(Some(id))).unwrap_or(false);
                if copied && let Some(button) = sender.and_then(|sender| sender.downcast_ref::<NSButton>()) {
                    crate::kit::acknowledge(button, "checkmark", t("Kopiert"));
                }
            });
        }

        #[unsafe(method(termBlockSave:))]
        fn save_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                self.with_hovered(|view, id| view.save_block_output(Some(id)));
            });
        }

        #[unsafe(method(termBlockFold:))]
        fn fold_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                self.with_hovered(|view, id| view.toggle_block(Some(id)));
            });
        }
    }
);

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.y >= rect.origin.y
        && point.x < rect.origin.x + rect.size.width
        && point.y < rect.origin.y + rect.size.height
}

fn small_label(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let label = label(mtm, "");
    label.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        11.0,
        term_render::constant!(objc2_app_kit::NSFontWeightRegular),
    )));
    label.setTextColor(Some(&NSColor::secondaryLabelColor()));
    label
}

/// Puts a system material with rounded corners behind `bar`, as macOS's floating bars have.
fn material_behind(mtm: MainThreadMarker, bar: &NSStackView) {
    let material = objc2_app_kit::NSVisualEffectView::new(mtm);
    material.setMaterial(objc2_app_kit::NSVisualEffectMaterial::Popover);
    material.setBlendingMode(objc2_app_kit::NSVisualEffectBlendingMode::WithinWindow);
    material.setState(objc2_app_kit::NSVisualEffectState::Active);
    material.setWantsLayer(true);
    if let Some(layer) = material.layer() {
        layer.setCornerRadius(6.0);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(0.5);
        layer.setBorderColor(Some(&NSColor::separatorColor().CGColor()));
    }
    bar.addSubview_positioned_relativeTo(&material, objc2_app_kit::NSWindowOrderingMode::Below, None);
    crate::kit::pin_edges(&material, bar);
}

fn rounded(view: &NSView, background: &NSColor) {
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setBackgroundColor(Some(&background.CGColor()));
        layer.setCornerRadius(5.0);
    }
}

impl BlockOverlay {
    /// The overlay for terminal view `view`, as big as `frame`.
    pub(crate) fn new(mtm: MainThreadMarker, view: u64, frame: NSRect) -> Retained<BlockOverlay> {
        let header = label(mtm, "");
        header.setDrawsBackground(true);
        header.setHidden(true);
        let status = small_label(mtm);
        let placeholder = sel!(termBlockCopy:);
        let copy = symbol_button(mtm, "doc.on.doc", t("Ausgabe kopieren"), 12.0, None, placeholder);
        let save = symbol_button(
            mtm,
            "square.and.arrow.down",
            t("Ausgabe sichern …"),
            12.0,
            None,
            placeholder,
        );
        let fold = symbol_button(mtm, "chevron.up", t("Ausgabe einklappen"), 12.0, None, placeholder);
        let bar = NSStackView::new(mtm);
        bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        bar.setSpacing(6.0);
        bar.setEdgeInsets(NSEdgeInsets {
            top: 2.0,
            left: 8.0,
            bottom: 2.0,
            right: 6.0,
        });
        for item in [&*status as &NSView, &copy, &save, &fold] {
            bar.addArrangedSubview(item);
        }
        for button in [&copy, &save, &fold] {
            crate::kit::activate([
                button.widthAnchor().constraintGreaterThanOrEqualToConstant(22.0),
                button.heightAnchor().constraintGreaterThanOrEqualToConstant(22.0),
            ]);
        }
        material_behind(mtm, &bar);
        bar.setHidden(true);
        let ivars = Ivars {
            view,
            shown: RefCell::new(BlocksView::default()),
            cell_height: Cell::new(16.0),
            header,
            bar,
            status,
            copy,
            save,
            fold,
            chips: RefCell::new(Vec::new()),
            hover: Cell::new(None),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let ivars = this.ivars();
        set_action(&ivars.copy, &this, sel!(termBlockCopy:));
        set_action(&ivars.save, &this, sel!(termBlockSave:));
        set_action(&ivars.fold, &this, sel!(termBlockFold:));
        this.addSubview(&ivars.header);
        this.addSubview(&ivars.bar);
        let options = NSTrackingAreaOptions::MouseMoved
            | NSTrackingAreaOptions::MouseEnteredAndExited
            | NSTrackingAreaOptions::ActiveInKeyWindow
            | NSTrackingAreaOptions::InVisibleRect;
        // SAFETY: A tracking area holds its owner weakly, and the overlay owns the area.
        let area = unsafe {
            NSTrackingArea::initWithRect_options_owner_userInfo(
                NSTrackingArea::alloc(),
                NSRect::ZERO,
                options,
                Some(&this),
                None,
            )
        };
        this.addTrackingArea(&area);
        this
    }

    /// Shows `view` with lines `cell_height` high; the header in `font`, `foreground` on `background`.
    pub(crate) fn show(&self, view: BlocksView, cell_height: f64, font: &NSFont, colours: (&NSColor, &NSColor)) {
        let ivars = self.ivars();
        ivars.cell_height.set(cell_height);
        *ivars.shown.borrow_mut() = view;
        self.place_header(font, colours);
        self.place_chips();
        self.place_bar();
        self.setNeedsDisplay(true);
    }

    fn line_top(&self, line: usize) -> f64 {
        INSET_Y + line as f64 * self.ivars().cell_height.get()
    }

    fn line_at(&self, y: f64) -> Option<usize> {
        let line = (y - INSET_Y) / self.ivars().cell_height.get();
        (line >= 0.0).then(|| term_render::render::clamp_to(line.floor(), usize::MAX))
    }

    fn place_header(&self, font: &NSFont, (foreground, background): (&NSColor, &NSColor)) {
        let ivars = self.ivars();
        let shown = ivars.shown.borrow();
        let header = &ivars.header;
        let Some(sticky) = shown.sticky.as_ref() else {
            header.setHidden(true);
            return;
        };
        let parts = model::status_parts(&sticky.block, std::time::SystemTime::now(), &term_base::home());
        let mut text = sticky.heading.clone();
        if !parts.is_empty() {
            text = format!("{text}   {}", parts.join(" · "));
        }
        header.setStringValue(&ns(&text));
        header.setFont(Some(font));
        header.setTextColor(Some(foreground));
        header.setBackgroundColor(Some(&background.colorWithAlphaComponent(0.96)));
        header.setToolTip(Some(&ns(t("Zum Befehl springen"))));
        let width = self.bounds().size.width;
        let height = INSET_Y + ivars.cell_height.get();
        header.setFrame(NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new((width - RIGHT).max(0.0), height),
        ));
        header.setHidden(false);
    }

    /// The line `n Zeilen ausgeblendet` at the end of each collapsed block's prompt.
    fn place_chips(&self) {
        let ivars = self.ivars();
        let shown = ivars.shown.borrow();
        let mut chips = ivars.chips.borrow_mut();
        let folded: Vec<&ShownBlock> = shown.blocks.iter().filter(|block| block.folded).collect();
        while chips.len() < folded.len() {
            let chip = small_label(self.mtm());
            rounded(&chip, &NSColor::quaternaryLabelColor());
            self.addSubview(&chip);
            chips.push(chip);
        }
        let width = self.bounds().size.width;
        for (at, chip) in chips.iter().enumerate() {
            let Some(block) = folded.get(at) else {
                chip.setHidden(true);
                continue;
            };
            chip.setStringValue(&ns(&format!(
                " {} ",
                tf("⋯ {} Zeilen ausgeblendet", &[&block.output_lines.to_string()])
            )));
            chip.sizeToFit();
            let size = chip.frame().size;
            let line = block.prompt_line.unwrap_or(block.lines.0);
            let y = self.line_top(line) + (ivars.cell_height.get() - size.height) / 2.0;
            chip.setFrameOrigin(NSPoint::new((width - RIGHT - size.width - 4.0).max(0.0), y));
            chip.setHidden(false);
        }
    }

    fn place_bar(&self) {
        let ivars = self.ivars();
        let shown = ivars.shown.borrow();
        let block = ivars
            .hover
            .get()
            .and_then(|id| shown.blocks.iter().find(|block| block.block.id == id))
            .filter(|block| model::gutter(&block.block).is_some());
        let Some(block) = block else {
            ivars.bar.setHidden(true);
            return;
        };
        let mut parts = model::status_parts(&block.block, std::time::SystemTime::now(), &term_base::home());
        if let Some(started) = block.block.started {
            parts.insert(parts.len().min(2), super::clock(started));
        }
        ivars.status.setStringValue(&ns(&parts.join(" · ")));
        let has_output = block.output_lines > 0;
        ivars.copy.setEnabled(has_output);
        ivars.save.setEnabled(has_output);
        let (symbol, tip) = if block.folded {
            ("chevron.down", t("Ausgabe ausklappen"))
        } else {
            ("chevron.up", t("Ausgabe einklappen"))
        };
        if let Some(image) = crate::kit::symbol(symbol, tip) {
            ivars.fold.setImage(Some(&image));
        }
        ivars.fold.setToolTip(Some(&ns(tip)));
        ivars
            .fold
            .setEnabled(has_output && model::gutter(&block.block) != Some(Gutter::Running));
        let size = ivars.bar.fittingSize();
        let width = self.bounds().size.width;
        let header = if shown.sticky.is_some() {
            INSET_Y + ivars.cell_height.get()
        } else {
            0.0
        };
        let line = block.prompt_line.unwrap_or(block.lines.0);
        let y = self.line_top(line).max(header + 2.0);
        ivars.bar.setFrame(NSRect::new(
            NSPoint::new((width - RIGHT - size.width).max(0.0), y),
            size,
        ));
        ivars.bar.setHidden(false);
    }

    fn draw(&self) {
        let ivars = self.ivars();
        let shown = ivars.shown.borrow();
        let width = self.bounds().size.width;
        let height = ivars.cell_height.get();
        for block in &shown.blocks {
            let Some(gutter) = model::gutter(&block.block) else {
                continue;
            };
            let top = self.line_top(block.lines.0);
            let bottom = self.line_top(block.lines.1) + height;
            let current = shown.current == Some(block.block.id) || ivars.hover.get() == Some(block.block.id);
            let colour = match gutter {
                Gutter::Failed => NSColor::systemRedColor(),
                Gutter::Running => NSColor::controlAccentColor(),
                Gutter::Done if current => NSColor::secondaryLabelColor(),
                Gutter::Done => NSColor::tertiaryLabelColor(),
            };
            if gutter == Gutter::Failed {
                NSColor::systemRedColor().colorWithAlphaComponent(0.07).setFill();
                NSBezierPath::fillRect(NSRect::new(NSPoint::new(0.0, top), NSSize::new(width, bottom - top)));
            }
            colour.setFill();
            let bar = if current { BAR + 1.0 } else { BAR };
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new((INSET_X - bar) / 2.0, top + 1.0),
                NSSize::new(bar, (bottom - top - 2.0).max(1.0)),
            ));
        }
        if !ivars.header.isHidden() {
            let below = ivars.header.frame();
            NSColor::separatorColor().setFill();
            NSBezierPath::fillRect(NSRect::new(
                NSPoint::new(0.0, below.origin.y + below.size.height),
                NSSize::new(below.size.width, 1.0),
            ));
        }
        NSColor::systemOrangeColor().setFill();
        for &line in &shown.bookmarks {
            let middle = self.line_top(line) + height / 2.0;
            let path = NSBezierPath::bezierPath();
            path.moveToPoint(NSPoint::new(0.0, middle - 4.0));
            path.lineToPoint(NSPoint::new(INSET_X - 1.0, middle));
            path.lineToPoint(NSPoint::new(0.0, middle + 4.0));
            path.closePath();
            path.fill();
        }
    }

    fn hit(&self, point: NSPoint) -> Option<Retained<NSView>> {
        if self.isHidden() {
            return None;
        }
        let ivars = self.ivars();
        // The point is in the terminal view's coordinates; both are flipped and the overlay lies at its origin.
        let origin = self.frame().origin;
        let local = NSPoint::new(point.x - origin.x, point.y - origin.y);
        let inside = |view: &NSView| !view.isHidden() && contains(view.frame(), local);
        if inside(&ivars.bar) {
            return ivars.bar.hitTest(local);
        }
        if inside(&ivars.header) || (local.x < INSET_X && self.gutter_block(local.y).is_some()) {
            return Some(self.retain().into_super());
        }
        None
    }

    fn gutter_block(&self, y: f64) -> Option<u64> {
        let line = self.line_at(y)?;
        let shown = self.ivars().shown.borrow();
        model::block_at(&shown, line)
            .filter(|block| model::gutter(&block.block).is_some())
            .map(|block| block.block.id)
    }

    /// A click on the header jumps to its command, one in the gutter selects the block there.
    fn clicked(&self, event: &NSEvent) {
        let local = self.convertPoint_fromView(event.locationInWindow(), None);
        let ivars = self.ivars();
        let id = if !ivars.header.isHidden() && contains(ivars.header.frame(), local) {
            ivars.shown.borrow().sticky.as_ref().map(|sticky| sticky.block.id)
        } else {
            self.gutter_block(local.y)
        };
        if let (Some(id), Some(view)) = (id, crate::find(ivars.view)) {
            view.select_block(Some(id));
        }
    }

    fn pointer_at(&self, event: Option<&NSEvent>) {
        let ivars = self.ivars();
        let id = event.and_then(|event| {
            let local = self.convertPoint_fromView(event.locationInWindow(), None);
            let line = self.line_at(local.y)?;
            let shown = ivars.shown.borrow();
            model::block_at(&shown, line).map(|block| block.block.id)
        });
        if ivars.hover.replace(id) != id {
            self.place_bar();
            self.setNeedsDisplay(true);
        }
    }

    fn with_hovered<R>(&self, f: impl FnOnce(&crate::TerminalView, u64) -> R) -> Option<R> {
        let id = self.ivars().hover.get()?;
        let view = crate::find(self.ivars().view)?;
        Some(f(&view, id))
    }
}
