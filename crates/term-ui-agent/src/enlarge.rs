//! A picture's tile opening the picture large over its tab, which blurs behind it, as VS Code's Claude panel does.

use std::cell::{OnceCell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    CIImageNSAppKitAdditions, NSAccessibility, NSBezelStyle, NSBezierPath, NSBitmapImageRep, NSButton, NSColor,
    NSControlSize, NSEvent, NSImage, NSImageScaling, NSImageView, NSLayoutConstraintOrientation, NSResponder, NSShadow,
    NSView,
};
use objc2_core_image::CIImage;
use objc2_foundation::{NSObjectProtocol, NSPoint, NSPointInRect, NSRect, NSSize};
use splat_i18n::t;

use super::tab::AgentRoot;
use crate::common::util::{activate, guarded, ns, pin_edges, set_action, set_label, symbol_button};

/// The room kept free around the picture, wide enough for the close button above it.
const MARGIN: f64 = 56.0;
const CLOSE_INSET: f64 = 14.0;
const RADIUS: f64 = 10.0;
/// The blur's sigma, in points.
const BLUR: f64 = 5.0;
const ESCAPE: u16 = 53;
/// Below the window's own resizing, so the picture never holds the window at its size.
const LOOSE: f32 = 1.0;
/// Below the holding priorities of the split views around the tab, so a picture wider than its tab never pushes the
/// sidebar or a document beside it aside.
const NATURAL_SIZE: f32 = 200.0;

thread_local! {
    static OPENER: OnceCell<Retained<Opener>> = const { OnceCell::new() };
}

/// The picture shown large.
#[derive(Default)]
pub(crate) struct EnlargedPicture {
    shown: RefCell<Option<Retained<Shade>>>,
}

fn with_shown<R>(f: impl FnOnce(&RefCell<Option<Retained<Shade>>>) -> R) -> R {
    f(&crate::host::state().enlarged_picture.shown)
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and `Opener` does not implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentPictureOpener"]
    struct Opener;

    unsafe impl NSObjectProtocol for Opener {}

    impl Opener {
        #[unsafe(method(ideAgentEnlargePicture:))]
        fn open(&self, sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(button) = sender.and_then(|s| s.downcast_ref::<NSButton>()) {
                    show(button);
                }
            });
        }

        #[unsafe(method(ideAgentClosePicture:))]
        fn close_picture(&self, _sender: Option<&AnyObject>) {
            guarded((), close);
        }
    }
);

