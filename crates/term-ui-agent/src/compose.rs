//! What the composer does with what the user types: `@` mentions with completion, the editor's selection offered
//! as context, the chips of attachments, and sending or queueing the message.

use objc2::runtime::Sel;
use objc2::sel;
use objc2_app_kit::{NSButton, NSColor, NSTextInputClient};
use objc2_foundation::NSRange;
use splat_i18n::{t, tf};
use term_thread::{Attachment, Status, UserMessage};

use super::mention::{self, Mention};
use super::suggest::Item;
use super::tab::AgentTab;
use super::{context, format, style, ui};
use crate::common::util::{main_soon, ns};

/// An attachment's name and symbol, whether it is outside text, and its picture.
type Chip = ((String, &'static str), bool, Option<std::sync::Arc<[u8]>>);

/// The message `typed` with `attached`: the line ranges of its file mentions as attachments, and what lies outside
/// the project at `cwd` as outside text.
fn resolve(typed: &str, attached: Vec<Attachment>, cwd: &std::path::Path) -> (String, Vec<Attachment>) {
    let (text, resolved) = mention::resolve_files(typed, cwd);
    let attachments = resolved
        .into_iter()
        .chain(attached)
        .map(|a| super::outside::classify(a, cwd))
        .collect();
    (text, attachments)
}

/// Whether resolving `typed` with `attached` looks at files, which only file mentions and attachments naming a file do.
fn reads_files(typed: &str, attached: &[Attachment]) -> bool {
    mention::parse(typed)
        .iter()
        .any(|(_, m)| matches!(m, Mention::File { .. }))
        || attached
            .iter()
            .any(|a| matches!(a, Attachment::File { .. } | Attachment::Selection { .. }))
}

/// Whether a message sent now goes into the turn that runs: Claude Code reads it as soon as its running tool calls
/// finish, within the turn, as its own prompt does; ACP agents take one prompt at a time and wait in the queue.
fn reaches_turn(status: Status, sending: bool, claude: bool) -> bool {
    claude && status == Status::Running && !sending
}

/// The byte offset in `text` of the UTF-16 offset `units`.
fn byte_offset(text: &str, units: usize) -> usize {
    let mut counted = 0;
    for (at, c) in text.char_indices() {
        if counted >= units {
            return at;
        }
        counted += c.len_utf16();
    }
    text.len()
}

/// The UTF-16 range of the bytes `range` of `text`.
fn utf16_range(text: &str, range: std::ops::Range<usize>) -> NSRange {
    let start = text[..range.start].encode_utf16().count();
    let length = text[range].encode_utf16().count();
    NSRange::new(start, length)
}

/// The slash command typed at `caret`: the first word of the text when it starts with `/` and the caret is in it,
/// with its range and what follows the slash.
fn slash_typing(text: &str, caret: usize) -> Option<(std::ops::Range<usize>, String)> {
    let rest = text.strip_prefix('/')?;
    let end = rest.find(char::is_whitespace).map_or(text.len(), |n| n + 1);
    if caret == 0 || caret > end {
        return None;
    }
    Some((0..end, text[1..end].to_string()))
}

/// The commands whose name holds `query`, those that start with it first.
fn matching_commands(commands: Vec<(String, String)>, query: &str) -> Vec<(String, String)> {
    let query = query.to_lowercase();
    let (mut first, rest): (Vec<_>, Vec<_>) = commands
        .into_iter()
        .map(|(name, detail)| (name.trim_start_matches('/').to_string(), detail))
        .filter(|(name, _)| name.to_lowercase().contains(&query))
        .partition(|(name, _)| name.to_lowercase().starts_with(&query));
    first.extend(rest);
    first
}

impl AgentTab {
    /// Return in the composer: takes a completion, opens the task overview for `/tasks`, else sends what was typed
    /// or queues it while a turn runs.
    pub fn submit(&self) {
        let typed_in_full = self
            .suggestions
            .chosen()
            .is_some_and(|(item, range)| self.composer.text().get(range) == Some(item.insert.as_str()));
        if typed_in_full {
            self.suggestions.hide();
        } else if self.accept_suggestion() {
            return;
        }
        let typed = self.composer.text().trim_end().to_string();
        if typed.trim() == "/tasks" {
            self.composer.clear();
            self.show_work(crate::work::Part::All, None);
            return;
        }
        if let Some(dialog) = super::dialogs::Dialog::from_typed(&typed)
            && super::dialogs::offers(self, &dialog)
        {
            self.composer.clear();
            self.open_dialog(dialog);
            return;
        }
        let attached = self.state.borrow_mut().take_attachments();
        if typed.trim().is_empty() && attached.is_empty() {
            return;
        }
        self.composer.clear();
        self.refresh_chips();
        if !reads_files(&typed, &attached) {
            let (text, attachments) = resolve(&typed, attached, &self.cwd);
            return self.send_typed(text, attachments);
        }
        let (key, cwd) = (self.key, self.cwd.clone());
        let kept = (typed.clone(), attached.clone());
        let queued = term_base::lane::DISK.ask(
            move || resolve(&typed, attached, &cwd),
            move |resolved| {
                main_soon(move || {
                    if let (Some(tab), Some((text, attachments))) = (super::get(key), resolved) {
                        tab.send_typed(text, attachments);
                    }
                });
            },
        );
        if queued.is_err() {
            let (text, attachments) = resolve(&kept.0, kept.1, &self.cwd);
            self.send_typed(text, attachments);
        }
    }

    /// Sends `text` with `attachments`, the output of the terminal it mentions along.
    fn send_typed(&self, text: String, mut attachments: Vec<Attachment>) {
        let controller = self.controller();
        if let Some(controller) = &controller
            && mention::mentions(&text, &Mention::Terminal)
        {
            attachments.extend(context::terminal(controller));
        }
        let message = UserMessage {
            text,
            attachments,
            message_id: None,
        };
        self.dispatch(message);
    }

    /// Sends `message` now, into the running turn where Claude Code takes it there, or queues it while a turn runs or
    /// the session is not there yet.
    fn dispatch(&self, message: UserMessage) {
        let (busy, into_turn) = {
            let state = self.state.borrow();
            let claude = state.source().session().is_some() && state.source().is_live();
            (
                state.running() || !state.source().is_live(),
                reaches_turn(state.status(), state.is_sending(), claude),
            )
        };
        if into_turn {
            self.deliver(message);
            return;
        }
        if busy {
            if self.waits_for_start() {
                self.wait_for_start(message);
            } else {
                self.state.borrow_mut().enqueue(message);
            }
            self.refresh_queue();
            self.resume_parked();
            return;
        }
        self.send_message(message);
    }

    /// Follows a keystroke in the composer; the pills follow the session's state, not the text.
    pub(crate) fn composer_changed(&self) {
        self.composer.fit();
        let text = self.composer.text();
        self.refresh_send(&text);
        self.update_suggestions(&text);
    }

    /// Commands of the composer's text view: arrows and Tab steer an open completion list, Esc closes it or, with
    /// nothing typed, stops the turn.
    pub(crate) fn composer_command(&self, command: Sel) -> bool {
        let open = self.suggestions.is_shown();
        if open && command == sel!(moveUp:) {
            self.suggestions.step(-1);
            return true;
        }
        if open && command == sel!(moveDown:) {
            self.suggestions.step(1);
            return true;
        }
        if open && command == sel!(insertTab:) {
            return self.accept_suggestion();
        }
        // A text view turns Esc into `complete:` unless the key bindings say `cancelOperation:`.
        if command == sel!(cancelOperation:) || command == sel!(complete:) {
            #[cfg(feature = "dictation")]
            if term_ui_dictation::cancel_in(&self.composer.text) {
                return true;
            }
            if open {
                self.suggestions.hide();
            } else if self.composer.text().is_empty() && super::stop::stop_key_pressed(self.mtm) {
                self.stop_turn(super::Stop::Key);
            }
            return true;
        }
        false
    }

    /// Offers completions for the mention at the caret: the terminal and the project's files.
    fn update_suggestions(&self, text: &str) {
        let caret = byte_offset(text, self.composer.text.selectedRange().location);
        if let Some((range, query)) = slash_typing(text, caret) {
            let items = matching_commands(self.known_commands(), &query)
                .into_iter()
                .map(|(name, detail)| Item {
                    label: format!("/{name}"),
                    insert: format!("/{name}"),
                    detail,
                    symbol: "slash.circle",
                })
                .collect();
            self.suggestions.ask();
            self.suggestions.show(items, range);
            return;
        }
        let Some((range, query)) = mention::typing(text, caret) else {
            self.suggestions.hide();
            return;
        };
        let lower = query.to_lowercase();
        let mut special = Vec::new();
        if mention::TERMINAL.starts_with(&lower) {
            special.push(Item {
                label: format!("@{}", mention::TERMINAL),
                detail: t("Ausgabe des Terminals").to_string(),
                insert: format!("@{}", mention::TERMINAL),
                symbol: "terminal",
            });
        }
        self.suggestions.show(special.clone(), range.clone());
        let Some(controller) = self.controller() else { return };
        if query.is_empty() {
            return;
        }
        let asked = self.suggestions.ask();
        let key = self.key;
        let answer = move |hits: Vec<crate::Hit>| {
            let files: Vec<(String, String)> = hits
                .into_iter()
                .map(|hit| {
                    let name = hit.path.rsplit('/').next().unwrap_or(&hit.path).to_string();
                    (name, hit.path)
                })
                .collect();
            main_soon(move || {
                let Some(tab) = super::get(key) else { return };
                if !tab.suggestions.is_current(asked) {
                    return;
                }
                let mut items = special;
                items.extend(files.into_iter().map(|(name, path)| Item {
                    label: name,
                    insert: mention::written(&path),
                    detail: path,
                    symbol: "doc.text",
                }));
                tab.suggestions.show(items, range);
            });
        };
        crate::host::host().query_files(controller.id(), &query, super::suggest::SHOWN, Box::new(answer));
    }

    /// Replaces the typed mention with the chosen completion; whether one was open.
    pub(super) fn accept_suggestion(&self) -> bool {
        if !self.suggestions.is_shown() {
            return false;
        }
        let Some((item, range)) = self.suggestions.chosen() else {
            self.suggestions.hide();
            return false;
        };
        self.suggestions.hide();
        let text = self.composer.text();
        if range.end > text.len() {
            return true;
        }
        let replaced = utf16_range(&text, range);
        let insert = ns(&format!("{} ", item.insert));
        // SAFETY: The replacement is a string and the range lies inside the text, as `insertText` asks.
        unsafe { self.composer.text.insertText_replacementRange(&insert, replaced) };
        true
    }

    /// The completion clicked in the list.
    pub(crate) fn pick_suggestion(&self, index: usize) {
        self.suggestions.choose(index);
        self.accept_suggestion();
        self.focus_composer();
    }

    /// Offers the selection of the editor as context, as the composer gets the focus.
    pub(super) fn offer_selection(&self) {
        let offer = self.controller().and_then(|c| context::editor_selection(&c));
        let changed = self.state.borrow_mut().offer_selection(offer);
        if changed {
            self.refresh_chips();
        }
    }

    /// Takes the offered selection as an attachment.
    pub fn use_selection(&self) {
        let offer = self.state.borrow_mut().take_offer();
        if let Some(offer) = offer {
            let attachment = super::outside::classify(offer, &self.cwd);
            self.state.borrow_mut().attach([attachment]);
        }
        self.refresh_chips();
    }

    /// The attachments above the text, each with a button that drops it, and the offered selection.
    pub(super) fn refresh_chips(&self) {
        let chips = &self.composer.chips;
        for view in chips.arrangedSubviews().iter() {
            chips.removeArrangedSubview(&view);
            view.removeFromSuperview();
        }
        let (names, offer) = {
            let state = self.state.borrow();
            let names: Vec<Chip> = state
                .attachments()
                .iter()
                .map(|a| {
                    let picture = match a {
                        Attachment::Image { data, .. } => Some(data.clone()),
                        _ => None,
                    };
                    (self.chip_name(a), matches!(a, Attachment::External { .. }), picture)
                })
                .collect();
            (names, state.offer().map(|o| self.chip_name(o).0))
        };
        for (n, ((name, symbol), outside, picture)) in names.iter().enumerate() {
            let tag = isize::try_from(n).unwrap_or(-1);
            let removal = tf("{} entfernen", &[name]);
            if let Some(picture) = picture {
                let remove = ui::symbol_button(
                    self.mtm,
                    "xmark.circle.fill",
                    &removal,
                    style::chat(13.0),
                    &self.target,
                    sel!(ideAgentRemoveAttachment:),
                    tag,
                );
                chips.addArrangedSubview(&ui::picture_tile(self.mtm, picture, name, Some(&remove), None));
                continue;
            }
            let remove = ui::symbol_button(
                self.mtm,
                "xmark",
                &removal,
                style::chat(8.0),
                &self.target,
                sel!(ideAgentRemoveAttachment:),
                tag,
            );
            chips.addArrangedSubview(&self.chip(symbol, name, &remove, (false, *outside)));
        }
        if let Some(offer) = &offer {
            let take = ui::symbol_button(
                self.mtm,
                "plus.circle",
                t("Auswahl anhängen"),
                style::chat(10.0),
                &self.target,
                sel!(ideAgentUseSelection:),
                0,
            );
            let label = tf("Auswahl: {}", &[offer]);
            chips.addArrangedSubview(&self.chip("text.cursor", &label, &take, (true, false)));
        }
        chips.addArrangedSubview(&ui::spacer(self.mtm));
        chips.setHidden(names.is_empty() && offer.is_none());
        self.refresh_composer();
    }

    fn chip_name(&self, attachment: &Attachment) -> (String, &'static str) {
        match attachment {
            Attachment::Image { name, .. } => (name.clone().unwrap_or_else(|| t("Bild").to_string()), "photo"),
            Attachment::File { path, lines } => {
                let mut name = format::short_path(&path.to_string_lossy(), &self.cwd);
                if let Some((a, b)) = lines {
                    name.push_str(&format!(":{a}–{b}"));
                }
                (name, "doc.text")
            }
            Attachment::Selection { path, lines, .. } => (
                format!(
                    "{}:{}–{}",
                    format::short_path(&path.to_string_lossy(), &self.cwd),
                    lines.0,
                    lines.1
                ),
                "text.cursor",
            ),
            Attachment::External { label, .. } => (label.clone(), "globe"),
        }
    }

    /// A chip: offered rather than attached, or with the mark of outside text.
    fn chip(
        &self,
        symbol: &str,
        name: &str,
        button: &NSButton,
        (offered, outside): (bool, bool),
    ) -> objc2::rc::Retained<objc2_app_kit::NSBox> {
        let mtm = self.mtm;
        let tone = if offered {
            NSColor::secondaryLabelColor()
        } else {
            NSColor::labelColor()
        };
        let icon = ui::icon(mtm, symbol, name, style::chat(10.0), &NSColor::secondaryLabelColor());
        let label = ui::label(mtm, name, &style::chat_small(), &tone);
        let line = if outside {
            ui::row(mtm, 3.0, &[&icon, &label, &super::outside::marker(mtm), button])
        } else {
            ui::row(mtm, 3.0, &[&icon, &label, button])
        };
        let border = if offered {
            NSColor::controlAccentColor().colorWithAlphaComponent(0.6)
        } else if outside {
            NSColor::systemOrangeColor().colorWithAlphaComponent(0.6)
        } else {
            NSColor::separatorColor()
        };
        ui::framed(mtm, &line, &border, None, 5.0, ui::insets(2.0, 5.0, 2.0, 4.0))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn slash_typing_takes_only_the_first_word() {
        assert_eq!(super::slash_typing("/com", 4), Some((0..4, "com".to_string())));
        assert_eq!(super::slash_typing("/", 1), Some((0..1, String::new())));
        assert_eq!(
            super::slash_typing("/review src", 3),
            Some((0..7, "review".to_string()))
        );
        assert_eq!(super::slash_typing("/review src", 9), None);
        assert_eq!(super::slash_typing("see /src", 8), None);
        assert_eq!(super::slash_typing("/x", 0), None);
    }

    #[test]
    fn matching_commands_puts_prefixes_first() {
        let commands = vec![
            ("init".to_string(), String::new()),
            ("/compact".to_string(), "c".to_string()),
            ("recompile".to_string(), String::new()),
        ];
        let names: Vec<String> = super::matching_commands(commands, "Comp")
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(names, ["compact", "recompile"]);
    }

    use super::*;

    #[test]
    fn only_claude_takes_a_message_into_a_running_turn() {
        assert!(reaches_turn(Status::Running, false, true));
        assert!(
            !reaches_turn(Status::Running, false, false),
            "an ACP agent or the own agent queues it"
        );
        assert!(
            !reaches_turn(Status::Running, true, true),
            "a checkpoint is being taken"
        );
        assert!(
            !reaches_turn(Status::RequiresAction, false, true),
            "it waits for an answer first"
        );
        assert!(
            !reaches_turn(Status::Idle, false, true),
            "an idle session gets it as a new turn"
        );
    }

    #[test]
    fn offsets_count_utf16_units_as_text_views_do() {
        let text = "ä @src";
        assert_eq!(byte_offset(text, 2), 3);
        assert_eq!(utf16_range(text, 3..7), NSRange::new(2, 4));
    }
}
