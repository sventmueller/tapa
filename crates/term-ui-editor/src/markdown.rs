//! What a Markdown document offers beyond the code editor (E59): the format commands of splat's toolbar in the Format
//! menu and the format bar over the document (E195), pasting from Word or the web as Markdown, opening it in Splat,
//! and the switch of the preview beside it, which the app shows. The commands go through the editor's one action
//! `termMarkdown:`, told apart by the menu item's tag.

mod bar;
mod files;
mod format;
mod insert;
mod paste;
mod splat_app;

use std::ops::Range;
use std::path::{Path, PathBuf};

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::NSMenuItem;
use objc2_foundation::NSRange;
use splat_i18n::t;
use splat_markdown::commands::Edit;
use splat_markdown::text::Byte;

use crate::EditorTextView;
use crate::util::{ns, separator, submenu, tagged};
pub(crate) use bar::FormatBar;
pub use splat_app::open_in_splat_item;

/// A command of the menus, by the tag of its menu item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    Bold = 1,
    Italic,
    Strikethrough,
    Code,
    Link,
    Heading1,
    Heading2,
    Heading3,
    BulletList,
    NumberedList,
    TaskList,
    Quote,
    CodeBlock,
    PastePlain,
    OpenInSplat,
    Underline,
    Highlight,
    Superscript,
    Subscript,
    RemoveFormatting,
    Paragraph,
    Heading4,
    Heading5,
    Heading6,
    Image,
    Table,
    Formula,
    Toc,
    Footnote,
    Rule,
    Preview,
}

const ALL: [Command; 31] = [
    Command::Bold,
    Command::Italic,
    Command::Strikethrough,
    Command::Code,
    Command::Link,
    Command::Heading1,
    Command::Heading2,
    Command::Heading3,
    Command::BulletList,
    Command::NumberedList,
    Command::TaskList,
    Command::Quote,
    Command::CodeBlock,
    Command::PastePlain,
    Command::OpenInSplat,
    Command::Underline,
    Command::Highlight,
    Command::Superscript,
    Command::Subscript,
    Command::RemoveFormatting,
    Command::Paragraph,
    Command::Heading4,
    Command::Heading5,
    Command::Heading6,
    Command::Image,
    Command::Table,
    Command::Formula,
    Command::Toc,
    Command::Footnote,
    Command::Rule,
    Command::Preview,
];

impl Command {
    /// The command whose menu title is `title`.
    #[cfg(feature = "selftest")]
    pub(crate) fn named(title: &str) -> Option<Command> {
        ALL.into_iter().find(|command| command.title() == title)
    }

    fn of_tag(tag: isize) -> Option<Command> {
        ALL.into_iter().find(|command| *command as isize == tag)
    }

