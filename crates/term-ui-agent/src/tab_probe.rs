//! What the self-test reads from and does with an agent tab.

use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2_app_kit::{NSStackView, NSView};
use term_thread::{Thread, ThreadId};

use super::acp::Agent;
use super::source;
use super::tab::AgentTab;
use term_ui::workspace_host::Workspace;

/// A menu of the composer or the head, as a click on its button shows it.
pub enum TabMenu {
    /// The slash menu.
    Slash,
    /// The model pill's menu.
    Model,
    /// The mode pill's menu.
    Mode,
    /// The clock's menu of past sessions.
    History,
    /// The phone's menu while the session is connected to Remote Control.
    Remote,
}

/// What the self-test reads from a tab.
pub struct Inspection {
    pub rows: usize,
    pub built: usize,
    pub visible: Vec<String>,
    pub pills: Vec<String>,
    pub composer: bool,
    pub status: Vec<String>,
    pub title: String,
    pub subagents: Vec<String>,
    pub background: Vec<String>,
    /// The tooltip of the head's Remote Control button, while it shows.
    pub remote: Option<String>,
}

impl AgentTab {
    /// A tab that plays recordings, with no process behind it; `state` holds the agent tabs.
    pub(crate) fn open_replay(controller: &Workspace, state: &crate::State) -> Option<Rc<AgentTab>> {
        let cwd = controller.project_root().or_else(|| controller.project())?;
        Some(AgentTab::open_replay_in(controller, state, cwd))
    }

    /// A tab that plays recordings in `cwd`, for the self-test, in the app's session registry.
    pub(crate) fn open_replay_in(controller: &Workspace, state: &crate::State, cwd: PathBuf) -> Rc<AgentTab> {
        let thread = Thread::new(ThreadId::new(), term_claude::AGENT, &cwd);
        let tab = AgentTab::shared(AgentTab::build(
            controller,
            state,
            Agent::Claude,
            cwd,
            thread.clone(),
            source::Source::Idle,
        ));
        if let Some(source) = tab
            .sessions
            .as_ref()
            .and_then(|sessions| source::replay(sessions, thread))
        {
            let replaced = tab.state.borrow_mut().started(source);
            replaced.end();
        }
        tab.add_to_window(controller);
        tab
    }

    /// Plays recorded lines.
    pub fn feed(&self, lines: &str) {
        let (changes, snapshot) = self.state.borrow_mut().feed(lines);
        if let Some(snapshot) = snapshot {
            self.absorb(snapshot, &changes);
        }
    }

    /// The tooltip of the head's button for `part`, whether it can be clicked and whether it shows the accent colour.
    pub fn work_button(&self, part: crate::work::Part) -> (String, bool, bool) {
        let button = self.header.work_button(part);
        let tip = button.toolTip().map(|t| t.to_string()).unwrap_or_default();
        let accent = button
            .contentTintColor()
            .is_some_and(|c| c == objc2_app_kit::NSColor::controlAccentColor());
        (tip, button.isEnabled(), accent)
    }

    /// Shows `title` in the head for a moment and measures it as [`Header::title_widths`] does.
    pub fn title_widths(&self, title: &str) -> (f64, f64, f64) {
        self.header.set_title(title);
        let widths = self.header.title_widths();
        self.refresh();
        widths
    }

    /// What the self-test reads from the tab.
    pub fn inspect(&self) -> Inspection {
        let thread = self.snapshot();
        Inspection {
            rows: self.timeline.count(),
            built: self.timeline.built(),
            visible: self.timeline.visible_summaries(),
            pills: [&self.composer.model, &self.composer.mode]
                .iter()
                .map(|b| b.title().to_string())
                .collect(),
            composer: self.composer.card.window().is_some() && !self.composer.card.isHidden(),
            status: self.status.texts(),
            title: self.title(),
            subagents: thread.subagents().iter().map(|s| s.description.clone()).collect(),
            background: thread.background().iter().map(|b| b.label.clone()).collect(),
            remote: self.header.remote_tip(),
        }
    }

