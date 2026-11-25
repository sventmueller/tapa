//! The output of a background task in a document tab: read again while the task runs, kept at its end unless the user
//! scrolled up.

use std::cell::{Cell, OnceCell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Weak as RcWeak;
use std::time::{Duration, Instant};

use crate::common::constant;
use crate::common::util::{guarded, label, main_after, main_soon, ns, pin_edges, pin_sides, set_action, with_editing};
use dispatch2::MainThreadBound;
use objc2::rc::{Retained, Weak};
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAccessibility, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSFont, NSLayoutAttribute,
    NSLineBreakMode, NSScrollView, NSStackView, NSStackViewGravity, NSStandardKeyBindingResponding, NSTextField,
    NSTextView, NSUserInterfaceLayoutDirection, NSUserInterfaceLayoutOrientation, NSView, NSWritingDirection,
};
use objc2_foundation::{NSEdgeInsets, NSObjectProtocol, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_ui::workspace_host::{Files, TabKind, Tabs};

use super::cards::status_text;
use super::tab::AgentTab;
use super::task_output::{self, Change, Found, MAX_SHOWN};

/// How often a shown tab reads the output of a running task again.
const EVERY: Duration = Duration::from_secs(1);
/// After how long a read that has not answered no longer holds the next one back.
const STALE: Duration = Duration::from_secs(5);
/// How many reads follow the end of the task, for the file the agent names with it.
const AFTER_END: u8 = 2;

pub(crate) struct OutputIvars {
    agent: RcWeak<AgentTab>,
    task: String,
    ui: OnceCell<OutputUi>,
    /// The file shown and how far it is read; none while the agent's text is shown.
    file: RefCell<Option<(PathBuf, u64)>>,
    /// The agent's text as shown, which the next one extends.
    text: RefCell<String>,
    /// The number of the latest read and when it started, while it runs.
    reads: Cell<u64>,
    reading: Cell<Option<Instant>>,
    /// Whether the next output goes to the end, as when the tab opens.
    to_end: Cell<bool>,
    reads_after_end: Cell<u8>,
}

pub(crate) struct OutputUi {
    text: Retained<NSTextView>,
    scroll: Retained<NSScrollView>,
    state: Retained<NSTextField>,
    open: Retained<NSButton>,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `OutputTab` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentOutputTab"]
    #[ivars = OutputIvars]
    pub(crate) struct OutputTab;

    unsafe impl NSObjectProtocol for OutputTab {}

    impl OutputTab {
        #[unsafe(method(ideAgentOutputOpen:))]
        fn open_action(&self, _sender: Option<&AnyObject>) {
            guarded((), || self.open_in_editor());
        }
    }
);

impl AgentTab {
    /// Shows the output of background task `task` in its tab, opening one if there is none; at its end either way.
    pub fn show_background_output(&self, task: &str) {
        let Some(controller) = self.controller() else { return };
        let key = format!("task-output:{}:{task}", self.key.get());
        let open = self.outputs.borrow().get(task).and_then(Weak::load);
        if let Some(open) = open
            && controller.select_tab(&key)
        {
            open.ivars().to_end.set(true);
            open.scroll_to_end();
            return;
        }
        let label = {
            let state = self.state.borrow();
            let item = state.thread().background().iter().find(|b| b.id == task).cloned();
            item.map_or_else(|| task.to_string(), |b| b.label)
        };
        let tab = OutputTab::new(self, task, &label);
        self.outputs.borrow_mut().insert(task.to_string(), Weak::from(&*tab));
        let text = tab.ui().text.clone();
        let _ = controller.add_tab(TabKind::Log, Some(key), &label, Retained::into_super(tab.clone()));
        if let Some(window) = text.window() {
            window.makeFirstResponder(Some(&text));
        }
        tab.tick();
    }
}

/// What a read takes along from the main thread.
struct Look {
    status: Option<term_thread::TaskStatus>,
    agent_file: Option<PathBuf>,
    streamed: String,
    logs: Vec<PathBuf>,
    session: Option<term_claude::Session>,
}

impl OutputTab {
    fn new(agent: &AgentTab, task: &str, title: &str) -> Retained<OutputTab> {
        let mtm = agent.mtm;
        let ivars = OutputIvars {
            agent: agent.me.borrow().clone(),
            task: task.to_string(),
            ui: OnceCell::new(),
            file: RefCell::new(None),
            text: RefCell::new(String::new()),
            reads: Cell::new(0),
            reading: Cell::new(None),
            to_end: Cell::new(true),
            reads_after_end: Cell::new(0),
        };
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
        // SAFETY: NSView's designated initializer, called once on the newly allocated object.
        let tab: Retained<OutputTab> =
            unsafe { msg_send![super(OutputTab::alloc(mtm).set_ivars(ivars)), initWithFrame: frame] };
        let _ = tab.ivars().ui.set(OutputUi::new(mtm, &tab, title));
        tab
    }

    fn ui(&self) -> &OutputUi {
        self.ivars().ui.get().expect("the tab builds its views in `new`")
    }

    /// Reads the output when the tab shows and the last read has answered, then waits for the next turn; it stops once
    /// the tab or its agent tab is gone, or the task ended and was read.
    fn tick(&self) {
        let Some(agent) = self.ivars().agent.upgrade() else {
            return;
        };
        if self.ivars().reads_after_end.get() > AFTER_END {
            return;
        }
        let waiting = self.ivars().reading.get().is_some_and(|since| since.elapsed() < STALE);
        if !waiting && self.is_shown() {
            self.read(&agent);
        }
        let me = MainThreadBound::new(Weak::from(self), MainThreadMarker::from(self));
        main_after(EVERY, move || {
            if let Some(tab) = MainThreadMarker::new().and_then(|mtm| me.get(mtm).load()) {
                guarded((), || tab.tick());
            }
        });
    }

    /// Whether the tab is the one in front of its window: a document tab behind others keeps its window.
    fn is_shown(&self) -> bool {
        self.window()
            .is_some_and(|window| crate::common::panel::in_front(&window))
            && !self.isHiddenOrHasHiddenAncestor()
    }

    fn look(&self, agent: &AgentTab) -> Look {
        let state = agent.state.borrow();
        let thread = state.thread();
        let item = thread.background().iter().find(|b| b.id == self.ivars().task);
        Look {
            status: item.map(|b| b.status),
            agent_file: item.and_then(|b| b.output.path.clone()).map(PathBuf::from),
            streamed: item.and_then(|b| b.output.text.clone()).unwrap_or_default(),
            logs: item
                .and_then(|b| task_output::command_line(thread, b))
                .map(|command| term_thread::shell::output_files(&command, agent.cwd()))
                .unwrap_or_default(),
            session: None,
        }
    }

    fn read(&self, agent: &AgentTab) {
        let mut look = self.look(agent);
        // The CLI lists a task it started only with its next list of background work.
        let ended = look.status.map_or_else(|| agent.session().is_none(), |s| s.is_final());
        if ended {
            self.ivars().reads_after_end.set(self.ivars().reads_after_end.get() + 1);
        }
        let label = look.status.map_or("", status_text);
        self.ui().state.setStringValue(&ns(label));
        let read = self.ivars().reads.get() + 1;
        self.ivars().reads.set(read);
        self.ivars().reading.set(Some(Instant::now()));
        let me = MainThreadBound::new(Weak::from(self), MainThreadMarker::from(self));
        let file = self.ivars().file.borrow().clone();
        let has_file = look.agent_file.as_deref().is_some_and(Path::is_file);
        look.session = if has_file { None } else { agent.session() };
        let notice = notice(agent, &self.ivars().task, ended);
        let found = move |text: String, look: Look| {
            let _ = term_base::lane::DISK.try_run(move || {
                let at = file.as_ref().map(|(path, read)| (path.as_path(), *read));
                let found = task_output::find(look.agent_file.as_deref(), text, &look.logs, at);
                main_soon(move || {
                    if let Some(tab) = MainThreadMarker::new().and_then(|mtm| me.get(mtm).load()) {
                        guarded((), || tab.take(read, found, &notice));
                    }
                });
            });
        };
        match look.session.take() {
            Some(session) => session.task_output(&self.ivars().task).then(move |output| {
                let text = output.map(|o| o.output).unwrap_or_else(|_| look.streamed.clone());
                found(text, look);
            }),
            None => found(look.streamed.clone(), look),
        }
    }

    /// Shows what read `read` found, unless a later read started meanwhile.
    fn take(&self, read: u64, found: Found, notice: &str) {
        if read != self.ivars().reads.get() {
            return;
        }
        self.ivars().reading.set(None);
        let change = match found {
            Found::File { path, read, change } => {
                let same = self
                    .ivars()
                    .file
                    .borrow()
                    .as_ref()
                    .is_some_and(|(shown, _)| *shown == path);
                *self.ivars().file.borrow_mut() = Some((path, read));
                self.ivars().text.borrow_mut().clear();
                match change {
                    Change::Append(added) if !same => Change::Replace(added),
                    change => change,
                }
            }
            Found::Text(text) => {
                let from_file = self.ivars().file.borrow_mut().take().is_some();
                let change = if from_file {
                    Change::Replace(text.clone())
                } else {
                    task_output::change(&self.ivars().text.borrow(), &text)
                };
                *self.ivars().text.borrow_mut() = text;
                change
            }
            Found::Nothing => {
                *self.ivars().file.borrow_mut() = None;
                self.ivars().text.borrow_mut().clear();
                Change::Replace(String::new())
            }
        };
        self.ui().open.setHidden(self.ivars().file.borrow().is_none());
        self.show(change, notice);
    }

    fn show(&self, change: Change, notice: &str) {
        let ui = self.ui();
        let at_end = self.ivars().to_end.replace(false) || is_at_end(&ui.scroll);
        // SAFETY: The text view keeps its text storage for as long as it lives.
        let Some(storage) = (unsafe { ui.text.textStorage() }) else {
            return;
        };
        match change {
            Change::Same => {}
            Change::Append(added) => {
                with_editing(&storage, || storage.mutableString().appendString(&ns(&added)));
                if storage.length() as u64 > 2 * MAX_SHOWN {
                    self.ivars().file.borrow_mut().take();
                }
            }
            Change::Replace(text) => {
                let empty = text.is_empty();
                let shown = if empty { notice } else { &text };
                let differs = storage.length() != shown.encode_utf16().count() || storage.string().to_string() != shown;
                if differs {
                    let origin = ui.scroll.contentView().bounds().origin;
                    ui.text.setString(&ns(shown));
                    if !at_end {
                        ui.scroll.contentView().scrollToPoint(origin);
                        ui.scroll.reflectScrolledClipView(&ui.scroll.contentView());
                    }
                }
                let color = if empty {
                    NSColor::secondaryLabelColor()
                } else {
                    NSColor::labelColor()
                };
                ui.text.setTextColor(Some(&color));
            }
        }
        if at_end {
            self.scroll_to_end();
        }
    }

    fn scroll_to_end(&self) {
        // SAFETY: A nil sender is what AppKit's own key bindings pass.
        unsafe { self.ui().text.scrollToEndOfDocument(None) };
    }

    /// Opens the file shown in an editor, which takes it again when it is open already: a file outside the project's
    /// folders changes unwatched.
    fn open_in_editor(&self) {
        let path = self.ivars().file.borrow().as_ref().map(|(path, _)| path.clone());
        let controller = self.ivars().agent.upgrade().and_then(|agent| agent.controller());
        if let (Some(path), Some(controller)) = (path, controller) {
            controller.open_file(&path);
        }
    }
}

/// What the tab says while the task has no output.
fn notice(agent: &AgentTab, task: &str, ended: bool) -> String {
    let state = agent.state.borrow();
    let label = state
        .thread()
        .background()
        .iter()
        .find(|b| b.id == task)
        .map_or_else(|| task.to_string(), |b| b.label.clone());
    if ended {
        tf("„{}“ hat nichts ausgegeben.", &[&label])
    } else {
        tf("„{}“ hat noch nichts ausgegeben.", &[&label])
    }
}

/// Whether `scroll` shows the end of its document, so new output keeps it there.
fn is_at_end(scroll: &NSScrollView) -> bool {
    let Some(document) = scroll.documentView() else {
        return true;
    };
    let visible = scroll.contentView().bounds();
    visible.origin.y + visible.size.height >= document.frame().size.height - 4.0
}

impl OutputUi {
    fn new(mtm: MainThreadMarker, tab: &OutputTab, title: &str) -> OutputUi {
        let name = label(mtm, title);
        name.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
        name.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        let state = label(mtm, "");
        state.setTextColor(Some(&NSColor::secondaryLabelColor()));
        // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
        let open = unsafe { NSButton::buttonWithTitle_target_action(&ns(t("Im Editor öffnen")), None, None, mtm) };
        set_action(&open, tab, sel!(ideAgentOutputOpen:));
        open.setBezelStyle(NSBezelStyle::Push);
        open.setControlSize(NSControlSize::Small);
        open.setFont(Some(&crate::common::style::small_system()));
        open.setHidden(true);
        let header = NSStackView::new(mtm);
        header.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
        header.setSpacing(crate::common::style::space::M);
        header.setEdgeInsets(NSEdgeInsets {
            top: 8.0,
            left: 12.0,
            bottom: 8.0,
            right: 12.0,
        });
        header.addView_inGravity(&name, NSStackViewGravity::Leading);
        header.addView_inGravity(&state, NSStackViewGravity::Leading);
        header.addView_inGravity(&open, NSStackViewGravity::Trailing);
        let (scroll, text) = text_view(mtm, title);
        let line = NSBox::new(mtm);
        line.setBoxType(NSBoxType::Separator);
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(crate::common::style::space::NONE);
        for view in [&*header as &NSView, &line, &scroll] {
            stack.addArrangedSubview(view);
            pin_sides(view, &stack, 0.0);
        }
        scroll.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);
        tab.addSubview(&stack);
        pin_edges(&stack, tab);
        OutputUi {
            text,
            scroll,
            state,
            open,
        }
    }
}

