//! The quick terminal's window (F45): a panel without a frame that slides in from an edge of the active screen above
//! other apps' windows, also over a full-screen space, without making the app active, and slides away again; its
//! sessions run on while it is hidden.

use std::rc::Rc;
use term_ui::workspace_host::{TabId, WorkspaceId};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication, NSBackingStoreType, NSEvent,
    NSFloatingWindowLevel, NSPanel, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{NSRect, NSSize};

use super::delegate::Delegate;
use super::{Quick, Window, session};
use crate::quick::{Edge, frames};
use crate::state::{app, try_app};
use crate::tab::TabContent;

/// How long sliding in or out takes.
const SLIDE: f64 = 0.18;

define_class!(
    // SAFETY: NSPanel has no subclassing requirements and `QuickPanel` does not implement Drop.
    #[unsafe(super(NSPanel, NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "TermQuickPanel"]
    struct QuickPanel;

    impl QuickPanel {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            true
        }
    }
);

impl Window {
    /// Opens the quick terminal with one workspace of one tab, hidden until `slide_in`.
    pub(crate) fn open_quick() -> Rc<Window> {
        let config = app().config();
        let window = Window::empty();
        let folder = session::start_folder(&config, None, config.window.inherit_directory.window);
        let workspace = window.add_workspace(folder.clone(), None);
        let tab = window.make_tab(folder, None);
        workspace.tabs.borrow_mut().push(tab.clone());
        workspace.select_tab(TabId::new(tab.id));
        window.workspaces.borrow_mut().select(WorkspaceId::new(workspace.id));
        window.enter_quick_mode();
        if let Some(panel) = window.front_ns_window() {
            panel.setFrame_display(place(window.mtm).0, false);
        }
        window.start_sessions(&tab);
        window
    }

    /// Whether this is the quick terminal.
    pub(crate) fn is_quick(&self) -> bool {
        self.quick.borrow().is_some()
    }

    /// Whether this is the quick terminal and it shows with the keyboard, so the hotkey hides it.
    pub(crate) fn quick_in_front(&self) -> bool {
        self.is_quick()
            && self
                .front_ns_window()
                .is_some_and(|panel| panel.isVisible() && panel.isKeyWindow())
    }

    /// Whether this is a window the user sees: any but a hidden quick terminal.
    pub(crate) fn is_shown(&self) -> bool {
        !self.is_quick() || self.front_ns_window().is_some_and(|panel| panel.isVisible())
    }

    fn enter_quick_mode(&self) {
        let mtm = self.mtm;
        let style =
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel | NSWindowStyleMask::Resizable;
        let frame = NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), NSSize::new(800.0, 400.0));
        // SAFETY: A new panel of the main thread's AppKit; the app keeps it, so closing must not release it.
        let panel: Retained<QuickPanel> = unsafe {
            msg_send![
                QuickPanel::alloc(mtm),
                initWithContentRect: frame,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        let panel: Retained<NSWindow> = Retained::into_super(Retained::into_super(panel));
        // SAFETY: As above, the app keeps its own reference.
        unsafe { panel.setReleasedWhenClosed(false) };
        panel.setLevel(NSFloatingWindowLevel);
        panel.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        panel.setHidesOnDeactivate(false);
        panel.setHasShadow(true);
        let delegate = Delegate::new(mtm, self.id(), None);
        panel.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        let body = NSView::new(mtm);
        panel.setContentView(Some(&body));
        if let Some(tab) = self.selected_tab() {
            super::fill(&body, &tab.view());
        }
        *self.quick.borrow_mut() = Some(Quick {
            panel,
            _delegate: delegate,
        });
    }

    /// Slides the quick terminal in on the screen under the pointer and gives it the keyboard.
    pub(crate) fn slide_in(&self) {
        let Some(panel) = self.front_ns_window() else { return };
        crate::quick::next_slide();
        let (shown, hidden) = place(self.mtm);
        if !panel.isVisible() {
            panel.setFrame_display(if reduce_motion(self.mtm) { shown } else { hidden }, false);
        }
        panel.makeKeyAndOrderFront(None);
        animate(&panel, shown, self.mtm, None);
        self.focus_active();
    }

    /// Slides the quick terminal out and hides it; its sessions run on.
    pub(crate) fn slide_out(&self) {
        let Some(panel) = self.front_ns_window() else { return };
        if !panel.isVisible() {
            return;
        }
        let (_, hidden) = place_on(self.mtm, panel.screen());
        let id = self.id();
        let slide = crate::quick::next_slide();
        let done = RcBlock::new(move || {
            term_base::guarded((), || {
                if crate::quick::slide() == slide
                    && let Some(window) = try_app().and_then(|app| app.window(id))
                    && let Some(panel) = window.front_ns_window()
                {
                    panel.orderOut(None);
                }
            });
        });
        animate(&panel, hidden, self.mtm, Some(&done));
    }

    /// The settings changed: a quick terminal that shows takes its new place at once.
    pub(crate) fn quick_config_changed(&self) {
        if let Some(panel) = self.front_ns_window()
            && panel.isVisible()
        {
            panel.setFrame_display(place_on(self.mtm, panel.screen()).0, true);
        }
    }

    /// The window lost the keyboard: the quick terminal slides away where the settings say so, unless an alert or
    /// a window of its own took it.
    pub(crate) fn resigned_key(&self) {
        if !self.is_quick() || !app().config().quick_terminal.autohide {
            return;
        }
        let id = self.id();
        term_base::main_soon(move || {
            let Some(window) = try_app().and_then(|app| app.window(id)) else {
                return;
            };
            let Some(panel) = window.front_ns_window() else { return };
            let ns_app = NSApplication::sharedApplication(window.mtm);
            let own_child = ns_app
                .keyWindow()
                .and_then(|key| key.parentWindow())
                .is_some_and(|parent| std::ptr::eq(&*parent, &*panel));
            if panel.isKeyWindow() || own_child || ns_app.modalWindow().is_some() || panel.attachedSheet().is_some() {
                return;
            }
            window.slide_out();
        });
    }
}

