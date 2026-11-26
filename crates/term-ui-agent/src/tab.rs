//! One agent tab: its session, the views showing it, and how session events reach them.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use objc2::rc::{Retained, Weak};
use objc2::runtime::ProtocolObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSColor, NSEvent, NSLayoutConstraint, NSResponder, NSStackView, NSView};
use objc2_foundation::{NSNotificationCenter, NSObjectProtocol, NSRect};
use splat_i18n::t;
use term_claude::Session;
use term_thread::{Change, Status, Thread, ThreadId};
use term_ui::workspace_host::AgentStatus;

use super::AgentKey;
use super::acp::Agent;
use super::composer::Composer;
use super::header::Header;
use super::model::State;
use super::source::{Problem, Resume, Source};
use super::status::StatusLine;
use super::suggest::Suggestions;
use super::target::AgentTarget;
use super::timeline::Timeline;
use super::{overlay, ui};
use crate::common::util::{activate, auto_layout, guarded, main_soon};
use term_ui::workspace_host::{Notices, TabDecor, TabId, TabKind, Tabs, Workspace};
#[cfg(feature = "selftest")]
mod selftest_probe;

/// The room on either side of the conversation, which takes the rest of the tab's width.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub const SIDE_MARGIN: f64 = 16.0;
/// The widest the composer gets; it stands in the middle below a wider conversation.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub const INPUT_WIDTH: f64 = 852.0;
/// The room between the composer and the conversation's edges.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub const INPUT_INSET: f64 = 14.0;
/// How strongly the composer fills the conversation's width up to `INPUT_WIDTH`: above the content's hugging and the
/// holding priority of the window's content, below the sidebar's, so a wide window neither narrows nor moves it.
const FILL_PRIORITY: f32 = 255.0;

pub(crate) struct RootIvars {
    key: AgentKey,
}

define_class!(
    // SAFETY: NSView has no subclassing requirements and `AgentRoot` does not implement Drop.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermAgentAgentRoot"]
    #[ivars = RootIvars]
    pub(crate) struct AgentRoot;

    unsafe impl NSObjectProtocol for AgentRoot {}

    impl AgentRoot {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            guarded((), || {
                NSColor::textBackgroundColor().setFill();
                objc2_app_kit::NSBezierPath::fillRect(self.bounds());
            });
        }

        /// A click on the composer's card that no control in it took, such as on its margin, goes into its text.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let taken = guarded(false, || {
                let Some(tab) = super::get(self.ivars().key) else {
                    return false;
                };
                let card = &tab.composer.card;
                let inside = card.convertPoint_fromView(event.locationInWindow(), None);
                if !objc2_foundation::NSPointInRect(inside, card.bounds()) {
                    return false;
                }
                tab.focus_composer();
                true
            });
            if !taken {
                // SAFETY: The overridden method with arguments of its types, which objc2 checks in debug builds.
                let () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }

        /// A tab that leaves its window for good was closed: its session ends with it.
        #[unsafe(method(viewDidMoveToWindow))]
        fn did_move_to_window(&self) {
            guarded((), || {
                if self.window().is_some() {
                    return;
                }
                let key = self.ivars().key;
                main_soon(move || {
                    if super::get(key).is_some_and(|tab| tab.root.window().is_none()) {
                        super::remove(key);
                    }
                });
            });
        }
    }
);