/// A read-only, monospaced text view with the find bar, left to right whatever the interface's direction.
fn text_view(mtm: MainThreadMarker, title: &str) -> (Retained<NSScrollView>, Retained<NSTextView>) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    let text = NSTextView::initWithFrame(NSTextView::alloc(mtm), frame);
    text.setEditable(false);
    text.setSelectable(true);
    text.setRichText(false);
    text.setUsesFindBar(true);
    text.setIncrementalSearchingEnabled(true);
    let size = crate::text_size::current();
    text.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        (size - 1.0).max(9.0),
        constant!(objc2_app_kit::NSFontWeightRegular),
    )));
    text.setTextContainerInset(NSSize::new(8.0, 8.0));
    text.setHorizontallyResizable(true);
    text.setMaxSize(NSSize::new(f64::MAX, f64::MAX));
    text.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::ViewWidthSizable);
    // SAFETY: A text view keeps its text container alive as long as it lives.
    if let Some(container) = unsafe { text.textContainer() } {
        container.setWidthTracksTextView(false);
        container.setContainerSize(NSSize::new(f64::MAX, f64::MAX));
    }
    text.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    text.setBaseWritingDirection(NSWritingDirection::LeftToRight);
    text.setAccessibilityLabel(Some(&ns(title)));
    scroll.setDocumentView(Some(&text));
    (scroll, text)
}
