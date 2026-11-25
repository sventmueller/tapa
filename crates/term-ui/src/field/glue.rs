//! What a session view does for its input field (E7): following the shell's marks, sending what the user typed,
//! stepping through the history, and handing the keyboard back and forth.

use objc2::{DefinedClass, MainThreadOnly};
use objc2_app_kit::NSFont;

use super::model::CONTINUATION_WAIT;
use crate::TerminalView;

impl TerminalView {
    pub(super) fn with_field<R>(&self, f: impl FnOnce(&super::InputField, &mut super::model::Field) -> R) -> Option<R> {
        let field = self.ivars().field.borrow();
        let field = field.as_ref()?;
        let mut state = self.ivars().field_state.borrow_mut();
        Some(f(field, &mut state))
    }

    /// The shell's marks and the screen move the field: `mark` is `'A'`, `'B'` or `'C'`.
    pub(crate) fn field_mark(&self, mark: char) {
        self.with_field(|_, state| match mark {
            'A' => state.prompt(),
            'B' => state.input(),
            _ => state.command_started(),
        });
        self.update_field();
    }

    /// Shows or hides the field as the shell and the screen now are; a field that shows takes the keyboard from the
    /// terminal, one that hides gives it back.
    pub(crate) fn update_field(&self) {
        let full_screen = self
            .with_session(|session| session.input_modes().alternate_screen)
            .unwrap_or(false);
        let terminal_has_keys = self.window().and_then(|w| w.firstResponder()).is_some_and(|responder| {
            let responder: &objc2::runtime::AnyObject = &responder;
            let own: &objc2::runtime::AnyObject = self;
            std::ptr::eq(responder, own)
        });
        let mut give_back = false;
        self.with_field(|field, state| {
            state.full_screen = full_screen;
            if let Some(toggle) = field.toggle.get() {
                let integration = state.phase != super::model::Phase::Unknown;
                toggle.show(super::toggle::Look::of(state.enabled, integration, full_screen));
            }
            let visible = state.visible();
            if visible != field.is_visible() {
                if !visible && field.has_focus() {
                    give_back = true;
                }
                field.set_visible(visible, terminal_has_keys);
            }
        });
        if give_back && let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
        self.field_hide_prompt();
        if self.field_shown() {
            self.field_show_context();
            self.field_text_changed();
        }
    }

    /// Has the own core leave the live prompt out of the view while the field stands in for it; copy mode moves over
    /// the rows as the shell printed them, so it shows them all.
    pub(crate) fn field_hide_prompt(&self) {
        let hide = self.with_field(|_, state| state.enabled && !state.raw).unwrap_or(false) && !self.in_copy_mode();
        if self.with_session(|session| session.hide_live_prompt(hide)) == Some(true) {
            let id = self.id();
            crate::kit::main_soon(move || {
                if let Some(view) = crate::find(id) {
                    view.refresh_now();
                }
            });
        }
    }

    /// The chips above the field show the folder the shell reported and the branch of its repository.
    fn field_show_context(&self) {
        let cwd = self.cwd();
        let (folder, branch) = super::chips::texts(&cwd, &term_base::home(), crate::git::branch(&cwd));
        self.with_field(|field, _| field.show_context(&folder, branch.as_deref()));
    }

    /// What the chips above the field say, folder first.
    pub fn field_context(&self) -> Vec<String> {
        self.with_field(|field, _| field.context()).unwrap_or_default()
    }

    /// Gives the field its switch in the window's toolbar (E7) and the terminal's face.
    pub(crate) fn attach_field_icon(&self) {
        let id = self.id();
        self.with_field(|field, _| {
            field.toggle.get_or_init(|| super::toggle::FieldToggle::new(id));
        });
        self.style_field();
        self.update_field();
    }

    /// A program entered or left the alternate screen since the field last looked: the field and its icon step aside
    /// or come back; the marks of OSC 133 do not tell.
    pub(crate) fn field_follow_screen(&self) {
        let Ok(state) = self.ivars().field_state.try_borrow() else {
            return;
        };
        let was = state.full_screen;
        drop(state);
        let now = self
            .with_session(|session| session.input_modes().alternate_screen)
            .unwrap_or(false);
        if now != was {
            self.update_field();
        }
    }

    /// Turns the field of this session on or off (F209).
    pub fn toggle_field(&self) {
        self.with_field(|_, state| state.enabled = !state.enabled);
        self.update_field();
    }

    /// Turns the field of this session on or off, as a session stored with it on or off comes back.
    pub fn set_field_enabled(&self, on: bool) {
        self.with_field(|_, state| state.enabled = on);
        self.update_field();
    }

    /// Whether the field shows now.
    pub fn field_shown(&self) -> bool {
        self.with_field(|field, _| field.is_visible()).unwrap_or(false)
    }