/// An agent tab: one session with an agent, its timeline and its composer.
pub struct AgentTab {
    /// The tab's number, which the app's registry of agent tabs keeps it under.
    pub key: AgentKey,
    /// The agent the tab runs, for good.
    pub(super) agent: Agent,
    pub(super) mtm: MainThreadMarker,
    pub(super) controller: Workspace,
    pub(super) tab_id: std::cell::Cell<Option<TabId>>,
    pub(super) cwd: PathBuf,
    pub(super) thread_id: ThreadId,
    pub(super) target: Retained<AgentTarget>,
    pub(super) root: Retained<AgentRoot>,
    pub(super) header: Header,
    pub(super) timeline: Timeline,
    pub(super) composer: Composer,
    pub(super) status: StatusLine,
    pub(super) problem_holder: Retained<NSView>,
    /// The note above the composer while the workspace is not trusted.
    pub(super) trust_note: Retained<NSStackView>,
    /// A change of trust waits for the running turn to end before the session starts again.
    pub(super) restart_for_trust: std::cell::Cell<bool>,
    pub(super) queue_view: Retained<NSStackView>,
    pub(super) suggestions: Suggestions,
    pub(super) state: RefCell<State>,
    pub(super) ticking: std::cell::Cell<bool>,
    pub(super) parking: super::tab_parking::Parking,
    /// A turn ended while the user looked elsewhere, until the tab comes to the front.
    pub(super) attention: std::cell::Cell<bool>,
    /// Counts the times the session's work stopped or started, which tells a pending "finished" notification whether
    /// the session stayed quiet since.
    pub(super) quiet_epoch: std::cell::Cell<u64>,
    /// Where the keyboard focus was before ⌘Esc moved it into the composer.
    pub(super) previous_focus: RefCell<Option<Weak<NSResponder>>>,
    /// The overlay open over the tab.
    pub(super) dialog: RefCell<Option<Rc<super::dialogs::Open>>>,
    /// How many overlays the tab opened, which tells an answer for the open one from one for an earlier.
    pub(super) dialogs_opened: std::cell::Cell<u64>,
    /// The questions asked beside the conversation with `/btw` and their answers, which the next one builds on.
    pub(super) asides: RefCell<Vec<super::dialogs::model::Aside>>,
    /// The tab itself, for what holds on to it without keeping it open, such as its overlay.
    pub(super) me: RefCell<std::rc::Weak<AgentTab>>,
    /// The tabs that show the output of a background task, by task id.
    pub(super) outputs: RefCell<std::collections::HashMap<String, Weak<super::output_tab::OutputTab>>>,
    /// The registry the tab runs its session in.
    pub(super) sessions: Option<Rc<dyn crate::Sessions>>,
    /// The action in progress while Codex Remote Control starts or moves its thread.
    pub(super) codex_remote: RefCell<Option<super::codex::remote::Turning>>,
    /// Lays the tab out anew when the user changes the text size.
    text_size: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>>,
}

impl Drop for AgentTab {
    fn drop(&mut self) {
        if let Some(observer) = self.text_size.take() {
            // SAFETY: An observer this center handed out.
            unsafe { NSNotificationCenter::defaultCenter().removeObserver(observer.as_ref()) };
        }
    }
}

impl AgentTab {
    /// A new tab in the workspace's base path running `agent`, not started yet; `state` holds the agent tabs.
    pub(crate) fn open(
        controller: &Workspace,
        state: &crate::State,
        agent: Agent,
        resume: Option<Resume>,
    ) -> Option<Rc<AgentTab>> {
        let cwd = controller.project_root().or_else(|| controller.project())?;
        AgentTab::open_in(controller, state, agent, cwd, resume)
    }

    /// A new tab of the workspace running `agent` in `cwd`, not started yet.
    pub(crate) fn open_in(
        controller: &Workspace,
        state: &crate::State,
        agent: Agent,
        cwd: PathBuf,
        resume: Option<Resume>,
    ) -> Option<Rc<AgentTab>> {
        let thread_id = resume.as_ref().and_then(|r| r.thread.clone()).unwrap_or_default();
        let thread = Thread::new(thread_id, agent.id(), &cwd);
        let tab = AgentTab::shared(AgentTab::build(controller, state, agent, cwd, thread, Source::Idle));
        let bookmarked = crate::agent_panel::is_bookmarked(&tab.thread_id);
        tab.state.borrow_mut().opened(resume, bookmarked);
        crate::agent_panel::read_past(&tab.cwd);
        tab.add_to_window(controller);
        Some(tab)
    }