    /// Keeps the thread off Remote Control whatever the user's Claude settings say, so a live self-test registers
    /// nothing on claude.ai.
    pub fn keep_remote_control_off(&self) {
        self.state.borrow_mut().set_remote(false);
    }

    /// Picks the model for the next start.
    pub fn use_model(&self, model: &str) {
        self.state.borrow_mut().pick_model(model.to_string());
    }

    /// Picks `model`, `effort` and `mode` as the pills' menus do, which remembers them for the agent's next sessions.
    pub fn pick_all(&self, model: &str, effort: &str, mode: &str) {
        {
            let mut state = self.state.borrow_mut();
            state.offered_mut().models = vec![model.to_string()];
            state.offered_mut().efforts = vec![effort.to_string()];
            state.offered_mut().modes = vec![mode.to_string()];
        }
        self.pick_model(0);
        self.pick_effort(0);
        self.pick_mode(0);
    }

    /// Picks the mode `mode` as the mode pill's menu does, which remembers it for the agent's next sessions.
    pub fn pick_mode_id(&self, mode: &str) {
        self.state.borrow_mut().offered_mut().modes = vec![mode.to_string()];
        self.pick_mode(0);
    }

    /// The model, effort and mode the session runs with or starts with.
    pub fn picks(&self) -> (Option<String>, Option<String>, String) {
        let state = self.state.borrow();
        let picks = state.picks();
        (
            picks.model().map(str::to_string),
            picks.effort().map(str::to_string),
            picks.mode().to_string(),
        )
    }

    /// Switches to the permission mode `mode`, as the mode pill's menu does.
    pub fn use_mode(&self, mode: &str) {
        self.state.borrow().source().set_mode(mode, |_| ());
        self.state.borrow_mut().pick_mode(mode.to_string());
        self.refresh_composer();
    }

    /// The views that show at the foot of the tab besides the composer and its status line, with their heights.
    pub fn foot_extras(&self) -> Option<Vec<(String, f64)>> {
        // SAFETY: Asking for the superview only reads; the foot holds the composer while the tab lives.
        let foot = unsafe { self.composer.card.superview() }.and_then(|v| v.downcast::<NSStackView>().ok())?;
        foot.layoutSubtreeIfNeeded();
        let own: [&NSView; 2] = [&self.composer.card, &self.status.view];
        Some(
            foot.arrangedSubviews()
                .iter()
                .filter(|view| !view.isHidden() && !own.iter().any(|o| std::ptr::eq(*o, &**view)))
                .map(|view| {
                    (
                        view.class().name().to_string_lossy().into_owned(),
                        view.frame().size.height,
                    )
                })
                .collect(),
        )
    }

    /// Whether the mode pill and the sandbox pill take clicks.
    pub fn mode_and_sandbox_enabled(&self) -> (bool, bool) {
        (self.composer.mode.isEnabled(), self.composer.sandbox.isEnabled())
    }

    /// Types `text` into the composer and sends it, as Return would.
    pub fn type_and_send(&self, text: &str) {
        self.composer.set_text(text);
        self.submit();
    }

    /// Attaches the files at `paths` as the `+` button's panel would.
    pub fn attach_files(&self, paths: &[PathBuf]) {
        let picked: Vec<term_thread::Attachment> = paths
            .iter()
            .map(|path| super::outside::classify(super::actions::file_attachment(path), &self.cwd))
            .collect();
        self.state.borrow_mut().attach(picked);
        self.refresh_chips();
    }

    /// What the next message takes along.
    pub fn pending_attachments(&self) -> Vec<term_thread::Attachment> {
        self.state.borrow().attachments().to_vec()
    }

