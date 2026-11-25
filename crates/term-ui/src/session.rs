//! A terminal view's shell: starting it, what it reports, and what the view does about it: the title, the bell, the
//! banners, the end of the program, clearing and selecting.

use std::time::{Duration, Instant};

use objc2::DefinedClass;
use objc2_app_kit::{NSBeep, NSPasteboard, NSWorkspace};
use splat_i18n::{t, tf};
use term_session::{Clipboard, Event, Foreground, Options, Session};

use super::view::TerminalView;
use crate::Change;
use crate::kit::main_after;
use crate::model::{self, BELL_FLASH, Bell, Frame};

/// How long a notice shows above the terminal.
const NOTICE_TIME: Duration = Duration::from_secs(8);

impl TerminalView {
    /// Starts the shell with the login shell's environment `env`.
    pub fn spawn(&self, env: &[(String, String)]) {
        self.spawn_restored(env, Vec::new());
    }

    /// Starts the shell like `spawn`, the rows of an earlier session in its place shown above its first prompt (F71).
    pub fn spawn_restored(&self, env: &[(String, String)], restored: Vec<String>) {
        let config = super::host().config();
        let mut options = Options::new(self.ivars().cwd.borrow().clone());
        options.restored = restored;
        options.palette = self.ivars().theme.borrow().palette.clone();
        options.scrollback = config.terminal.scrollback;
        options.allow_clipboard_read = config.terminal.allow_clipboard_read;
        options.kitty_keyboard = config.terminal.kitty_keyboard;
        if !config.terminal.shell_integration {
            options.shell_integration = None;
        }
        options.ssh_integration = config.terminal.ssh_integration;
        if let Some(program) = self.ivars().program.borrow().clone() {
            options.program = program;
        }
        if let Some(size) = self.grid_size() {
            options.size = size;
            self.ivars().state.size.set(Some(size));
        }
        match Session::spawn(&super::host().spawner(), env, options, super::listener(self.id())) {
            Ok(session) => {
                session.focus(self.ivars().state.focused.get());
                *self.ivars().session.borrow_mut() = Some(session);
                self.layout_grid();
                if self.has_renderer() {
                    self.connect_renderer();
                } else {
                    self.refresh(true);
                }
            }
            Err(error) => {
                term_base::diag::report("starting a terminal", &error);
                self.notify(&tf(
                    "Das Terminal konnte nicht starten: {}",
                    &[&term_base::explain::Explain::explain(&error)],
                ));
            }
        }
    }

    /// Runs `f` with the session once it has started.
    pub fn with_session<R>(&self, f: impl FnOnce(&Session) -> R) -> Option<R> {
        self.ivars().session.borrow().as_ref().map(f)
    }

    /// Reacts to what the session reports.
    pub(super) fn handle(&self, event: Event) {
        match event {
            Event::Redraw => {
                self.redraw_requested();
                // A program turns echo off for a password, then writes its prompt, which brings this redraw.
                if self.ivars().state.focused.get() {
                    crate::secure::refresh();
                }
            }
            Event::Title(title) => self.set_title(title),
            Event::Bell => self.bell(),
            Event::ClipboardStore {
                clipboard: Clipboard::Clipboard,
                text,
            } => self.program_copied(&text),
            // A Mac has no primary selection, and copying into it on every selection would overwrite the pasteboard.
            Event::ClipboardStore { .. } => {}
            Event::ClipboardLoad(_) => {
                let text = super::clip::text(&NSPasteboard::generalPasteboard()).unwrap_or_default();
                self.with_session(|session| session.answer_clipboard(&text));
            }
            Event::WorkingDirectory(dir) => {
                if dir.is_local() {
                    *self.ivars().cwd.borrow_mut() = dir.path.clone();
                    super::host().changed(self.id(), Change::Directory(dir.path));
                }
            }
            Event::Prompt => {
                let prompts = &self.ivars().state.prompts;
                prompts.set(prompts.get() + 1);
                self.field_mark('A');
                self.scroll_marks_changed();
                super::host().changed(self.id(), Change::Prompt);
            }
            Event::InputStarted => {
                self.field_mark('B');
                super::host().changed(self.id(), Change::Input);
            }
            Event::CommandStarted { command } => {
                self.field_mark('C');
                super::host().changed(self.id(), Change::CommandStarted(command));
            }
            Event::CommandFinished { exit_code } => {
                self.scroll_marks_changed();
                super::host().changed(self.id(), Change::CommandFinished(exit_code));
            }
            // Dango opens these in its browser tab; a terminal has none, and a banner per `vite` would be noise.
            Event::DevServer(_) => {}
            Event::Progress(progress) => super::host().changed(self.id(), Change::Progress(progress)),
            Event::Notification(notification) => {
                super::host().changed(self.id(), Change::Notification(notification));
            }
            Event::Exited(ending) => {
                self.exited();
                super::host().changed(self.id(), Change::Exited(ending));
            }
        }
    }