    /// A tab in `controller`'s window on `thread` of `agent` in `cwd`, whose session `source` runs; `agents` holds the
    /// agent tabs and the app's session registry.
    pub(super) fn build(
        controller: &Workspace,
        agents: &crate::State,
        agent: Agent,
        cwd: PathBuf,
        thread: Thread,
        source: Source,
    ) -> AgentTab {
        let mtm = controller.mtm();
        let thread_id = thread.id().clone();
        let thread = Arc::new(thread);
        let (key, sessions) = agents.admit();

        let target = AgentTarget::new(mtm, key);
        let this = AgentRoot::alloc(mtm).set_ivars(RootIvars { key });
        // SAFETY: NSView's initializer, called once on the newly allocated object.
        let root: Retained<AgentRoot> = unsafe { msg_send![super(this), init] };
        let header = Header::new(mtm, &target);
        let timeline = Timeline::new(mtm, key, &target, thread.clone(), cwd.clone());
        let composer = Composer::new(mtm, key, &target);
        let status = StatusLine::new(mtm, key);
        #[cfg(feature = "dictation")]
        term_ui_dictation::register(
            Rc::new(ComposerField(key)),
            &composer.text,
            &composer.mic,
            &status.dictation,
        );
        let problem_holder = NSView::new(mtm);
        let trust_note = super::tab_trust::note(mtm, &target);
        let queue_view = ui::column(mtm, 4.0, &[]);
        let suggestions = Suggestions::new(mtm, &target);
        // ACP agents take the user's last picks once their session runs.
        let choice = if matches!(agent, Agent::Claude) {
            super::choices::of(agent.id())
        } else {
            crate::AgentChoice::default()
        };
        let state = RefCell::new(State::new(&agent, source, thread, choice));
        let tab = AgentTab {
            key,
            mtm,
            controller: *controller,
            tab_id: std::cell::Cell::new(None),
            cwd,
            thread_id,
            target,
            root,
            header,
            timeline,
            composer,
            status,
            problem_holder,
            trust_note,
            restart_for_trust: std::cell::Cell::new(false),
            queue_view,
            suggestions,
            agent,
            state,
            ticking: std::cell::Cell::new(false),
            parking: super::tab_parking::Parking::default(),
            attention: std::cell::Cell::new(false),
            quiet_epoch: std::cell::Cell::new(0),
            previous_focus: RefCell::new(None),
            dialog: RefCell::new(None),
            dialogs_opened: std::cell::Cell::new(0),
            asides: RefCell::new(Vec::new()),
            me: RefCell::new(std::rc::Weak::new()),
            outputs: RefCell::default(),
            sessions,
            codex_remote: RefCell::new(None),
            text_size: RefCell::new(None),
        };
        tab.lay_out();
        tab
    }

    /// `tab` shared, knowing itself.
    pub(super) fn shared(tab: AgentTab) -> Rc<AgentTab> {
        let tab = Rc::new(tab);
        *tab.me.borrow_mut() = Rc::downgrade(&tab);
        *tab.text_size.borrow_mut() = Some(crate::text_size::follow(&tab));
        crate::timeline::Timeline::watch(&tab);
        tab
    }