/// The quick terminal's frames on the screen under the pointer: showing, and just past its edge.
fn place(mtm: MainThreadMarker) -> (NSRect, NSRect) {
    let pointer = NSEvent::mouseLocation();
    let screen = NSScreen::screens(mtm).iter().find(|screen| {
        let frame = screen.frame();
        pointer.x >= frame.origin.x
            && pointer.x <= frame.origin.x + frame.size.width
            && pointer.y >= frame.origin.y
            && pointer.y <= frame.origin.y + frame.size.height
    });
    place_on(mtm, screen)
}

fn place_on(mtm: MainThreadMarker, screen: Option<Retained<NSScreen>>) -> (NSRect, NSRect) {
    let screen = screen.or_else(|| NSScreen::mainScreen(mtm));
    let (visible, full) = screen.map_or(
        (
            NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 800.0)),
            NSRect::new(objc2_foundation::NSPoint::new(0.0, 0.0), NSSize::new(1280.0, 800.0)),
        ),
        |screen| (screen.visibleFrame(), screen.frame()),
    );
    let config = app().config();
    let quick = &config.quick_terminal;
    let (size, span) = quick.shares();
    let (shown, hidden) = frames(rect(visible), rect(full), Edge::from(quick.position), size, span);
    (ns_rect(shown), ns_rect(hidden))
}

fn rect(rect: NSRect) -> [f64; 4] {
    [rect.origin.x, rect.origin.y, rect.size.width, rect.size.height]
}

fn ns_rect([x, y, width, height]: [f64; 4]) -> NSRect {
    NSRect::new(objc2_foundation::NSPoint::new(x, y), NSSize::new(width, height))
}

fn reduce_motion(_mtm: MainThreadMarker) -> bool {
    NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion()
}

/// Moves `panel` to `frame`, sliding unless the user asked for reduced motion (F73), then runs `done`.
fn animate(panel: &NSWindow, frame: NSRect, mtm: MainThreadMarker, done: Option<&RcBlock<dyn Fn()>>) {
    if reduce_motion(mtm) {
        panel.setFrame_display(frame, true);
        if let Some(done) = done {
            done.call(());
        }
        return;
    }
    let target = panel.retain();
    let changes = RcBlock::new(move |context: std::ptr::NonNull<NSAnimationContext>| {
        // SAFETY: AppKit hands the block its live context for the duration of the call.
        unsafe { context.as_ref() }.setDuration(SLIDE);
        target.animator().setFrame_display(frame, true);
    });
    NSAnimationContext::runAnimationGroup_completionHandler(&changes, done.map(|done| &**done));
}