    /// The name of its shortcut in the keymap (E26), for those that have one.
    fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Command::Bold => "format.bold",
            Command::Italic => "format.italic",
            Command::Strikethrough => "format.strikethrough",
            Command::Code => "format.code",
            Command::Link => "format.link",
            Command::Heading1 => "format.heading1",
            Command::Heading2 => "format.heading2",
            Command::Heading3 => "format.heading3",
            Command::PastePlain => "edit.pastePlain",
            _ => return None,
        })
    }

    fn title(self) -> &'static str {
        match self {
            Command::Bold => t("Fett"),
            Command::Italic => t("Kursiv"),
            Command::Strikethrough => t("Durchgestrichen"),
            Command::Code => t("Code"),
            Command::Link => t("Link …"),
            Command::Heading1 => t("Überschrift 1"),
            Command::Heading2 => t("Überschrift 2"),
            Command::Heading3 => t("Überschrift 3"),
            Command::BulletList => t("Aufzählung"),
            Command::NumberedList => t("Nummerierte Liste"),
            Command::TaskList => t("Aufgabe"),
            Command::Quote => t("Zitat"),
            Command::CodeBlock => t("Codeblock"),
            Command::PastePlain => t("Als reinen Text einsetzen"),
            Command::OpenInSplat => t("In Splat öffnen"),
            Command::Underline => t("Unterstrichen"),
            Command::Highlight => t("Textmarker"),
            Command::Superscript => t("Hochgestellt"),
            Command::Subscript => t("Tiefgestellt"),
            Command::RemoveFormatting => t("Formatierung entfernen"),
            Command::Paragraph => t("Fließtext"),
            Command::Heading4 => t("Überschrift 4"),
            Command::Heading5 => t("Überschrift 5"),
            Command::Heading6 => t("Überschrift 6"),
            Command::Image => t("Bild …"),
            Command::Table => t("Tabelle …"),
            Command::Formula => t("Formel"),
            Command::Toc => t("Inhaltsverzeichnis"),
            Command::Footnote => t("Fußnote"),
            Command::Rule => t("Trennlinie"),
            Command::Preview => t("Vorschau daneben ein-/ausblenden"),
        }
    }

    fn item(self, mtm: MainThreadMarker, shortcut: &dyn Fn(&str) -> String) -> Retained<NSMenuItem> {
        let key = self.shortcut().map(shortcut).unwrap_or_default();
        tagged(mtm, self.title(), sel!(termMarkdown:), &key, self as isize)
    }
}

/// The Format menu, with the shortcut `shortcut` gives each command's name in the keymap (E26).
pub fn format_menu(mtm: MainThreadMarker, shortcut: &dyn Fn(&str) -> String) -> Retained<NSMenuItem> {
    use Command::*;
    let groups: [&[Command]; 4] = [
        &[Bold, Italic, Strikethrough, Code],
        &[Link],
        &[Heading1, Heading2, Heading3],
        &[BulletList, NumberedList, TaskList, Quote, CodeBlock],
    ];
    let mut items = Vec::new();
    for (n, group) in groups.iter().enumerate() {
        if n > 0 {
            items.push(separator(mtm));
        }
        items.extend(group.iter().map(|command| command.item(mtm, shortcut)));
    }
    submenu(mtm, t("Format"), items).0
}

/// The File menu's entry that opens a Markdown document in Splat, hidden where Splat is not installed.
pub fn open_in_splat_menu_item(mtm: MainThreadMarker) -> Retained<NSMenuItem> {
    let open = Command::OpenInSplat.item(mtm, &|_: &str| String::new());
    open.setHidden(splat_app::location().is_none());
    open
}

/// The View menu's entry that shows or hides the preview beside a Markdown document.
pub fn preview_item(mtm: MainThreadMarker, shortcut: &dyn Fn(&str) -> String) -> Retained<NSMenuItem> {
    Command::Preview.item(mtm, shortcut)
}

/// The Edit menu's entry that pastes text without its formatting.
pub fn paste_plain_item(mtm: MainThreadMarker, shortcut: &dyn Fn(&str) -> String) -> Retained<NSMenuItem> {
    Command::PastePlain.item(mtm, shortcut)
}

/// Runs the command of the menu item tagged `tag` on the editor `view`.
pub(crate) fn perform(view: &EditorTextView, tag: isize) {
    if let Some(choice) = format::Choice::of_tag(tag) {
        format::choose(view, choice);
        return;
    }
    let Some(command) = Command::of_tag(tag) else { return };
    match command {
        Command::Preview => {
            if let (Some(host), Some(controller)) = (crate::host::installed(), view.controller()) {
                host.toggle_preview(controller.id(), view.ivars().serial);
            }
        }
        Command::PastePlain => paste::plain(view),
        Command::OpenInSplat => {
            if let Some(path) = view.document().path() {
                splat_app::open(view.mtm(), path);
            }
        }
        command => format::run(view, command),
    }
}