    fn lay_out(&self) {
        let mtm = self.mtm;
        let root: &NSView = &self.root;
        let line = ui::separator(mtm);
        let column = NSView::new(mtm);
        column.setTranslatesAutoresizingMaskIntoConstraints(false);
        let bottom = ui::column(
            mtm,
            6.0,
            &[
                &self.queue_view,
                &self.suggestions.view,
                &self.trust_note,
                &self.composer.card,
                &self.status.view,
            ],
        );
        self.status.view.setEdgeInsets(objc2_foundation::NSEdgeInsets {
            top: 0.0,
            left: 4.0,
            bottom: 0.0,
            right: 4.0,
        });
        for view in [&*self.header.view as &NSView, &line, &column] {
            root.addSubview(view);
        }
        for view in [self.timeline.view() as &NSView, &self.problem_holder, &bottom] {
            column.addSubview(view);
        }
        auto_layout(&[&self.problem_holder]);
        let scroll = self.timeline.view();
        let constraints: Vec<Retained<NSLayoutConstraint>> = vec![
            self.header.view.topAnchor().constraintEqualToAnchor(&root.topAnchor()),
            self.header
                .view
                .leadingAnchor()
                .constraintEqualToAnchor(&root.leadingAnchor()),
            self.header
                .view
                .trailingAnchor()
                .constraintEqualToAnchor(&root.trailingAnchor()),
            line.topAnchor()
                .constraintEqualToAnchor(&self.header.view.bottomAnchor()),
            line.leadingAnchor().constraintEqualToAnchor(&root.leadingAnchor()),
            line.trailingAnchor().constraintEqualToAnchor(&root.trailingAnchor()),
            column.topAnchor().constraintEqualToAnchor(&line.bottomAnchor()),
            column.bottomAnchor().constraintEqualToAnchor(&root.bottomAnchor()),
            column
                .leadingAnchor()
                .constraintEqualToAnchor_constant(&root.leadingAnchor(), SIDE_MARGIN),
            column
                .trailingAnchor()
                .constraintEqualToAnchor_constant(&root.trailingAnchor(), -SIDE_MARGIN),
            scroll.topAnchor().constraintEqualToAnchor(&column.topAnchor()),
            scroll.leadingAnchor().constraintEqualToAnchor(&column.leadingAnchor()),
            scroll
                .trailingAnchor()
                .constraintEqualToAnchor(&column.trailingAnchor()),
            scroll
                .bottomAnchor()
                .constraintEqualToAnchor_constant(&bottom.topAnchor(), -6.0),
            self.problem_holder
                .centerXAnchor()
                .constraintEqualToAnchor(&scroll.centerXAnchor()),
            self.problem_holder
                .centerYAnchor()
                .constraintEqualToAnchor(&scroll.centerYAnchor()),
            self.problem_holder
                .widthAnchor()
                .constraintLessThanOrEqualToAnchor(&scroll.widthAnchor()),
        ];
        activate(constraints.into_iter().chain(input_constraints(&bottom, &column)));
    }

    /// Adds the tab to the window's tab bar.
    pub(super) fn add_to_window(&self, controller: &Workspace) {
        let view: Retained<NSView> = self.root();
        let id = controller.add_tab(TabKind::Agent, Some(self.tab_key()), self.agent.name(), view);
        self.tab_id.set(id);
        if let Some(id) = id {
            controller.set_tab_tint(id, &self.agent.colour());
            if let Some(colors) = self.agent.four_colours() {
                controller.set_tab_four_colours(id, colors);
            }
        }
        self.composer.set_agent(self.agent.name());
        self.refresh();
    }

    /// The key the window's document tabs know the tab by.
    pub fn tab_key(&self) -> String {
        format!("agent:{}", self.key.get())
    }

    /// The view the window shows as the tab.
    pub fn root(&self) -> Retained<NSView> {
        Retained::into_super(self.root.clone())
    }

    /// The conversation's entries as the tab shows them.
    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    pub(crate) fn controller(&self) -> Option<Workspace> {
        self.controller.load()
    }

    pub(crate) fn belongs_to(&self, controller: &Workspace) -> bool {
        self.controller.is_open() && self.controller == *controller
    }

    /// The agent's id for the conversation, once it has one, which the restored state keeps (`KeptSession`).
    pub fn session_id(&self) -> Option<String> {
        let state = self.state.borrow();
        state
            .thread()
            .session()
            .map(str::to_string)
            .or_else(|| state.resume().map(|r| r.session.clone()))
    }

    /// The thread the session belongs to in the agents panel.
    pub fn thread_id(&self) -> &ThreadId {
        &self.thread_id
    }

    /// The folder the session works in: the workspace's base path.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Names the thread `title`, as the user would rename it.
    #[cfg(feature = "selftest")]
    pub(crate) fn set_custom_title(&self, title: &str) {
        self.state.borrow_mut().set_custom_title(title.to_string());
        self.refresh();
    }