pub(crate) struct ShadeIvars {
    close: Retained<NSButton>,
    picture: Retained<NSView>,
    before: RefCell<Option<Retained<NSResponder>>>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Shade` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentPictureShade"]
    #[ivars = ShadeIvars]
    struct Shade;

    unsafe impl NSObjectProtocol for Shade {}

    impl Shade {
        /// The tab's own background, so the tab never shows through sharp where the blur does not reach.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || {
                NSColor::textBackgroundColor().setFill();
                NSBezierPath::fillRect(self.bounds());
            });
        }

        /// Everything but the close button answers as the shade, so the tab behind takes no clicks.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> *mut NSView {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            let Some(hit) = hit else {
                return std::ptr::null_mut();
            };
            let close: &NSView = &self.ivars().close;
            let view: &NSView = if hit.isDescendantOf(close) { close } else { self };
            std::ptr::from_ref(view).cast_mut()
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            guarded((), || {
                let point = self.convertPoint_fromView(event.locationInWindow(), None);
                if !NSPointInRect(point, self.ivars().picture.frame()) {
                    close();
                }
            });
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, _event: &NSEvent) {}

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let closed = guarded(false, || {
                let escape = event.keyCode() == ESCAPE;
                if escape {
                    close();
                }
                escape
            });
            if !closed {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            guarded((), close);
        }
    }
);

fn opener(mtm: MainThreadMarker) -> Retained<Opener> {
    OPENER.with(|cell| {
        cell.get_or_init(|| {
            let this = Opener::alloc(mtm).set_ivars(());
            // SAFETY: NSObject's initializer, called once on the newly allocated object.
            unsafe { msg_send![super(this), init] }
        })
        .clone()
    })
}

/// A clear button over a tile's picture, named `name`, that opens the picture large.
pub(crate) fn button(mtm: MainThreadMarker, name: &str) -> Retained<NSButton> {
    // SAFETY: A nil target sends the action along the responder chain; `set_action` gives it the opener right after.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(""), None, None, mtm) };
    set_action(&button, &opener(mtm), sel!(ideAgentEnlargePicture:));
    button.setBordered(false);
    button.setTransparent(true);
    button.setToolTip(Some(&ns(name)));
    set_label(&button, Some(name));
    button
}

/// Opens the picture of the tile `button` lies on over the tab the tile is in.
fn show(button: &NSButton) {
    // SAFETY: Asking for the superview only reads; the tile holds the button while it is clicked.
    let Some(tile) = (unsafe { button.superview() }) else {
        return;
    };
    let Some(image) = picture_in(&tile) else { return };
    let Some(root) = tab_root(&tile) else { return };
    close();
    let backdrop = blurred(&root);
    let shade = shade(
        button.mtm(),
        &image,
        backdrop.as_deref(),
        button.accessibilityLabel().as_deref(),
    );
    root.addSubview(&shade);
    pin_edges(&shade, &root);
    if let Some(window) = root.window() {
        *shade.ivars().before.borrow_mut() = window.firstResponder();
        window.makeFirstResponder(Some(&shade));
    }
    with_shown(|shown| *shown.borrow_mut() = Some(shade));
}

fn close() {
    let Some(shade) = with_shown(|shown| shown.borrow_mut().take()) else {
        return;
    };
    let before = shade.ivars().before.borrow_mut().take();
    let window = shade.window();
    shade.removeFromSuperview();
    if let (Some(window), Some(before)) = (window, before) {
        window.makeFirstResponder(Some(&before));
    }
}

fn tab_root(view: &NSView) -> Option<Retained<NSView>> {
    // SAFETY: Asking for the superview only reads.
    let mut current = unsafe { view.superview() };
    while let Some(view) = current {
        if view.downcast_ref::<AgentRoot>().is_some() {
            return Some(view);
        }
        // SAFETY: The getter only reads, and the returned superview is retained before the walk goes on.
        current = unsafe { view.superview() };
    }
    None
}

fn picture_in(view: &NSView) -> Option<Retained<NSImage>> {
    view.subviews()
        .iter()
        .find_map(|child| child.downcast_ref::<NSImageView>().and_then(|v| v.image()))
}

/// What `view` shows now, blurred.
fn blurred(view: &NSView) -> Option<Retained<NSImage>> {
    let bounds = view.bounds();
    let bitmap = view.bitmapImageRepForCachingDisplayInRect(bounds)?;
    view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
    let scale = bitmap.pixelsWide() as f64 / bounds.size.width.max(1.0);
    let source = CIImage::initWithBitmapImageRep(CIImage::alloc(), &bitmap)?;
    // SAFETY: Each call makes a new image from one that lives through it. Clamped first, the blur keeps the edges
    // opaque instead of fading them out; the crop takes the image back to its own size.
    let blurred = unsafe {
        source
            .imageByClampingToExtent()
            .imageByApplyingGaussianBlurWithSigma(BLUR * scale)
            .imageByCroppingToRect(source.extent())
    };
    // Rendered here into pixels: an image view drawing through its layer shows a lazily rendered CIImage as nothing.
    let rendered = NSBitmapImageRep::initWithCIImage(NSBitmapImageRep::alloc(), &blurred);
    rendered.setSize(bounds.size);
    let image = NSImage::initWithSize(NSImage::alloc(), bounds.size);
    image.addRepresentation(&rendered);
    Some(image)
}

/// The layer over a tab: `backdrop` filling it, `image` in the middle as large as it fits, and a close button.
fn shade(
    mtm: MainThreadMarker,
    image: &NSImage,
    backdrop: Option<&NSImage>,
    label: Option<&objc2_foundation::NSString>,
) -> Retained<Shade> {
    let picture = framed_picture(mtm, image, label);
    let close = symbol_button(
        mtm,
        "xmark",
        t("Schließen"),
        13.0,
        Some(&opener(mtm)),
        sel!(ideAgentClosePicture:),
    );
    close.setBordered(true);
    close.setBezelStyle(NSBezelStyle::Circular);
    close.setControlSize(NSControlSize::Large);
    set_label(&close, Some(t("Schließen")));
    let this = Shade::alloc(mtm).set_ivars(ShadeIvars {
        close: close.clone(),
        picture: picture.clone(),
        before: RefCell::new(None),
    });
    // SAFETY: NSView's initializer, called once on the newly allocated object.
    let shade: Retained<Shade> = unsafe { msg_send![super(this), init] };
    let behind = NSImageView::new(mtm);
    behind.setImage(backdrop);
    behind.setImageScaling(NSImageScaling::ScaleAxesIndependently);
    behind.setAccessibilityElement(false);
    loosen(&behind);
    shade.addSubview(&behind);
    pin_edges(&behind, &shade);
    shade.addSubview(&picture);
    place(&picture, &shade, image.size());
    shade.addSubview(&close);
    close.setTranslatesAutoresizingMaskIntoConstraints(false);
    activate([
        close
            .topAnchor()
            .constraintEqualToAnchor_constant(&shade.topAnchor(), CLOSE_INSET),
        close
            .trailingAnchor()
            .constraintEqualToAnchor_constant(&shade.trailingAnchor(), -CLOSE_INSET),
    ]);
    shade
}

/// `image` with rounded corners on a shadow, or as it is if it brings its own margin, as a window's screenshot does.
fn framed_picture(
    mtm: MainThreadMarker,
    image: &NSImage,
    label: Option<&objc2_foundation::NSString>,
) -> Retained<NSView> {
    let frame = NSView::new(mtm);
    let view = NSImageView::new(mtm);
    view.setImage(Some(image));
    view.setImageScaling(NSImageScaling::ScaleProportionallyUpOrDown);
    view.setAccessibilityLabel(label);
    loosen(&view);
    frame.addSubview(&view);
    pin_edges(&view, &frame);
    if clear_corner(image) {
        return frame;
    }
    frame.setWantsLayer(true);
    let shadow = NSShadow::new();
    shadow.setShadowColor(Some(&NSColor::colorWithWhite_alpha(0.0, 0.3)));
    shadow.setShadowBlurRadius(24.0);
    shadow.setShadowOffset(NSSize::new(0.0, -6.0));
    frame.setShadow(Some(&shadow));
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(RADIUS);
        layer.setMasksToBounds(true);
        layer.setBorderWidth(1.0);
        layer.setBorderColor(Some(&NSColor::colorWithWhite_alpha(0.0, 0.12).CGColor()));
    }
    frame
}

/// Whether `image`'s top left pixel is see-through, the margin a window's screenshot keeps for its shadow.
fn clear_corner(image: &NSImage) -> bool {
    image.representations().iter().any(|rep| {
        rep.downcast_ref::<NSBitmapImageRep>()
            .filter(|bitmap| bitmap.hasAlpha())
            .and_then(|bitmap| bitmap.colorAtX_y(0, 0))
            .is_some_and(|color| color.alphaComponent() < 0.5)
    })
}

/// Centres `picture` in `shade` at the size of `natural`, shrunk to fit inside the margins, keeping its shape.
fn place(picture: &NSView, shade: &NSView, natural: NSSize) {
    picture.setTranslatesAutoresizingMaskIntoConstraints(false);
    let (width, height) = (natural.width.max(1.0), natural.height.max(1.0));
    let preferred = picture.widthAnchor().constraintEqualToConstant(width);
    preferred.setPriority(NATURAL_SIZE);
    activate([
        picture.centerXAnchor().constraintEqualToAnchor(&shade.centerXAnchor()),
        picture.centerYAnchor().constraintEqualToAnchor(&shade.centerYAnchor()),
        picture
            .widthAnchor()
            .constraintEqualToAnchor_multiplier(&picture.heightAnchor(), width / height),
        picture.widthAnchor().constraintLessThanOrEqualToConstant(width),
        picture
            .widthAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&shade.widthAnchor(), -2.0 * MARGIN),
        picture
            .heightAnchor()
            .constraintLessThanOrEqualToAnchor_constant(&shade.heightAnchor(), -2.0 * MARGIN),
        preferred,
    ]);
}

fn loosen(view: &NSView) {
    for orientation in [
        NSLayoutConstraintOrientation::Horizontal,
        NSLayoutConstraintOrientation::Vertical,
    ] {
        view.setContentHuggingPriority_forOrientation(LOOSE, orientation);
        view.setContentCompressionResistancePriority_forOrientation(LOOSE, orientation);
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
