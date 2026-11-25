//! The views of a Markdown preview tab: a bar offering the pictures from the web that a document of an untrusted folder
//! names, over a TextKit 1 text view, the only text system that lays out the text blocks and tables of
//! `splat_render::render`.

use std::cell::Cell;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSAutoresizingMaskOptions, NSButton, NSColor, NSControlSize, NSLayoutConstraint, NSLayoutManager,
    NSScrollView, NSStackView, NSTextContainer, NSTextStorage, NSTextView, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSEdgeInsets, NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::t;

use term_ui::kit::{activate, guarded, label, live, ns, pin_sides};

/// The margin around the text, as splat's preview has it.
pub(crate) const INSET: NSSize = NSSize::new(24.0, 20.0);

const BAR_HEIGHT: f64 = 32.0;

/// A change of width this large lays the pictures out anew, as they are no wider than the text.
pub(crate) const WIDTH_STEP: f64 = 24.0;

pub(crate) struct PageIvars {
    /// The preview it shows.
    key: Cell<u64>,
    width: Cell<f64>,
    /// A drag of the window's edge or a divider changed the width; the pictures follow once it ends.
    drag_due: Cell<bool>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `Page` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermPreviewPage"]
    #[ivars = PageIvars]
    pub(crate) struct Page;

    unsafe impl NSObjectProtocol for Page {}

    impl Page {
        #[unsafe(method(termLoadWebPictures:))]
        fn load_web_pictures(&self, _sender: Option<&AnyObject>) {
            guarded((), || super::allow_web_pictures(self.ivars().key.get()));
        }

        #[unsafe(method(viewDidChangeEffectiveAppearance))]
        fn appearance_changed(&self) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), viewDidChangeEffectiveAppearance] };
            guarded((), || super::appearance_changed(self.ivars().key.get()));
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            guarded((), || self.width_may_have_changed(size.width));
        }
    }
);

impl Page {
    fn width_may_have_changed(&self, width: f64) {
        let ivars = self.ivars();
        if (width - ivars.width.get()).abs() < WIDTH_STEP {
            return;
        }
        if live::dragging(self) {
            if !ivars.drag_due.replace(true) {
                let key = ivars.key.get();
                live::after_drag(move || super::drag_ended(key));
            }
            return;
        }
        ivars.width.set(width);
        super::width_changed(ivars.key.get());
    }

    /// The drag that changed the width ended.
    pub(crate) fn drag_ended(&self) {
        self.ivars().drag_due.set(false);
        self.width_may_have_changed(self.frame().size.width);
    }
}

/// The views of one preview tab.
pub(crate) struct Views {
    pub(crate) page: Retained<Page>,
    pub(crate) scroll: Retained<NSScrollView>,
    pub(crate) text: Retained<NSTextView>,
    /// Owns the text system; a text view made from a container does not retain its storage.
    pub(crate) storage: Retained<NSTextStorage>,
    pub(crate) layout: Retained<NSLayoutManager>,
    pub(crate) container: Retained<NSTextContainer>,
    bar: Retained<NSStackView>,
    bar_height: Retained<NSLayoutConstraint>,
}

impl Views {
    /// The views of the preview `key`, which VoiceOver calls `name`.
    pub(crate) fn new(mtm: MainThreadMarker, key: u64, name: &str) -> Views {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
        let page = Page::alloc(mtm).set_ivars(PageIvars {
            key: Cell::new(key),
            width: Cell::new(frame.size.width),
            drag_due: Cell::new(false),
        });
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let page: Retained<Page> = unsafe { msg_send![super(page), initWithFrame: frame] };
        let (scroll, text, (storage, layout, container)) = text_view(mtm, frame);
        text.setAccessibilityLabel(Some(&ns(name)));
        let bar = bar(mtm, &page);
        page.addSubview(&bar);
        page.addSubview(&scroll);
        pin_sides(&bar, &page, 0.0);
        pin_sides(&scroll, &page, 0.0);
        let bar_height = bar.heightAnchor().constraintEqualToConstant(0.0);
        activate([
            bar.topAnchor().constraintEqualToAnchor(&page.topAnchor()),
            bar_height.clone(),
            scroll.topAnchor().constraintEqualToAnchor(&bar.bottomAnchor()),
            scroll.bottomAnchor().constraintEqualToAnchor(&page.bottomAnchor()),
        ]);
        bar.setHidden(true);
        Views {
            page,
            scroll,
            text,
            storage,
            layout,
            container,
            bar,
            bar_height,
        }
    }