    /// The title the tab and the session list show.
    pub fn title(&self) -> String {
        self.state.borrow().title(&untitled(&self.agent))
    }

    /// The thread as it is now.
    pub fn snapshot(&self) -> Thread {
        self.state.borrow().thread().as_ref().clone()
    }

    /// The last state received by the UI, shared without copying its history.
    pub fn view_snapshot(&self) -> Arc<Thread> {
        self.state.borrow().thread().clone()
    }

    /// The running Claude session, if there is one.
    pub(crate) fn session(&self) -> Option<Session> {
        self.state.borrow().source().session().filter(|s| !s.is_ended())
    }

    /// What the session is doing, as the tab's symbol and the agents panel show it.
    pub fn status(&self) -> Status {
        self.state.borrow().status()
    }

    /// Whether the tab is the one showing in its window.
    pub(crate) fn is_shown(&self) -> bool {
        self.root.window().is_some_and(|w| crate::common::panel::in_front(&w))
            && !self.root.isHiddenOrHasHiddenAncestor()
    }

    /// Brings the tab and its window to the front.
    pub fn select(&self) {
        if let Some(controller) = self.controller() {
            controller.select_tab(&self.tab_key());
            if let Some(window) = controller.window() {
                window.makeKeyAndOrderFront(None);
            }
        }
    }

    /// Resumes presentation and live labels after a background tab comes back into view.
    pub(crate) fn show_pending(&self) {
        self.parking.hidden_since.take();
        if self.attention.replace(false) {
            self.refresh_tab_mark();
        }
        self.timeline.show_pending();
        self.start_ticking();
        self.wake_deferred();
    }

    /// Shows the head, the timeline, the composer and the status line at the text size the user chose.
    pub(crate) fn text_size_changed(&self) {
        self.header.text_size_changed();
        self.timeline.text_size_changed();
        self.composer.text_size_changed();
        self.status.text_size_changed();
        self.refresh_chips();
        self.refresh_queue();
    }

    /// Shows the thread as `snapshot` has it and reacts to how its status moved.
    pub(super) fn absorb(&self, snapshot: impl Into<Arc<Thread>>, changes: &[Change]) {
        let _interval = term_base::signpost::begin(term_base::signpost::name!("agent update"));
        let snapshot: Arc<Thread> = snapshot.into();
        let refresh = super::refresh::Refresh::of(&snapshot, changes);
        let before = self.state.borrow().status();
        let now = snapshot.status();
        if now != Status::Idle || snapshot.background().iter().any(|task| !task.status.is_final()) {
            self.parking.hidden_since.take();
        }
        let delegated_before = delegates(self.state.borrow().thread());
        let delegated_now = delegates(&snapshot);
        let title_changed = snapshot.title() != self.state.borrow().thread().title();
        self.state.borrow_mut().absorbed(snapshot.clone());
        let had_rows = self.timeline.count() > 0;
        self.timeline.inputs().borrow_mut().forget_answered(&snapshot, changes);
        self.timeline.apply(snapshot, changes);
        if refresh.controls
            || changes.is_empty()
            || title_changed
            || before != now
            || had_rows != (self.timeline.count() > 0)
        {
            self.refresh();
        } else if refresh.status {
            self.status.show(self.state.borrow().thread());
        }
        if before != now {
            self.status_moved(before, now);
        }
        if delegated_before != delegated_now {
            self.refresh_tab_mark();
        }
        self.work_moved(
            before != Status::Idle || delegated_before,
            now != Status::Idle || delegated_now,
        );
        if title_changed || before != now {
            self.persist();
        }
        if now != Status::Idle {
            self.start_ticking();
        }
        if refresh.overview {
            self.refresh_work();
            if let Some(controller) = self.controller() {
                if refresh.sessions {
                    super::changed(&controller);
                } else {
                    controller.refresh_activity();
                }
            }
        }
    }

