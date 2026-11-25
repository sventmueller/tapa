//! The format commands of splat's toolbar as source edits: a style toggles on the selection or the word at the caret,
//! a block kind toggles on the selected lines; each is one undo step.

use objc2::MainThreadOnly;
use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString, NSTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use splat_i18n::t;
use splat_markdown::commands::{self, Edit, InlineStyle, ListStyle};
use splat_markdown::doc::AlertKind;
use splat_markdown::text::TextAt;
use term_ui::kit::alert::{Alert, Role, first};

use super::{Command, apply, insert, text_and_selection};
use crate::EditorTextView;
use crate::util::{constant, ns};

/// A command that takes one of several kinds, which a menu of the format bar offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Choice {
    /// A code block in the language at this place of [`languages`].
    CodeBlock(usize),
    Alert(AlertKind),
    Diagram(insert::Diagram),
}

/// Where the tags of each kind of choice begin, past those of [`Command`].
const ALERT_TAGS: isize = 100;
const DIAGRAM_TAGS: isize = 120;
const LANGUAGE_TAGS: isize = 200;

impl Choice {
    pub(crate) fn tag(self) -> isize {
        match self {
            Choice::CodeBlock(n) => LANGUAGE_TAGS + n.cast_signed(),
            Choice::Alert(kind) => ALERT_TAGS + index_of(&AlertKind::ALL, kind),
            Choice::Diagram(diagram) => DIAGRAM_TAGS + index_of(&insert::Diagram::ALL, diagram),
        }
    }

    pub(crate) fn of_tag(tag: isize) -> Option<Choice> {
        let at = |base: isize| usize::try_from(tag - base).ok();
        if tag >= LANGUAGE_TAGS {
            at(LANGUAGE_TAGS)
                .filter(|n| *n < languages().count())
                .map(Choice::CodeBlock)
        } else if tag >= DIAGRAM_TAGS {
            insert::Diagram::ALL
                .get(at(DIAGRAM_TAGS)?)
                .copied()
                .map(Choice::Diagram)
        } else if tag >= ALERT_TAGS {
            AlertKind::ALL.get(at(ALERT_TAGS)?).copied().map(Choice::Alert)
        } else {
            None
        }
    }
}

fn index_of<T: PartialEq>(all: &[T], one: T) -> isize {
    all.iter().position(|x| *x == one).unwrap_or_default().cast_signed()
}

/// The languages a code block can be in, by info string and name; the first is none.
pub(crate) fn languages() -> impl Iterator<Item = (&'static str, &'static str)> {
    std::iter::once(("", t("Ohne Sprache"))).chain(splat_markdown::code::MENU.iter().copied())
}

pub(crate) fn run(view: &EditorTextView, command: Command) {
    match command {
        Command::Link => {
            if let Some(url) = ask_link(view) {
                link_with(view, &url);
            }
            return;
        }
        Command::Table => return insert::table(view),
        Command::Image => return insert::image(view),
        _ => {}
    }
    let (text, sel) = text_and_selection(view);
    let doc = splat_markdown::reader::parse(&text);
    let inline = |style| commands::toggle_inline(&doc, sel.clone(), style);
    let heading = |level: u8| {
        let current = commands::active_at(&doc, sel.start).heading;
        commands::set_heading(&doc, sel.clone(), if current == level { 0 } else { level })
    };
    let edit: Edit = match command {
        Command::Bold => inline(InlineStyle::Bold),
        Command::Italic => inline(InlineStyle::Italic),
        Command::Strikethrough => inline(InlineStyle::Strikethrough),
        Command::Code => inline(InlineStyle::Code),
        Command::Underline => inline(InlineStyle::Underline),
        Command::Highlight => inline(InlineStyle::Highlight),
        Command::Superscript => inline(InlineStyle::Superscript),
        Command::Subscript => inline(InlineStyle::Subscript),
        Command::RemoveFormatting => commands::remove_formatting(&doc, sel),
        Command::Paragraph => commands::set_heading(&doc, sel, 0),
        Command::Heading1 => heading(1),
        Command::Heading2 => heading(2),
        Command::Heading3 => heading(3),
        Command::Heading4 => heading(4),
        Command::Heading5 => heading(5),
        Command::Heading6 => heading(6),
        Command::BulletList => commands::toggle_list(&text, sel, ListStyle::Bullet),
        Command::NumberedList => commands::toggle_list(&text, sel, ListStyle::Numbered),
        Command::TaskList => commands::toggle_list(&text, sel, ListStyle::Task),
        Command::Quote => commands::toggle_quote(&text, sel),
        Command::CodeBlock => commands::code_block_language(&doc, sel, ""),
        Command::Formula => commands::insert_formula(&text, sel),
        Command::Footnote => commands::insert_footnote(&text, sel),
        Command::Rule => commands::insert_rule(&text, sel),
        Command::Toc => match commands::insert_toc(&doc, sel) {
            Some(edit) => edit,
            None => return,
        },
        _ => return,
    };
    apply(view, &edit, command.title().trim_end_matches(" …"));
}