    /// The text of the next message's attachments from outside.
    pub fn outside_texts(&self) -> Vec<String> {
        self.state
            .borrow()
            .attachments()
            .iter()
            .filter_map(|a| match a {
                term_thread::Attachment::External { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// What the composer holds.
    pub fn composer_text(&self) -> String {
        self.composer.text()
    }

    /// Types `text` into the composer, the caret at its end, as a user would.
    pub fn type_in_composer(&self, text: &str) {
        self.composer.set_text(text);
        let end = text.encode_utf16().count();
        self.composer
            .text
            .setSelectedRange(objc2_foundation::NSRange::new(end, 0));
        self.composer_changed();
    }

    /// The completions the list shows.
    pub fn suggestion_labels(&self) -> Vec<String> {
        if self.suggestions.is_shown() {
            self.suggestions.labels()
        } else {
            Vec::new()
        }
    }

    /// Whether the editor's selection is offered as context.
    pub fn has_offer(&self) -> bool {
        self.state.borrow().offer().is_some()
    }

    /// The attachments of the next message.
    pub fn attachment_names(&self) -> Vec<String> {
        self.state
            .borrow()
            .attachments()
            .iter()
            .map(|a| match a {
                term_thread::Attachment::Image { .. } => "image".to_string(),
                term_thread::Attachment::File { path, .. } | term_thread::Attachment::Selection { path, .. } => {
                    path.display().to_string()
                }
                term_thread::Attachment::External { label, .. } => format!("{label} (external)"),
            })
            .collect()
    }

    /// The size of each picture tile above the composer's text, with the width of the picture it shows.
    pub fn thumbnails(&self) -> Vec<(objc2_foundation::NSSize, f64)> {
        fn picture(view: &objc2_app_kit::NSView) -> Option<f64> {
            if let Some(image) = view
                .downcast_ref::<objc2_app_kit::NSImageView>()
                .and_then(|v| v.image())
                .filter(|image| !image.isTemplate())
            {
                return Some(image.size().width);
            }
            view.subviews().iter().find_map(|child| picture(&child))
        }
        let chips = &self.composer.chips;
        chips.layoutSubtreeIfNeeded();
        chips
            .arrangedSubviews()
            .iter()
            .filter_map(|chip| picture(&chip).map(|width| (chip.frame().size, width)))
            .collect()
    }

    /// Clicks the first picture tile above the composer's text; the size the picture opens at over the tab.
    pub fn enlarge_thumbnail(&self) -> Option<objc2_foundation::NSSize> {
        fn clear_button(view: &objc2_app_kit::NSView) -> Option<Retained<objc2_app_kit::NSButton>> {
            if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>()
                && button.isTransparent()
            {
                return Some(objc2::Message::retain(button));
            }
            view.subviews().iter().find_map(|child| clear_button(&child))
        }
        let button = clear_button(&self.composer.chips)?;
        // SAFETY: The button's target is the opener, which lives as long as the app.
        unsafe { button.performClick(None) };
        super::enlarge::shown()
    }

    /// The size of the picture open large over the tab, if one is.
    pub fn enlarged(&self) -> Option<objc2_foundation::NSSize> {
        super::enlarge::shown()
    }

    /// Menu `which`, built as a click on its button would show it.
    pub fn menu(&self, which: TabMenu) -> Retained<objc2_app_kit::NSMenu> {
        match which {
            TabMenu::Slash => self.slash_items(),
            TabMenu::Model => self.model_items(),
            TabMenu::Mode => self.mode_items(),
            TabMenu::History => self.history_items(),
            TabMenu::Remote => self.remote_items(),
        }
    }

    /// The menu of the rules permission request `id` could store, when it offers several.
    pub fn rule_menu(&self, id: u64) -> Option<Retained<objc2_app_kit::NSMenu>> {
        let entry = self.entry(id)?;
        let term_thread::EntryKind::PermissionRequest(request) = &entry.kind else {
            return None;
        };
        let rules = super::requests::always_rules(&request.options);
        (rules.len() > 1).then(|| self.rule_items(entry.id, &rules))
    }

    /// Starts the session with `program` standing in for `claude`, as `start` does once it looked for the CLI; with
    /// `None` as when it found none.
    pub fn launch_with(&self, program: Option<PathBuf>) {
        let env = term_ui::workspace_host::environment_for(&self.cwd, std::time::Duration::ZERO);
        let thread = self.snapshot();
        self.launch(program, env, thread);
    }

    /// The ids of the entries of the main timeline that `matches` picks, oldest first.
    pub fn entry_ids(&self, matches: impl Fn(&term_thread::EntryKind) -> bool) -> Vec<u64> {
        self.snapshot()
            .entries(&term_thread::Lane::Main)
            .iter()
            .filter(|e| matches(&e.kind))
            .map(|e| e.id.get())
            .collect()
    }

    /// The messages waiting to be sent, by their text.
    pub fn queued(&self) -> Vec<String> {
        self.state.borrow().queue().iter().map(|m| m.text.clone()).collect()
    }

    /// The rows of the queue above the composer that show.
    pub fn queue_rows(&self) -> usize {
        if self.queue_view.isHidden() {
            0
        } else {
            self.queue_view.arrangedSubviews().len()
        }
    }

    /// The texts and button titles of the overlay that says what keeps the session from running, while it shows.
    pub fn problem_texts(&self) -> Vec<String> {
        fn collect(view: &NSView, out: &mut Vec<String>) {
            if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>() {
                out.push(button.title().to_string());
            } else if let Some(field) = view.downcast_ref::<objc2_app_kit::NSTextField>() {
                out.push(field.stringValue().to_string());
            }
            for child in view.subviews().iter() {
                collect(&child, out);
            }
        }
        let mut out = Vec::new();
        if !self.problem_holder.isHidden() {
            collect(&self.problem_holder, &mut out);
        }
        out
    }

    /// Clicks the button titled `title` of the overlay that says what keeps the session from running; whether it
    /// showed one.
    pub fn press_problem(&self, title: &str) -> bool {
        fn find(view: &NSView, title: &str) -> Option<Retained<objc2_app_kit::NSButton>> {
            if let Some(button) = view.downcast_ref::<objc2_app_kit::NSButton>()
                && button.title().to_string() == title
            {
                return Some(objc2::Message::retain(button));
            }
            view.subviews().iter().find_map(|child| find(&child, title))
        }
        let button = find(&self.problem_holder, title).filter(|_| !self.problem_holder.isHidden());
        if let Some(button) = &button {
            // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
            unsafe { button.performClick(None) };
        }
        button.is_some()
    }

    /// Clicks the head's button for `part`, as the user would.
    pub fn press_work(&self, part: crate::work::Part) {
        // SAFETY: A click without a sender, on the main thread, sends the button's action as a real click does.
        unsafe { self.header.work_button(part).performClick(None) };
    }

    /// Has the composer's text view do `command`, as the key bound to it does.
    pub fn composer_does(&self, command: objc2::runtime::Sel) {
        // SAFETY: The selectors the self-test sends are standard text commands a text view knows.
        let () = unsafe { objc2::msg_send![&*self.composer.text, doCommandBySelector: command] };
    }

    /// Sends the composer's text view the key with `code` and `characters`, holding `flags`, as typing it does.
    pub fn composer_key(&self, code: u16, characters: &str, flags: objc2_app_kit::NSEventModifierFlags) {
        let window = self.composer.text.window().map_or(0, |w| w.windowNumber());
        let event = objc2_app_kit::NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            objc2_app_kit::NSEventType::KeyDown,
            objc2_foundation::NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            window,
            None,
            &crate::common::util::ns(characters),
            &crate::common::util::ns(characters),
            false,
            code,
        );
        if let Some(event) = event {
            self.composer.text.keyDown(&event);
        }
    }

    /// Whether the user bookmarked the session.
    pub fn is_bookmarked(&self) -> bool {
        self.state.borrow().bookmarked()
    }

    /// Opens every card part with `part`, as a user would click them.
    pub fn open_all(&self, part: super::cards::Part) {
        let ids: Vec<u64> = self
            .snapshot()
            .entries(&term_thread::Lane::Main)
            .iter()
            .map(|e| e.id.get())
            .collect();
        for id in ids {
            self.timeline.toggle(id, part);
        }
    }
}