    /// Records the session in the thread index, which the past sessions read.
    pub(super) fn persist(&self) {
        let state = self.state.borrow();
        if state.thread().session().is_none() && state.thread().entries(&term_thread::Lane::Main).is_empty() {
            return;
        }
        crate::agent_panel::record(
            term_thread::IndexEntry::of(state.thread(), SystemTime::now()),
            state.thread().usage().subscription,
            state.custom_title().map(str::to_string),
        );
    }

    /// Takes in the bookmarks, read after the tab opened.
    pub(super) fn bookmarks_read(&self) {
        let on = crate::agent_panel::is_bookmarked(&self.thread_id);
        self.state.borrow_mut().set_bookmarked(on);
        self.header.set_bookmarked(on);
    }

    pub(crate) fn set_problem(&self, problem: Option<Problem>) {
        self.state.borrow_mut().set_problem(problem);
        self.refresh();
    }

    /// Shows that the session did not start, for `problem`.
    pub(super) fn start_failed(&self, problem: Problem) {
        let replaced = self.state.borrow_mut().failed(problem);
        replaced.end();
        self.refresh();
    }

    /// Brings the head, the composer, the status line and the overlays up to date with the state.
    pub(crate) fn refresh(&self) {
        let (problem, thread, title) = {
            let state = self.state.borrow();
            (
                state.problem().cloned(),
                state.thread().clone(),
                state.title(&untitled(&self.agent)),
            )
        };
        self.header.set_title(&title);
        self.header.set_bookmarked(self.state.borrow().bookmarked());
        self.refresh_remote();
        self.refresh_work();
        if let (Some(controller), Some(tab)) = (self.controller(), self.tab_id.get()) {
            controller.set_tab_title(tab, &super::format::first_line(&title, 32));
        }
        if matches!(self.agent, Agent::Claude)
            && let Some(session) = self.session()
        {
            super::usage::fetch(&session);
        }
        self.status.show(&thread);
        self.refresh_composer();
        for view in self.problem_holder.subviews().iter() {
            view.removeFromSuperview();
        }
        if let Some(problem) = &problem {
            let methods = self.state.borrow().acp_caps().cloned().unwrap_or_default().auth_methods;
            let view = overlay::problem(self.mtm, &self.target, problem, self.agent.name(), &methods);
            self.problem_holder.addSubview(&view);
            crate::common::util::pin_edges(&view, &self.problem_holder);
        }
        self.problem_holder.setHidden(problem.is_none());
        self.show_trust_note(problem.as_ref());
        let blocked = matches!(problem, Some(Problem::Untrusted(_) | Problem::NotFound));
        self.composer.card.setHidden(blocked);
        self.refresh_tab_mark();
    }

    pub(super) fn refresh_remote(&self) {
        use super::header::{Reach, Remote};

        if let Some(session) = self.codex_session() {
            self.header.set_remote(&self.codex_remote_shown(&session), Reach::Codex);
            return;
        }
        let remote = self.session().map(|s| s.remote_control()).unwrap_or_default();
        self.header.set_remote(&Remote::of_claude(&remote), Reach::Claude);
    }

    pub(super) fn refresh_tab_mark(&self) {
        let (Some(controller), Some(tab)) = (self.controller(), self.tab_id.get()) else {
            return;
        };
        if self.attention.get() && self.is_shown() && !crate::notify::user_looks_elsewhere(true) {
            self.attention.set(false);
        }
        controller.set_tab_status(tab, self.agent_status());
    }

    /// What the tab and the workspace's row in the sidebar show: the session waits for the user, works, could not
    /// start or ended with an error, finished a turn while the user looked elsewhere, or rests.
    pub fn agent_status(&self) -> AgentStatus {
        let state = self.state.borrow();
        match state.status() {
            Status::RequiresAction => AgentStatus::Waiting,
            Status::Running => AgentStatus::Running,
            Status::Idle if delegates(state.thread()) => AgentStatus::Running,
            Status::Idle if state.problem().is_some() => AgentStatus::Failed,
            Status::Idle if self.attention.get() => AgentStatus::Attention,
            Status::Idle => AgentStatus::Idle,
        }
    }

