//! The timeline: one table row per entry of the thread's own lane.
//!
//! Only rows in view get a card. A row's height is the measured height of its card once it was built at the
//! current width, else an estimate from the entry; a card that turns out taller or shorter than the table thought
//! corrects its row afterwards. A plain stack of cards would build all of them; `usesAutomaticRowHeights` would
//! measure text views before they know their width.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAccessibility, NSEvent, NSScrollView, NSTableColumn, NSTableView, NSTableViewSelectionHighlightStyle,
    NSTableViewStyle, NSView,
};
use objc2_foundation::{NSIndexSet, NSNotificationCenter, NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::t;
use term_thread::{Entry, EntryId, EntryKind, Lane, Thread};

use super::cards::{self, Inputs, Live, Part};
use super::target::AgentTarget;
use crate::AgentKey;
use crate::common::util::{guarded, main_soon, ns};

/// Room on the leading side for the line and its dots.
const LEADING: f64 = 30.0;
/// The leading room of a user message, which stands apart from the line.
const USER_LEADING: f64 = 8.0;
const TRAILING: f64 = 12.0;
pub(super) const TOP: f64 = 3.0;
pub(super) const BOTTOM: f64 = 10.0;

pub(crate) struct TableIvars {
    key: AgentKey,
}

define_class!(
    // SAFETY: NSTableView has no subclassing requirements and `TimelineTable` does not implement Drop.
    #[unsafe(super(NSTableView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentTimelineTable"]
    #[ivars = TableIvars]
    pub(crate) struct TimelineTable;

    unsafe impl NSObjectProtocol for TimelineTable {}

    impl TimelineTable {
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: NSSize) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            let key = self.ivars().key;
            if !self.inLiveResize() {
                main_soon(move || {
                    if let Some(tab) = super::get(key) {
                        tab.timeline().width_changed();
                    }
                });
            }
        }

        #[unsafe(method(viewDidEndLiveResize))]
        fn did_end_live_resize(&self) {
            // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
            let _: () = unsafe { msg_send![super(self), viewDidEndLiveResize] };
            guarded((), || {
                if let Some(tab) = super::get(self.ivars().key) {
                    tab.timeline().width_changed();
                }
            });
        }

        #[unsafe(method(cancelOperation:))]
        fn cancel_operation(&self, _sender: Option<&AnyObject>) {
            guarded((), || {
                if let Some(tab) = super::get(self.ivars().key)
                    && super::stop::stop_key_pressed(self.mtm())
                {
                    tab.stop_turn(super::Stop::Key);
                }
            });
        }

        // The plan in the selected row comes next for Tab: its text is selectable, so the keyboard marks passages
        // of it to comment on; the views in a table's rows are not in the window's key view loop.
        #[unsafe(method_id(nextValidKeyView))]
        fn next_valid_key_view(&self) -> Option<Retained<NSView>> {
            let plan = guarded(None, || {
                let tab = super::get(self.ivars().key)?;
                let text = tab.timeline().plan_text_of_row(self.selectedRow())?;
                let view: Retained<NSView> = Retained::into_super(Retained::into_super(text));
                (view.window().is_some() && !view.isHiddenOrHasHiddenAncestor()).then_some(view)
            });
            plan.or_else(|| self.following_key_view())
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let handled = guarded(false, || {
                let characters = event.charactersIgnoringModifiers().map(|s| s.to_string()).unwrap_or_default();
                if characters != " " && characters != "\r" {
                    return false;
                }
                let row = self.selectedRow();
                let tab = super::get(self.ivars().key);
                tab.is_some_and(|tab| tab.timeline().toggle_row(row))
            });
            if !handled {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let _: () = unsafe { msg_send![super(self), keyDown: event] };
            }
        }
    }
);

impl TimelineTable {
    /// The view Tab goes to after the timeline and the plan in it.
    pub(crate) fn following_key_view(&self) -> Option<Retained<NSView>> {
        // SAFETY: The superclass's implementation, which takes nothing and returns a view or nil.
        unsafe { msg_send![super(self), nextValidKeyView] }
    }
}

#[derive(Clone, Copy)]
pub(super) struct Correction {
    id: EntryId,
    width: f64,
    before: f64,
    after: f64,
}