    /// A program copied `text` with OSC 52: it replaces the pasteboard only while the user types in this terminal, and
    /// says so, as `cat` of a file could otherwise swap what the user copied for a command (B105).
    fn program_copied(&self, text: &str) {
        if !self.ivars().state.focused.get() {
            return;
        }
        super::clip::store(text);
        self.notify(t("Zwischenablage durch Programm ersetzt"));
    }

    fn set_title(&self, title: Option<String>) {
        *self.ivars().title.borrow_mut() = model::shown_title(title);
        self.show_title();
    }

    /// Tells the app the title when it changed, at most every `TITLE_GAP`.
    fn show_title(&self) {
        let state = &self.ivars().state;
        let title = self.ivars().title.borrow().clone();
        let shown = title.clone().unwrap_or_default();
        if state.tab_title.borrow().as_deref() == Some(shown.as_str()) {
            return;
        }
        match state.titles.ask(Instant::now(), model::TITLE_GAP) {
            Frame::Now => {
                super::host().changed(self.id(), Change::Title(title));
                *state.tab_title.borrow_mut() = Some(shown);
            }
            Frame::After(wait) => {
                let id = self.id();
                main_after(wait, move || {
                    if let Some(view) = super::find(id) {
                        view.ivars().state.titles.pending.set(false);
                        view.show_title();
                    }
                });
            }
            Frame::Pending | Frame::Stale => {}
        }
    }

    /// The title the program set last, if any.
    pub fn title(&self) -> Option<String> {
        self.ivars().title.borrow().clone()
    }

    /// The name VoiceOver reads for the view, with the shell's title once it set one.
    pub fn label(&self) -> String {
        model::label(self.ivars().title.borrow().as_deref())
    }

    fn bell(&self) {
        let visible = !self.isHiddenOrHasHiddenAncestor() && self.window().is_some_and(|w| w.isKeyWindow());
        let reduce_motion = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldReduceMotion();
        if !visible {
            super::host().changed(self.id(), Change::Bell);
        }
        match self.ivars().state.bell(Instant::now(), visible, reduce_motion) {
            Bell::Ignore => return,
            Bell::Beep => return NSBeep(),
            Bell::Flash(_) => {}
        }
        self.repaint();
        let id = self.id();
        main_after(BELL_FLASH, move || {
            if let Some(view) = super::find(id) {
                view.repaint();
            }
        });
    }

    /// Shows `text` above the terminal for a few seconds.
    pub fn notify(&self, text: &str) {
        let shown = self.ivars().state.next_notice();
        self.with_banners(|banners| banners.notice.show(text));
        let id = self.id();
        main_after(NOTICE_TIME, move || {
            if let Some(view) = super::find(id)
                && view.ivars().state.is_last_notice(shown)
            {
                view.with_banners(|banners| banners.notice.hide());
            }
        });
    }

    fn exited(&self) {
        self.ivars().state.exited.set(true);
        self.update_blink();
        self.layout_grid();
        self.repaint();
    }

    pub(super) fn clear(&self) {
        self.with_session(|session| {
            let shell = matches!(session.foreground(), Foreground::Shell(_));
            session.clear(shell && !session.input_modes().alternate_screen);
        });
        self.refresh(true);
    }

    pub(super) fn select_all(&self) {
        let Some(size) = self.ivars().state.size.get() else {
            return;
        };
        self.with_session(|session| {
            let corner = |line: u16, column: u16| term_session::Point {
                line: usize::from(line),
                column: usize::from(column),
            };
            session.scroll(term_session::Scroll::Top);
            session.select(
                term_session::SelectionKind::Simple,
                corner(0, 0),
                term_session::CellSide::Left,
            );
            session.scroll(term_session::Scroll::Bottom);
            let end = corner(size.lines.saturating_sub(1), size.columns.saturating_sub(1));
            session.extend_selection(end, term_session::CellSide::Right);
        });
        self.refresh(true);
    }

    pub(super) fn selected_text(&self) -> Option<String> {
        self.with_session(Session::selected_text)
            .flatten()
            .filter(|text| !text.is_empty())
    }
}
