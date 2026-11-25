//! Read-only rich text in a card: a TextKit 1 text view that is as tall as its text at the width it is given, so a
//! card can be measured before it is shown.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSLayoutManager, NSTextContainer, NSTextStorage, NSTextView, NSUserInterfaceLayoutDirection, NSView,
    NSViewNoIntrinsicMetric,
};
use objc2_foundation::{NSAttributedString, NSObjectProtocol, NSPoint, NSRange, NSRect, NSSize};

use super::timeline::TimelineTable;
use crate::common::util::{guarded, with_editing};

pub(crate) struct TextIvars {
    /// Owns the text system; a text view made from a container does not retain its storage.
    storage: RefCell<Option<Retained<NSTextStorage>>>,
    width: Cell<f64>,
    height: Cell<f64>,
    /// Whether AppKit is resizing the view, during which it must not scroll the timeline.
    resizing: Cell<bool>,
    streaming: RefCell<Option<super::markdown::Streaming>>,
}

define_class!(
    // SAFETY: NSTextView has no subclassing requirements and `CardText` does not implement Drop.
    #[unsafe(super(NSTextView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentCardText"]
    #[ivars = TextIvars]
    pub(crate) struct CardText;

    unsafe impl NSObjectProtocol for CardText {}

    impl CardText {
        #[unsafe(method(intrinsicContentSize))]
        fn intrinsic_content_size(&self) -> NSSize {
            NSSize::new(constant_no_metric(), self.ivars().height.get())
        }

        // Tab leaves read-only text for the view after the timeline, and ⇧Tab goes back to the timeline.
        #[unsafe(method(insertTab:))]
        fn insert_tab(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(window) = self.window() {
                    window.selectNextKeyView(Some(self));
                }
            });
        }

        #[unsafe(method(insertBacktab:))]
        fn insert_backtab(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(window) = self.window() {
                    window.selectPreviousKeyView(Some(self));
                }
            });
        }

        #[unsafe(method_id(nextValidKeyView))]
        fn next_valid_key_view(&self) -> Option<Retained<NSView>> {
            guarded(None, || self.timeline().and_then(|table| table.following_key_view()))
        }

        #[unsafe(method_id(previousValidKeyView))]
        fn previous_valid_key_view(&self) -> Option<Retained<NSView>> {
            guarded(None, || self.timeline().map(Retained::into_super).map(Retained::into_super).map(Retained::into_super))
        }

        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // NSTextView scrolls its selection into view as it resizes, which would pull the timeline away from its end.
            let outer = self.ivars().resizing.replace(true);
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            self.ivars().resizing.set(outer);
            guarded((), || {
                if (size.width - self.ivars().width.get()).abs() > 0.5 && size.width > 1.0 {
                    self.ivars().width.set(size.width);
                    self.measure();
                    self.invalidateIntrinsicContentSize();
                }
            });
        }

        #[unsafe(method(scrollRectToVisible:))]
        fn scroll_rect_to_visible(&self, rect: NSRect) -> Bool {
            if self.ivars().resizing.get() {
                return Bool::NO;
            }
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            unsafe { msg_send![super(self), scrollRectToVisible: rect] }
        }
    }
);

fn constant_no_metric() -> f64 {
    // SAFETY: AppKit sets its constants when it loads.
    unsafe { NSViewNoIntrinsicMetric }
}