pub struct Timeline {
    mtm: MainThreadMarker,
    key: AgentKey,
    pub(super) scroll: Retained<NSScrollView>,
    pub(super) table: Retained<TimelineTable>,
    target: Retained<AgentTarget>,
    cwd: PathBuf,
    thread: RefCell<Arc<Thread>>,
    rows: RefCell<Vec<EntryId>>,
    pub(super) positions: RefCell<HashMap<EntryId, usize>>,
    figures: RefCell<HashMap<EntryId, Vec<splat_render::js::Key>>>,
    pending: Cell<bool>,
    /// Whether a turn of deferred work, the height corrections and the cards built ahead, is asked for.
    deferred: Cell<bool>,
    prefetch_due: Cell<bool>,
    prefetching: Cell<bool>,
    pending_follow: Cell<bool>,
    /// The table let go of its rows while out of view, and where it was scrolled to then.
    parked: Cell<bool>,
    parked_at: Cell<f64>,
    /// The rows whose cards changed while the timeline was hidden, so showing it builds only those again; `None` once
    /// a change other than an append means rebuilding every card in view.
    behind: RefCell<Option<BTreeSet<usize>>>,
    /// The entries of the rows, in their order.
    list: RefCell<Vec<std::sync::Arc<Entry>>>,
    /// The height published to AppKit for each entry, at the committed width, initially estimated.
    heights: RefCell<HashMap<u64, (f64, f64)>>,
    /// The width the rows were last laid out at.
    width: Cell<f64>,
    /// A divider drag changed the width, and the rows are laid out anew once it ends.
    after_drag: Cell<bool>,
    pub(super) expanded: RefCell<HashSet<(u64, Part)>>,
    pub(super) inputs: RefCell<Inputs>,
    live: RefCell<Vec<Live>>,
    /// Rows whose card turned out another height than the table had, with that height.
    pub(super) corrections: RefCell<BTreeMap<usize, Correction>>,
    pub(super) built: Cell<usize>,
    kept: kept::Kept<Retained<NSView>>,
    /// Only the conversation shows: no tools, thinking or status lines.
    focus: Cell<bool>,
    follow: Rc<follow::Follow>,
    glide: follow::Glide,
    scrolled: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Drop for Timeline {
    fn drop(&mut self) {
        // SAFETY: An observer this center handed out.
        unsafe { NSNotificationCenter::defaultCenter().removeObserver(self.scrolled.as_ref()) };
    }
}

impl Timeline {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        key: AgentKey,
        target: &AgentTarget,
        thread: Arc<Thread>,
        cwd: PathBuf,
    ) -> Timeline {
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
        let this = TimelineTable::alloc(mtm).set_ivars(TableIvars { key });
        // SAFETY: NSTableView's initializer, called once on the newly allocated object.
        let table: Retained<TimelineTable> = unsafe { msg_send![super(this), initWithFrame: frame] };
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns("entry"));
        column.setResizingMask(objc2_app_kit::NSTableColumnResizingOptions::AutoresizingMask);
        table.addTableColumn(&column);
        table.setHeaderView(None);
        table.setStyle(NSTableViewStyle::Plain);
        table.setIntercellSpacing(NSSize::new(0.0, 0.0));
        table.setBackgroundColor(&objc2_app_kit::NSColor::clearColor());
        table.setSelectionHighlightStyle(NSTableViewSelectionHighlightStyle::Regular);
        table.setAllowsEmptySelection(true);
        table.setRowHeight(40.0);
        table.setUsesAutomaticRowHeights(false);
        table.setColumnAutoresizingStyle(
            objc2_app_kit::NSTableViewColumnAutoresizingStyle::FirstColumnOnlyAutoresizingStyle,
        );
        table.setAccessibilityLabel(Some(&ns(t("Verlauf der Sitzung"))));
        // SAFETY: The table holds its data source and delegate weakly; the tab keeps the target alive as long as the
        // table.
        unsafe {
            table.setDataSource(Some(ProtocolObject::from_ref(target)));
            table.setDelegate(Some(ProtocolObject::from_ref(target)));
        }
        let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&table));
        scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
        table.sizeLastColumnToFit();
        let width = scroll.contentSize().width.max(200.0);
        let list = shown(&thread, false);
        let rows: Vec<_> = list.iter().map(|e| e.id).collect();
        let positions = rows.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let follow = follow::Follow::new();
        let scrolled = follow::observe(&scroll, &follow);
        Timeline {
            mtm,
            key,
            scroll,
            table,
            target: target.retain(),
            cwd,
            thread: RefCell::new(thread),
            rows: RefCell::new(rows),
            positions: RefCell::new(positions),
            figures: RefCell::new(HashMap::new()),
            pending: Cell::new(false),
            deferred: Cell::new(false),
            prefetch_due: Cell::new(false),
            prefetching: Cell::new(false),
            pending_follow: Cell::new(false),
            parked: Cell::new(false),
            parked_at: Cell::new(0.0),
            behind: RefCell::new(Some(BTreeSet::new())),
            list: RefCell::new(list),
            heights: RefCell::new(HashMap::new()),
            width: Cell::new(width),
            after_drag: Cell::new(false),
            expanded: RefCell::new(HashSet::new()),
            inputs: RefCell::new(Inputs::default()),
            live: RefCell::new(Vec::new()),
            corrections: RefCell::new(BTreeMap::new()),
            built: Cell::new(0),
            kept: kept::Kept::default(),
            focus: Cell::new(false),
            follow,
            glide: follow::Glide::default(),
            scrolled,
        }
    }

    pub fn view(&self) -> &NSScrollView {
        &self.scroll
    }

    pub fn inputs(&self) -> &RefCell<Inputs> {
        &self.inputs
    }

    pub fn count(&self) -> usize {
        self.rows.borrow().len()
    }

    /// The width the table has now.
    fn table_width(&self) -> f64 {
        self.scroll.contentSize().width.max(200.0)
    }

    fn entry(&self, row: usize) -> Option<std::sync::Arc<Entry>> {
        self.list.borrow().get(row).cloned()
    }

    fn leading(entry: &Entry) -> f64 {
        if matches!(entry.kind, EntryKind::UserMessage(_)) {
            USER_LEADING
        } else {
            LEADING
        }
    }

    /// Opens or closes `part` of entry `id`.
    pub(crate) fn toggle(&self, id: u64, part: Part) {
        {
            let mut expanded = self.expanded.borrow_mut();
            if !expanded.remove(&(id, part)) {
                expanded.insert((id, part));
            }
        }
        self.reload_entry(id);
    }

    /// The text of the plan in `row`, which the keyboard selects passages of.
    pub fn plan_text_of_row(&self, row: isize) -> Option<Retained<objc2_app_kit::NSTextView>> {
        let entry = usize::try_from(row).ok().and_then(|r| self.entry(r))?;
        self.inputs.borrow().plan_text.get(&entry.id.get()).cloned()
    }

    /// Opens or closes the main part of the card in `row`, as Space or Return on a selected row do.
    pub(crate) fn toggle_row(&self, row: isize) -> bool {
        let Some(entry) = usize::try_from(row).ok().and_then(|r| self.entry(r)) else {
            return false;
        };
        let part = match &entry.kind {
            EntryKind::Thought(_) => Part::Thought,
            EntryKind::ToolCall(call) if matches!(call.view, term_thread::ToolView::Agent { .. }) => Part::Subagent,
            EntryKind::ToolCall(_) => Part::Output,
            EntryKind::PlanApproval(_) => Part::Plan,
            EntryKind::Notice(_) => Part::Detail,
            _ => return false,
        };
        self.toggle(entry.id.get(), part);
        true
    }

    pub(crate) fn focus_view(&self) -> bool {
        self.focus.get()
    }

    /// Shows only the conversation, or everything again.
    pub(crate) fn set_focus_view(&self, on: bool) {
        self.focus.set(on);
        self.reindex(&self.thread.borrow());
        self.table.reloadData();
        self.scroll_to_end();
    }

    /// Whether the view shows the end of the timeline.
    pub fn is_at_bottom(&self) -> bool {
        at_bottom(&self.scroll)
    }

    /// Scrolls to entry `key` and selects its row.
    pub fn reveal(&self, key: &str) {
        let thread = self.thread.borrow().clone();
        let Some(row) = owner_row(&thread, &self.positions.borrow(), key) else {
            return;
        };
        self.table.scrollRowToVisible(isize::try_from(row).unwrap_or(0));
        self.table
            .selectRowIndexes_byExtendingSelection(&NSIndexSet::indexSetWithIndex(row), false);
    }

    /// Advances the running times of the cards in view; whether any runs.
    pub(crate) fn tick(&self, now: SystemTime) -> bool {
        if !self.is_visible() {
            return false;
        }
        let mut live = self.live.borrow_mut();
        live.retain(|l| l.label.window().is_some());
        for l in live.iter() {
            l.label.setStringValue(&ns(&super::format::elapsed(
                now.duration_since(l.since).unwrap_or_default(),
            )));
        }
        !live.is_empty()
    }
}