    /// Ends the session, as the tab closes.
    pub(crate) fn close(&self) {
        self.persist();
        let source = self.state.borrow_mut().end();
        source.end();
    }

    /// Stops the current turn, as `by` asked; background work goes on. The window's foot says what stopped it.
    pub fn stop_turn(&self, by: super::Stop) {
        if self.status() == Status::Idle {
            return;
        }
        crate::common::diag::report("stopping a turn", format!("{by:?}"));
        super::stop::record(by, &self.title());
        self.controller.show_notice(by.notice());
        self.state.borrow().source().interrupt();
    }

    /// Whether the keyboard focus is in the composer.
    pub fn composer_has_focus(&self) -> bool {
        self.composer.has_focus()
    }

    /// Puts `text` into the composer for the user to read and send, and moves the keyboard focus there.
    pub fn set_draft(&self, text: &str) {
        self.composer.set_text(text);
        self.focus_composer();
    }

    /// Adds text from outside to the next message, such as a CI log.
    pub fn attach_outside(&self, attachment: term_thread::Attachment) {
        self.state.borrow_mut().attach([attachment]);
        self.refresh_chips();
    }

    /// Moves the keyboard focus to the composer, remembering where it was.
    pub fn focus_composer(&self) {
        let Some(window) = self.root.window() else { return };
        let previous = window.firstResponder().map(|r| Weak::from(&*r));
        *self.previous_focus.borrow_mut() = previous;
        window.makeFirstResponder(Some(&self.composer.text));
    }

    pub(crate) fn leave_focus(&self) {
        let Some(window) = self.root.window() else { return };
        let previous = self.previous_focus.borrow_mut().take().and_then(|w| w.load());
        window.makeFirstResponder(previous.as_deref());
    }

    pub(crate) fn composer_focus(&self, focused: bool) {
        self.composer.show_focus(focused);
        if focused {
            self.offer_selection();
        }
    }
}

/// Whether work of `thread` runs on beside its turn: a subagent, a workflow or a background command, but not the
/// agent's own housekeeping.
pub(crate) fn delegates(thread: &Thread) -> bool {
    term_agents::turn::running_work(thread) > 0
}

/// The composer and what stands above and below it at the foot of `column`: as wide as the column but for its inset,
/// at most `INPUT_WIDTH`, in the middle.
fn input_constraints(bottom: &NSView, column: &NSView) -> Vec<Retained<NSLayoutConstraint>> {
    let fill = bottom
        .widthAnchor()
        .constraintEqualToAnchor_constant(&column.widthAnchor(), -2.0 * INPUT_INSET);
    fill.setPriority(FILL_PRIORITY);
    vec![
        bottom.centerXAnchor().constraintEqualToAnchor(&column.centerXAnchor()),
        bottom
            .leadingAnchor()
            .constraintGreaterThanOrEqualToAnchor_constant(&column.leadingAnchor(), INPUT_INSET),
        bottom.widthAnchor().constraintLessThanOrEqualToConstant(INPUT_WIDTH),
        fill,
        bottom
            .bottomAnchor()
            .constraintEqualToAnchor_constant(&column.bottomAnchor(), -8.0),
    ]
}

/// The composer of the agent tab with this key, as a field that dictation goes into.
#[cfg(feature = "dictation")]
struct ComposerField(AgentKey);

#[cfg(feature = "dictation")]
impl term_ui_dictation::DictationField for ComposerField {
    fn notice(&self, text: &str) -> bool {
        let controller = super::get(self.0).and_then(|tab| tab.controller());
        controller.inspect(|controller| controller.show_notice(text)).is_some()
    }
}

/// What a session is called before anything names it.
fn untitled(agent: &Agent) -> String {
    match agent {
        Agent::Claude => t("Neue Sitzung").to_string(),
        other => other.name().to_string(),
    }
}