/// Runs the command `choice` on the selection of `view`.
pub(crate) fn choose(view: &EditorTextView, choice: Choice) {
    let (text, sel) = text_and_selection(view);
    let doc = splat_markdown::reader::parse(&text);
    let (edit, name) = match choice {
        Choice::CodeBlock(n) => {
            let language = languages().nth(n).map_or("", |(info, _)| info);
            (commands::code_block_language(&doc, sel, language), t("Codeblock"))
        }
        Choice::Alert(kind) => (commands::insert_alert(&doc, sel, kind), t("Hinweisbox")),
        Choice::Diagram(diagram) => (commands::insert_diagram(&text, sel, &diagram.template()), t("Diagramm")),
    };
    apply(view, &edit, name);
}

/// Inserts a link to `url` at the selection, which becomes its text; without a selection the address is the text.
pub(crate) fn link_with(view: &EditorTextView, url: &str) {
    let (text, sel) = text_and_selection(view);
    let selected = text.at(sel.clone()).to_string();
    let label = if selected.trim().is_empty() {
        url
    } else {
        selected.as_str()
    };
    let edit = commands::insert_link(&text, sel, url, label);
    apply(view, &edit, t("Link einfügen"));
}

/// Asks for the address of a link, offering one on the pasteboard; `None` when the user cancels.
fn ask_link(view: &EditorTextView) -> Option<String> {
    let mtm = view.mtm();
    let offered = NSPasteboard::generalPasteboard()
        .stringForType(constant!(NSPasteboardTypeString))
        .map(|s| s.to_string().trim().to_string())
        .filter(|s| s.contains("://") && !s.contains(char::is_whitespace))
        .unwrap_or_default();
    let field = NSTextField::textFieldWithString(&ns(&offered), mtm);
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(300.0, 24.0)));
    let answer = Alert::new(mtm, t("Link einfügen"))
        .informative(t("Adresse des Links:"))
        .button(t("Einfügen"), Role::Plain)
        .button(t("Abbrechen"), Role::Plain)
        .accessory(&field)
        .focus(&field)
        .run_modal();
    if !first(answer) {
        return None;
    }
    let url = field.stringValue().to_string().trim().to_string();
    (!url.is_empty()).then_some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_choice_comes_back_from_its_tag_and_no_tag_of_a_command_is_one() {
        let choices = (0..languages().count())
            .map(Choice::CodeBlock)
            .chain(AlertKind::ALL.map(Choice::Alert))
            .chain(insert::Diagram::ALL.map(Choice::Diagram));
        for choice in choices {
            assert_eq!(Choice::of_tag(choice.tag()), Some(choice));
        }
        for command in super::super::ALL {
            assert_eq!(Choice::of_tag(command as isize), None, "{command:?}");
        }
        assert_eq!(Choice::of_tag(LANGUAGE_TAGS + 999), None);
    }
}