/// Whether `entry` is a hook that ran fine, which shows only in its turn's count of such hooks.
pub(crate) fn is_quiet_hook(entry: &Entry) -> bool {
    matches!(
        &entry.kind,
        EntryKind::Notice(term_thread::Notice::Hook {
            outcome: term_thread::HookOutcome::Success,
            ..
        })
    )
}

/// Whether `entry` is a hook still running; it gets a row only once it failed, so hooks firing around every tool call
/// do not add and drop a row at the end of the timeline.
fn is_running_hook(entry: &Entry) -> bool {
    matches!(
        &entry.kind,
        EntryKind::Notice(term_thread::Notice::Hook {
            outcome: term_thread::HookOutcome::Running,
            ..
        })
    )
}

/// Whether the focus view leaves `entry` out: tools, thinking, to-dos, settled requests and status lines but errors.
fn hidden_in_focus(entry: &Entry) -> bool {
    match &entry.kind {
        EntryKind::UserMessage(_) | EntryKind::AgentMessage(_) => false,
        EntryKind::PermissionRequest(r) => r.state != term_thread::RequestState::Pending,
        EntryKind::Question(_) | EntryKind::PlanApproval(_) => false,
        EntryKind::Notice(term_thread::Notice::Error { .. }) => false,
        EntryKind::ToolCall(_) | EntryKind::Thought(_) | EntryKind::Todos(_) | EntryKind::Notice(_) => true,
    }
}

