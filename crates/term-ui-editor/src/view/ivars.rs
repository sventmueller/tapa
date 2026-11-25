//! What the editor's text view holds, and how one is made over a document.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, msg_send};
use objc2_app_kit::{NSTextContainer, NSTextContentStorage, NSTextStorage};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use term_document::Document;
use term_document::recovery::RecoveryId;

use super::EditorTextView;
use term_ui::workspace_host::{TabId, Workspace, WorkspaceId};

pub(crate) struct ViewIvars {
    pub(crate) document: RefCell<Document>,
    /// Keeps the TextKit 2 text system alive; the view holds it only weakly.
    pub(super) _content: RefCell<Option<Retained<NSTextContentStorage>>>,
    /// Set while the model changes the view's text, so the change is not fed back into the model.
    pub(crate) applying: Cell<bool>,
    /// Whether a panic in one of its callbacks left the editor to be put back in step (Review B9).
    pub(super) recovering: crate::resync::Recovery,
    /// The workspace whose tab shows it.
    pub(crate) workspace: WorkspaceId,
    pub(crate) tab: Cell<Option<TabId>>,
    pub(crate) font_size: Cell<f64>,
    pub(crate) indent: RefCell<String>,
    /// How it indents and what a save tidies.
    pub(crate) spacing: RefCell<crate::indent::State>,
    /// The lines that wait to be coloured.
    pub(crate) paint: crate::highlight::Paint,
    /// The views around and over the text.
    pub(crate) around: crate::container::Around,
    /// What it knows of its file beside the document.
    pub(crate) file: crate::file::File,
    /// Tells editors apart in callbacks from other threads, which cannot hold a view.
    pub(crate) serial: crate::text_view::EditorSerial,
    /// Where the text differs from its file's last commit.
    pub(crate) marks: RefCell<crate::changes::Marks>,
    /// Vim's state while the Vim mode is on (E105).
    pub(crate) vim: RefCell<crate::vim::State>,
    /// The ranges that fold and those folded.
    pub(crate) folds: RefCell<crate::fold::Folds>,
    /// The carets beyond the first.
    pub(crate) carets: RefCell<crate::multi::Carets>,
    /// The match `reveal` marked, in UTF-16 offsets, marked in the find colour until the selection moves.
    pub(super) found: RefCell<Option<std::ops::Range<usize>>>,
    /// The notifications the editor follows, its scroll view's size and scrolling, which end with it.
    pub(crate) watches: RefCell<Vec<crate::util::Observation>>,
    /// The revision and frame size at which TextKit last laid out the whole text.
    pub(crate) laid_out: Cell<Option<(term_document::Revision, NSSize)>>,
    /// The fragments on screen as TextKit last laid them out (SN-S0).
    pub(crate) viewport: crate::viewport::Cache,
    /// Whether the view waits for its next layout pass to take its scroll view's new width (SN-S3).
    pub(crate) width_due: Cell<bool>,
}

impl EditorTextView {
    /// A text view over `document` with a TextKit 2 text system of its own, whose copy against a crash is `recovery`.
    pub(crate) fn create(
        mtm: MainThreadMarker,
        document: Document,
        workspace: &Workspace,
        recovery: RecoveryId,
    ) -> Retained<Self> {
        let content = NSTextContentStorage::new();
        let layout = objc2_app_kit::NSTextLayoutManager::new();
        content.addTextLayoutManager(&layout);
        let container = NSTextContainer::initWithSize(NSTextContainer::alloc(), NSSize::new(1.0e7, 1.0e7));
        container.setWidthTracksTextView(false);
        layout.setTextContainer(Some(&container));
        let serial = crate::text_view::new_serial();
        let ivars = ViewIvars {
            document: RefCell::new(document),
            _content: RefCell::new(Some(content)),
            applying: Cell::new(false),
            recovering: crate::resync::Recovery::default(),
            workspace: workspace.id(),
            tab: Cell::new(None),
            font_size: Cell::new(13.0),
            indent: RefCell::new("    ".into()),
            spacing: RefCell::new(crate::indent::State::default()),
            paint: crate::highlight::Paint::default(),
            around: crate::container::Around::default(),
            file: crate::file::File::new(recovery),
            serial,
            marks: RefCell::new(crate::changes::Marks::default()),
            vim: RefCell::new(Default::default()),
            folds: RefCell::new(crate::fold::Folds::default()),
            carets: RefCell::new(crate::multi::Carets::default()),
            found: RefCell::new(None),
            watches: RefCell::new(Vec::new()),
            laid_out: Cell::new(None),
            viewport: crate::viewport::Cache::default(),
            width_due: Cell::new(false),
        };
        let this = Self::alloc(mtm).set_ivars(ivars);
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
        // SAFETY: NSTextView's designated initializer with a container of its own text system.
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame, textContainer: &*container] };
        if let Some(content) = this.ivars()._content.borrow().as_ref() {
            // SAFETY: The storage holds its delegate weakly, and the view owns the storage, so the delegate lives as long.
            unsafe { content.setDelegate(Some(ProtocolObject::from_ref(&*this))) };
        }
        this
    }

    pub(crate) fn storage(&self) -> Option<Retained<NSTextStorage>> {
        // SAFETY: The view keeps its text system alive through `content`; asking for the storage keeps TextKit 2.
        unsafe { self.textStorage() }
    }

    /// Starts following the storage, after the initial text is in.
    pub(crate) fn follow_storage(&self) {
        if let Some(storage) = self.storage() {
            storage.setDelegate(Some(ProtocolObject::from_ref(self)));
        }
        crate::long_lines::opened(self);
    }

    pub(crate) fn document(&self) -> std::cell::Ref<'_, Document> {
        self.ivars().document.borrow()
    }

    pub(crate) fn document_mut(&self) -> std::cell::RefMut<'_, Document> {
        self.ivars().document.borrow_mut()
    }

    /// The workspace whose tab shows the editor, while it is open.
    pub(crate) fn controller(&self) -> Option<Workspace> {
        Workspace::open(self.ivars().workspace)
    }
}