    /// Shows or hides the offer to load the pictures from the web.
    pub(crate) fn offer_web_pictures(&self, on: bool) {
        if self.bar.isHidden() != on {
            return;
        }
        self.bar.setHidden(!on);
        self.bar_height.setConstant(if on { BAR_HEIGHT } else { 0.0 });
    }

    /// Whether the offer shows.
    pub(crate) fn offers_web_pictures(&self) -> bool {
        !self.bar.isHidden()
    }

    /// The widest a picture may be: the text's width.
    pub(crate) fn picture_width(&self) -> f64 {
        (self.scroll.contentSize().width - 2.0 * INSET.width - 4.0).max(80.0)
    }
}

type TextSystem = (
    Retained<NSTextStorage>,
    Retained<NSLayoutManager>,
    Retained<NSTextContainer>,
);

fn text_view(mtm: MainThreadMarker, frame: NSRect) -> (Retained<NSScrollView>, Retained<NSTextView>, TextSystem) {
    let storage = NSTextStorage::new();
    let layout = NSLayoutManager::new();
    storage.addLayoutManager(&layout);
    let container = NSTextContainer::initWithSize(NSTextContainer::alloc(), NSSize::new(frame.size.width, f64::MAX));
    container.setWidthTracksTextView(true);
    layout.addTextContainer(&container);
    let text = NSTextView::initWithFrame_textContainer(NSTextView::alloc(mtm), frame, Some(&container));
    text.setEditable(false);
    text.setSelectable(true);
    text.setRichText(true);
    text.setImportsGraphics(false);
    text.setUsesFindBar(true);
    text.setDrawsBackground(true);
    text.setBackgroundColor(&NSColor::textBackgroundColor());
    text.setTextContainerInset(INSET);
    text.setVerticallyResizable(true);
    text.setHorizontallyResizable(false);
    text.setMinSize(NSSize::new(0.0, frame.size.height));
    text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
    text.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(true);
    scroll.setBackgroundColor(&NSColor::textBackgroundColor());
    scroll.setDocumentView(Some(&text));
    (scroll, text, (storage, layout, container))
}

fn bar(mtm: MainThreadMarker, page: &Page) -> Retained<NSStackView> {
    let text = label(
        mtm,
        t("Der Ordner ist nicht vertraut: Bilder aus dem Netz bleiben ungeladen."),
    );
    text.setFont(Some(&term_ui::kit::style::small_system()));
    text.setTextColor(Some(&NSColor::secondaryLabelColor()));
    // The bar is there, at no height, while it is hidden too, so a text that cannot shorten would hold the preview wide.
    text.setLineBreakMode(objc2_app_kit::NSLineBreakMode::ByTruncatingTail);
    text.setContentCompressionResistancePriority_forOrientation(
        200.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    text.setToolTip(Some(&text.stringValue()));
    // SAFETY: The page implements the action, and the button lives inside the page, which it targets.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &ns(t("Bilder aus dem Netz laden")),
            Some(page),
            Some(sel!(termLoadWebPictures:)),
            mtm,
        )
    };
    button.setControlSize(NSControlSize::Small);
    button.setFont(Some(&term_ui::kit::style::small_system()));
    let bar = NSStackView::stackViewWithViews(&NSArray::from_slice(&[&*text as &NSView, &*button]), mtm);
    bar.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    bar.setSpacing(term_ui::kit::style::space::L);
    bar.setEdgeInsets(NSEdgeInsets {
        top: 4.0,
        left: 12.0,
        bottom: 4.0,
        right: 12.0,
    });
    bar
}