/// The entries of the main lane that get a row: hooks of a turn that ran fine share the row of the first, those
/// before the first prompt and running hooks get none, and the focus view leaves out what is not conversation.
fn shown(thread: &Thread, focus: bool) -> Vec<std::sync::Arc<Entry>> {
    let mut hook_row = true;
    let mut out = Vec::new();
    for entry in thread.entries(&Lane::Main) {
        if matches!(entry.kind, EntryKind::UserMessage(_)) {
            hook_row = false;
        }
        if is_running_hook(entry) || (focus && hidden_in_focus(entry)) {
            continue;
        }
        if is_quiet_hook(entry) {
            if hook_row {
                continue;
            }
            hook_row = true;
        }
        out.push(entry.clone());
    }
    out
}

/// The row of the main lane whose card shows the subagent or tool call `key`.
fn owner_row(thread: &Thread, rows: &HashMap<EntryId, usize>, key: &str) -> Option<usize> {
    let mut key = key.to_string();
    for _ in 0..8 {
        let entry = thread.entry_by_key(&key)?;
        if let Some(i) = rows.get(&entry.id) {
            return Some(*i);
        }
        match thread.place(entry.id) {
            Some((Lane::Subagent(parent), _)) => key = parent.clone(),
            _ => return None,
        }
    }
    None
}

fn ui_summary(view: &NSView, entry: &Entry, thread: &Thread) {
    super::ui::group(view, &cards::summary(thread, entry));
}

fn at_bottom(scroll: &NSScrollView) -> bool {
    let clip = scroll.contentView().bounds();
    let height = scroll.documentView().map_or(0.0, |table| table.frame().size.height);
    clip.origin.y + clip.size.height >= height - 40.0
}

#[cfg(test)]
#[path = "timeline_tests.rs"]
mod tests;

mod append;
mod apply;
mod follow;
mod kept;
mod park;
mod rows;
#[cfg(feature = "selftest")]
mod selftest_probe;
mod text_size;