    /// The field's text and the grey hint beside it.
    pub fn field_contents(&self) -> (String, String) {
        self.with_field(|field, _| (field.text(), field.hint()))
            .unwrap_or_default()
    }

    /// Puts `text` into the field without sending it, as typing it would.
    pub fn type_into_field(&self, text: &str) {
        self.with_field(|field, _| field.set_text(text));
    }

    /// Whether the field is on for this session.
    pub fn field_enabled(&self) -> bool {
        self.with_field(|_, state| state.enabled).unwrap_or(false)
    }

    /// The field's text changed; only the field is touched, as this runs while its state may be borrowed. The
    /// history's suggestion for it shows in grey (E8): the rest of a command that starts with what was typed, or with
    /// nothing typed the command most likely next.
    pub(crate) fn field_text_changed(&self) {
        if let Ok(field) = self.ivars().field.try_borrow()
            && let Some(field) = field.as_ref()
        {
            field.fit();
            let text = field.text();
            self.field_suggest(field, &text);
        }
    }

    /// →: takes the history's suggestion; whether there was one.
    pub(crate) fn field_accept(&self) -> bool {
        let Some(line) = self.ivars().suggestion.borrow().clone() else {
            return false;
        };
        self.with_field(|field, _| field.set_text(&line));
        true
    }

    /// Return: what the field holds goes to the shell as one paste and a carriage return.
    pub(crate) fn field_submit(&self) {
        let bracketed = self
            .with_session(|session| session.input_modes().bracketed_paste)
            .unwrap_or(false);
        let Some(bytes) = self.with_field(|field, state| {
            let bytes = state.submit(&field.text(), bracketed);
            field.set_text("");
            bytes
        }) else {
            return;
        };
        self.with_session(|session| {
            session.scroll(term_session::Scroll::Bottom);
            session.write(bytes);
        });
        let id = self.id();
        crate::kit::main_after(CONTINUATION_WAIT, move || {
            if let Some(view) = crate::find(id) {
                view.with_field(|_, state| state.continuation_timed_out());
                view.update_field();
            }
        });
    }

    /// Puts `text` into the field and sends it, as typing it and Return would; nothing while the field is away.
    pub fn submit_command(&self, text: &str) -> bool {
        let shown = self.with_field(|field, _| {
            let shown = field.is_visible();
            if shown {
                field.set_text(text);
            }
            shown
        });
        if shown == Some(true) {
            self.field_submit();
            return true;
        }
        false
    }

    /// ↑ (`older`) and ↓ step through the field's history.
    pub(crate) fn field_history(&self, older: bool) {
        self.with_field(|field, state| {
            let entry = if older {
                state.older(&field.text())
            } else {
                state.newer()
            };
            if let Some(entry) = entry {
                field.set_text(&entry);
            }
        });
    }

    /// ⌃R: the newest earlier entry holding what the field holds.
    pub(crate) fn field_search(&self) {
        self.with_field(|field, state| {
            if let Some(entry) = state.search(&field.text()) {
                field.set_text(&entry);
            } else {
                objc2_app_kit::NSBeep();
            }
        });
    }

    /// Hands the keyboard on to the field while it shows, however the terminal got it: a click once its button is up,
    /// a tab switch, the window coming to the front.
    pub(crate) fn pass_keys_to_field(&self) {
        if !self.field_shown() || self.in_copy_mode() {
            return;
        }
        let id = self.id();
        crate::kit::main_soon(move || {
            let Some(view) = crate::find(id).filter(|view| !view.in_copy_mode() && !view.mouse_held()) else {
                return;
            };
            view.with_field(|field, state| {
                if state.visible() {
                    field.set_visible(true, true);
                }
            });
        });
    }

    /// Escape: the keyboard goes to the terminal for raw typing until the next prompt (F99).
    pub(crate) fn field_leave(&self) {
        self.with_field(|_, state| state.raw = true);
        self.update_field();
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(self));
        }
    }

    /// The field takes up the terminal's face, size and colours.
    pub(crate) fn style_field(&self) {
        let theme = self.ivars().theme.borrow().palette.clone();
        let colour = |c: term_session::Rgb| {
            objc2_app_kit::NSColor::colorWithSRGBRed_green_blue_alpha(
                f64::from(c.r) / 255.0,
                f64::from(c.g) / 255.0,
                f64::from(c.b) / 255.0,
                1.0,
            )
        };
        let size = self.font_size();
        let family = crate::host().config().face().family;
        let font = (!family.is_empty() && family != "SF Mono")
            .then(|| NSFont::fontWithName_size(&crate::kit::ns(&family), size))
            .flatten()
            .unwrap_or_else(|| NSFont::monospacedSystemFontOfSize_weight(size, 0.0));
        let _ = self.mtm();
        self.with_field(|field, _| field.style(&font, &colour(theme.foreground), &colour(theme.background)));
    }
}