/// Whether the menu item of an `termMarkdown:` command applies to the editor `view`; `None` for other items.
pub(crate) fn validate(view: &EditorTextView, item: &NSMenuItem) -> Option<bool> {
    (item.action() == Some(sel!(termMarkdown:))).then(|| match Command::of_tag(item.tag()) {
        Some(Command::PastePlain) => view.isEditable(),
        Some(Command::OpenInSplat) => is_markdown(view) && splat_app::location().is_some(),
        Some(Command::Preview) => is_markdown(view),
        Some(_) => is_markdown(view) && view.isEditable(),
        None => false,
    })
}

/// ⌘V: in a Markdown document, rich text from Word or the web goes in as Markdown; whether the paste was taken.
pub(crate) fn paste(view: &EditorTextView) -> bool {
    is_markdown(view) && paste::rich(view, &objc2_app_kit::NSPasteboard::generalPasteboard())
}

/// Shows the format bar of `view` while it holds a Markdown document, else hides it.
pub(crate) fn refresh_bar(view: &EditorTextView) {
    let around = &view.ivars().around;
    let bar = around.format.borrow();
    let Some(bar) = bar.as_ref() else { return };
    let title = view.document().path().map(crate::display_name).unwrap_or_default();
    bar.show(is_markdown(view).then_some(view), &title);
}

/// Whether `view` shows a Markdown document.
pub(crate) fn is_markdown(view: &EditorTextView) -> bool {
    view.document().path().is_some_and(is_markdown_path)
}

/// Whether `path` names a Markdown file, by its extension.
pub(crate) fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["md", "markdown", "mdown", "mkd", "mkdn"].contains(&e.to_ascii_lowercase().as_str()))
}

/// The folder of the document `view` shows, where relative links and pictures start.
fn doc_dir(view: &EditorTextView) -> Option<PathBuf> {
    view.document().path()?.parent().map(PathBuf::from)
}

/// The text of `view` and its selection, in bytes.
fn text_and_selection(view: &EditorTextView) -> (String, Range<Byte>) {
    let document = view.document();
    let text = document.text();
    let selected = view.selectedRange();
    let byte = |utf16: usize| Byte::new(text.utf16_to_byte(utf16).unwrap_or_else(|_| text.to_string().len()));
    let range = byte(selected.location)..byte(selected.location + selected.length);
    (text.to_string(), range)
}

/// The UTF-16 offset of byte `at` of `text`.
fn utf16(text: &str, at: Byte) -> usize {
    text.get(..at.get())
        .map_or_else(|| text.encode_utf16().count(), |s| s.encode_utf16().count())
}

/// Makes `edit` in `view` like typing does, as one undo step named `name`, and selects what it says.
fn apply(view: &EditorTextView, edit: &Edit, name: &str) {
    let (text, _) = text_and_selection(view);
    let start = utf16(&text, edit.range.start);
    let range = NSRange::new(start, utf16(&text, edit.range.end) - start);
    let replacement = ns(&edit.replacement);
    let replaced = std::cell::Cell::new(false);
    crate::undo_step(view, || {
        if view.shouldChangeTextInRange_replacementString(range, Some(&replacement)) {
            if let Some(storage) = view.storage() {
                storage.replaceCharactersInRange_withString(range, &replacement);
            }
            view.didChangeText();
            replaced.set(true);
        }
    });
    if !replaced.get() {
        return;
    }
    if let Some(undo) = view.undoManager() {
        undo.setActionName(&ns(name));
    }
    let edited = edit.applied_to(&text);
    let from = utf16(&edited, edit.selection.start);
    view.setSelectedRange(NSRange::new(from, utf16(&edited, edit.selection.end) - from));
    view.scrollRangeToVisible(view.selectedRange());
}

/// What the self-test calls directly, without menus, panels or the general pasteboard.
#[cfg(feature = "selftest")]
pub(crate) mod selftest {
    pub(crate) use super::splat_app::saved_now as saved_for_splat;
}