impl CardText {
    /// `text`, laid out `width` points wide; selectable unless `selectable` is off.
    pub(crate) fn new(
        mtm: MainThreadMarker,
        text: &NSAttributedString,
        width: f64,
        selectable: bool,
    ) -> Retained<CardText> {
        let width = width.max(40.0);
        let storage = NSTextStorage::new();
        storage.setAttributedString(text);
        let layout = NSLayoutManager::new();
        storage.addLayoutManager(&layout);
        let container = NSTextContainer::initWithSize(NSTextContainer::alloc(), NSSize::new(width, 1.0e7));
        container.setLineFragmentPadding(0.0);
        container.setWidthTracksTextView(true);
        layout.addTextContainer(&container);
        let ivars = TextIvars {
            storage: RefCell::new(Some(storage)),
            width: Cell::new(width),
            height: Cell::new(0.0),
            resizing: Cell::new(false),
            streaming: RefCell::new(None),
        };
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(width, 20.0));
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSTextView's designated initializer, called once on the newly allocated object, with a container
        // that belongs to a layout manager of a storage this view keeps.
        let view: Retained<CardText> =
            unsafe { msg_send![super(this), initWithFrame: frame, textContainer: &*container] };
        // Laid out before `setSelectable`, which otherwise lays out the whole text itself and `measure` again after.
        view.measure();
        view.setEditable(false);
        view.setSelectable(selectable);
        view.setDrawsBackground(false);
        view.setRichText(true);
        view.setTextContainerInset(NSSize::new(0.0, 0.0));
        view.setVerticallyResizable(false);
        view.setHorizontallyResizable(false);
        view.setTranslatesAutoresizingMaskIntoConstraints(false);
        view.measure();
        view
    }

    /// The live answer keeps its parser, rendered blocks and TextKit storage between updates.
    pub(crate) fn streaming(mtm: MainThreadMarker, markdown: &str, width: f64) -> Retained<CardText> {
        let (streaming, text) = super::markdown::Streaming::start(mtm, markdown, width);
        let view = Self::new(mtm, &text, width, true);
        *view.ivars().streaming.borrow_mut() = Some(streaming);
        view
    }

    /// Replaces the affected tail while preserving the user's text selection.
    pub(crate) fn update_markdown(&self, markdown: &str) -> bool {
        let mut streaming = self.ivars().streaming.borrow_mut();
        let Some(streaming) = streaming.as_mut() else {
            return false;
        };
        let Some((text, prefix)) = streaming.render(self.mtm(), markdown, self.ivars().width.get()) else {
            return false;
        };
        let storage = self.ivars().storage.borrow();
        let Some(storage) = storage.as_ref() else { return false };
        let selected = self.selectedRange();
        with_editing(storage, || {
            storage
                .replaceCharactersInRange_withAttributedString(NSRange::new(prefix, storage.length() - prefix), &text);
        });
        let start = selected.location.min(storage.length());
        self.setSelectedRange(NSRange::new(start, selected.length.min(storage.length() - start)));
        self.measure();
        self.invalidateIntrinsicContentSize();
        true
    }

    /// The timeline the card is in.
    fn timeline(&self) -> Option<Retained<TimelineTable>> {
        // SAFETY: A view's superview outlives the walk, which holds each one retained.
        let mut next = unsafe { self.superview() };
        while let Some(view) = next {
            if let Ok(table) = view.clone().downcast::<TimelineTable>() {
                return Some(table);
            }
            // SAFETY: The getter only reads, and the returned superview is retained before the walk goes on.
            next = unsafe { view.superview() };
        }
        None
    }

    /// Keeps the text left to right in an interface that reads right to left, as code and output do.
    pub(crate) fn left_to_right(&self) {
        self.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    }

    fn measure(&self) {
        let storage = self.ivars().storage.borrow();
        let Some(storage) = storage.as_ref() else { return };
        let layouts = storage.layoutManagers();
        let Some(layout) = layouts.firstObject() else { return };
        let containers = layout.textContainers();
        let Some(container) = containers.firstObject() else {
            return;
        };
        let size = NSSize::new(self.ivars().width.get(), 1.0e7);
        // A new size invalidates the layout of the whole text, so a streamed tail at the same width lays out only
        // itself.
        if container.size() != size {
            container.setSize(size);
        }
        layout.ensureLayoutForTextContainer(&container);
        let used = layout.usedRectForTextContainer(&container);
        self.ivars().height.set(used.size.height.ceil().max(1.0));
    }
}
