//! The whitespace rules of one editor: tabs or spaces and their width from the file's `.editorconfig`, else guessed
//! from its lines; how wide a tab shows; the line break of a file that has none yet; what a save tidies; and the
//! status bar's control that changes the indentation for the file.

use std::ops::Range;
use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSButton, NSColor, NSControlStateValueOff, NSControlStateValueOn, NSFontAttributeName,
    NSForegroundColorAttributeName, NSMenu, NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSStackView,
    NSStackViewGravity, NSStringDrawing, NSTextField, NSView, NSWritingDirection,
};
use objc2_foundation::{NSArray, NSDictionary, NSPoint, NSRange, NSString};
use splat_i18n::{t, tf};
use term_document::spacing::{self, Indent};
use term_document::{Encoding, LineEnding};
use term_project::editorconfig::{self, Charset, EndOfLine, IndentStyle, Properties};

use super::view::EditorTextView;
use crate::util::{constant, label, ns, set_action, tagged, with_editing};

/// The level when neither the file nor its lines say.
const DEFAULT_WIDTH: usize = 4;
/// The widths the status bar's menu offers.
const WIDTHS: [usize; 4] = [2, 3, 4, 8];
/// The menu tags of the two styles; a width's tag is `WIDTH_TAG` plus the width.
const TABS_TAG: isize = 1;
const SPACES_TAG: isize = 2;
const WIDTH_TAG: isize = 100;

/// How one editor indents and what its saves tidy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) tabs: bool,
    /// The columns of a level indented with spaces.
    pub(crate) size: usize,
    /// The columns a tab shows as.
    pub(crate) tab_width: usize,
    /// The break new lines get in a file that had none when it was opened.
    pub(crate) newline: Option<LineEnding>,
    pub(crate) trim_trailing: bool,
    pub(crate) final_newline: bool,
    /// The charset `.editorconfig` asks for when the file has another.
    pub(crate) other_charset: Option<Charset>,
}

impl Default for Format {
    fn default() -> Format {
        Format {
            tabs: false,
            size: DEFAULT_WIDTH,
            tab_width: DEFAULT_WIDTH,
            newline: None,
            trim_trailing: false,
            final_newline: false,
            other_charset: None,
        }
    }
}

impl Format {
    /// The format of a file with `properties` from its `.editorconfig`, the lines `text`, read as `encoding` with or
    /// without a byte order mark.
    pub(crate) fn of(properties: &Properties, text: &str, encoding: Option<(Encoding, bool)>) -> Format {
        let detected = spacing::detect_indent(text);
        let tabs = match properties.indent_style {
            Some(style) => style == IndentStyle::Tab,
            None => detected == Some(Indent::Tabs),
        };
        let tab_width = properties.tab_columns().unwrap_or(DEFAULT_WIDTH);
        let size = properties
            .indent_columns()
            .or(match detected {
                Some(Indent::Spaces(n)) => Some(n),
                _ => None,
            })
            .unwrap_or(if tabs { tab_width } else { DEFAULT_WIDTH });
        let has_breaks = text.contains(['\n', '\r']);
        Format {
            tabs,
            size,
            tab_width,
            newline: properties.end_of_line.filter(|_| !has_breaks).map(ending),
            trim_trailing: properties.trim_trailing_whitespace == Some(true),
            final_newline: properties.insert_final_newline == Some(true),
            other_charset: properties
                .charset
                .filter(|&charset| encoding.is_some_and(|e| !fits(charset, e))),
        }
    }

    /// One level of indentation.
    pub(crate) fn unit(&self) -> String {
        if self.tabs { "\t".into() } else { " ".repeat(self.size) }
    }

    /// What the status bar shows.
    pub(crate) fn title(&self) -> String {
        if self.tabs {
            tf("Tabs: {}", &[&self.tab_width.to_string()])
        } else {
            tf("Leerzeichen: {}", &[&self.size.to_string()])
        }
    }
}

fn ending(end: EndOfLine) -> LineEnding {
    match end {
        EndOfLine::Lf => LineEnding::Lf,
        EndOfLine::Crlf => LineEnding::Crlf,
        EndOfLine::Cr => LineEnding::Cr,
    }
}

/// Whether a file read as `encoding`, with a byte order mark or not, is in `charset`.
fn fits(charset: Charset, (encoding, bom): (Encoding, bool)) -> bool {
    match charset {
        Charset::Utf8 => encoding == Encoding::Utf8 && !bom,
        Charset::Utf8Bom => encoding == Encoding::Utf8 && bom,
        Charset::Utf16Le => encoding == Encoding::Utf16Le,
        Charset::Utf16Be => encoding == Encoding::Utf16Be,
        Charset::Latin1 => false,
    }
}

/// The edits that tidy `text` for a save with `format`: whitespace at line ends, a missing last line break, which
/// is `last`, and every break made `convert` for a file saved the first time.
pub(crate) fn tidy_edits(
    text: &str,
    format: &Format,
    convert: Option<LineEnding>,
    last: LineEnding,
) -> Vec<(Range<usize>, String)> {
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    if format.trim_trailing {
        edits.extend(
            spacing::trailing_whitespace(text)
                .into_iter()
                .map(|r| (r, String::new())),
        );
    }
    if let Some(wanted) = convert {
        edits.extend(
            spacing::other_breaks(text, wanted)
                .into_iter()
                .map(|r| (r, wanted.as_str().to_string())),
        );
    }
    if format.final_newline && spacing::lacks_final_newline(text) {
        let end = text.encode_utf16().count();
        edits.push((end..end, last.as_str().to_string()));
    }
    edits
}

/// The state of one editor: its format and the status bar's controls.
#[derive(Default)]
pub(crate) struct State {
    pub(crate) format: Format,
    button: Option<Retained<NSButton>>,
    warning: Option<Retained<NSTextField>>,
}

/// Reads the `.editorconfig` of `view`'s file and its lines, and sets the indentation and tab width from them.
pub(crate) fn configure(view: &EditorTextView) {
    let format = {
        let document = view.document();
        let properties = document.path().map(editorconfig::for_file).unwrap_or_default();
        let encoding = document
            .path()
            .map(|_| (document.format().encoding, document.format().bom));
        Format::of(&properties, &document.text().to_string(), encoding)
    };
    apply(view, format);
}

fn apply(view: &EditorTextView, format: Format) {
    format.unit().clone_into(&mut view.ivars().indent.borrow_mut());
    set_tab_width(view, format.tab_width);
    view.ivars().spacing.borrow_mut().format = format;
    refresh(view);
}

/// Shows tabs `columns` spaces wide, in the text there is and the text typed.
fn set_tab_width(view: &EditorTextView, columns: usize) {
    let font = super::code_font(view.ivars().font_size.get());
    let attributes = NSDictionary::<NSString, AnyObject>::from_retained_objects(
        &[constant!(NSFontAttributeName)],
        &[font.clone().into()],
    );
    let sample = ns(&" ".repeat(columns.max(1)));
    // SAFETY: The attributes hold a font, as the method expects.
    let interval = unsafe { sample.sizeWithAttributes(Some(&attributes)) }.width;
    let style = NSMutableParagraphStyle::new();
    style.setTabStops(Some(&NSArray::new()));
    style.setDefaultTabInterval(interval);
    // Code runs left to right also in a right-to-left interface (E3).
    style.setBaseWritingDirection(NSWritingDirection::LeftToRight);
    view.setDefaultParagraphStyle(Some(&style));
    let key = constant!(NSParagraphStyleAttributeName);
    let typing = NSDictionary::<NSString, AnyObject>::from_retained_objects(
        &[
            constant!(NSFontAttributeName),
            constant!(NSForegroundColorAttributeName),
            key,
        ],
        &[font.into(), NSColor::textColor().into(), style.clone().into()],
    );
    // SAFETY: The dictionary holds a font, a colour and a paragraph style under their keys, as typing attributes
    // expect.
    unsafe { view.setTypingAttributes(&typing) };
    if let Some(storage) = view.storage() {
        let whole = NSRange::new(0, storage.length());
        with_editing(&storage, || {
            // SAFETY: The key is AppKit's paragraph style key and the value a paragraph style.
            unsafe { storage.addAttribute_value_range(key, &style, whole) };
        });
    }
}

/// The controls this module adds to the status bar of `view`: a warning about the charset and the indentation menu.
pub(crate) fn status_items(mtm: MainThreadMarker, view: &EditorTextView, stack: &NSStackView) {
    let warning = label(mtm, "");
    term_ui::kit::ui_size::set_system_font(&warning, term_ui::kit::style::SMALL);
    warning.setTextColor(Some(&NSColor::systemOrangeColor()));
    warning.setHidden(true);
    // SAFETY: A nil target until `set_action` below, which checks that the view answers the action.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(""), None, None, mtm) };
    button.setBordered(false);
    term_ui::kit::ui_size::set_system_font(&button, term_ui::kit::style::SMALL);
    button.setContentTintColor(Some(&NSColor::secondaryLabelColor()));
    button.setToolTip(Some(&ns(t("Einrückung für diese Datei ändern"))));
    set_action(&button, view, sel!(termIndentMenu:));
    stack.addView_inGravity(&warning, NSStackViewGravity::Trailing);
    stack.addView_inGravity(&button, NSStackViewGravity::Trailing);
    let mut state = view.ivars().spacing.borrow_mut();
    state.button = Some(button);
    state.warning = Some(warning);
    drop(state);
    refresh(view);
}

/// Shows the format in the status bar.
fn refresh(view: &EditorTextView) {
    let state = view.ivars().spacing.borrow();
    if let Some(button) = &state.button {
        button.setTitle(&ns(&state.format.title()));
    }
    if let Some(warning) = &state.warning {
        match state.format.other_charset {
            Some(charset) => {
                warning.setStringValue(&ns(&tf("Zeichensatz laut .editorconfig: {}", &[charset.name()])));
                warning.setToolTip(Some(&ns(t(
                    "Die Datei ist anders codiert, als .editorconfig verlangt. Sichern behält ihre Codierung.",
                ))));
                warning.setHidden(false);
            }
            None => warning.setHidden(true),
        }
    }
}

/// Opens the indentation menu below `sender`, the status bar's button.
pub(crate) fn show_menu(view: &EditorTextView, sender: Option<&AnyObject>) {
    let Some(button) = sender.and_then(|s| s.downcast_ref::<NSButton>()) else {
        return;
    };
    let menu = menu(view);
    let below = NSPoint::new(0.0, button.bounds().size.height + 4.0);
    let view_of_button: &NSView = button;
    menu.popUpMenuPositioningItem_atLocation_inView(None, below, Some(view_of_button));
}

/// The indentation menu of `view`: tabs or spaces and the widths, the current ones ticked.
pub(crate) fn menu(view: &EditorTextView) -> Retained<NSMenu> {
    let mtm = view.mtm();
    let format = view.ivars().spacing.borrow().format.clone();
    let menu = NSMenu::new(mtm);
    menu.setAutoenablesItems(false);
    let action = sel!(termSetIndent:);
    let mut items = vec![
        (tagged(mtm, t("Mit Tabs einrücken"), action, "", TABS_TAG), format.tabs),
        (
            tagged(mtm, t("Mit Leerzeichen einrücken"), action, "", SPACES_TAG),
            !format.tabs,
        ),
    ];
    let current = if format.tabs { format.tab_width } else { format.size };
    for width in WIDTHS {
        let title = tf("Breite: {}", &[&width.to_string()]);
        let tag = WIDTH_TAG + isize::try_from(width).unwrap_or(0);
        items.push((tagged(mtm, &title, action, "", tag), current == width));
    }
    for (index, (item, on)) in items.iter().enumerate() {
        if index == 2 {
            menu.addItem(&objc2_app_kit::NSMenuItem::separatorItem(mtm));
        }
        item.setState(if *on {
            NSControlStateValueOn
        } else {
            NSControlStateValueOff
        });
        crate::util::set_target(item, view);
        menu.addItem(item);
    }
    menu
}

/// The user chose an entry of the indentation menu, by its tag.
pub(crate) fn choose(view: &EditorTextView, tag: isize) {
    let mut format = view.ivars().spacing.borrow().format.clone();
    match tag {
        TABS_TAG => format.tabs = true,
        SPACES_TAG => format.tabs = false,
        _ => {
            let Ok(width) = usize::try_from(tag - WIDTH_TAG) else {
                return;
            };
            if !WIDTHS.contains(&width) {
                return;
            }
            if format.tabs {
                format.tab_width = width;
            } else {
                format.size = width;
            }
        }
    }
    apply(view, format);
}

/// Tidies `view`'s text before it is saved, to `target` when that is a new place for it, as one undoable step.
pub(crate) fn before_save(view: &EditorTextView, target: Option<&Path>) {
    let (format, convert) = match target {
        Some(target) => {
            let properties = editorconfig::for_file(target);
            let first_save = view.document().path().is_none();
            let convert = properties.end_of_line.map(ending).filter(|_| first_save);
            (Format::of(&properties, "", None), convert)
        }
        None => (view.ivars().spacing.borrow().format.clone(), None),
    };
    let edits = {
        let document = view.document();
        let last = convert.or(format.newline).unwrap_or(document.line_endings().dominant);
        tidy_edits(&document.text().to_string(), &format, convert, last)
    };
    if !edits.is_empty() {
        crate::text_view::apply_text_edits(view, edits);
    }
}

/// The break a new line gets in `view`: the one `.editorconfig` asks for in a file that had none, else the one most
/// lines have.
pub(crate) fn newline(view: &EditorTextView) -> LineEnding {
    let chosen = view.ivars().spacing.borrow().format.newline;
    chosen.unwrap_or_else(|| view.document().line_endings().dominant)
}

/// Indents `lines` of `view`, counted from 0, by `levels` of its own indentation, or takes as many away where negative,
/// as one undo step; for Vim's `>>`, `<<` and `>` (E106).
pub(crate) fn shift_lines(view: &EditorTextView, lines: std::ops::Range<usize>, levels: isize) {
    let unit = view.ivars().indent.borrow().clone();
    let width = unit.chars().count().max(1);
    let mut changes = Vec::new();
    for line in lines.rev() {
        let Some(start) = view.line_start(line) else { continue };
        let text = view
            .document()
            .text()
            .line(line)
            .map(|l| l.to_string())
            .unwrap_or_default();
        if text.trim().is_empty() {
            continue;
        }
        let edit = if levels > 0 {
            term_document::Edit {
                range_utf16: start..start,
                text: unit.repeat(levels.unsigned_abs()),
            }
        } else {
            let wanted = levels.unsigned_abs() * width;
            let mut cut = 0;
            let mut seen = 0;
            for c in text.chars() {
                if seen >= wanted || !(c == ' ' || c == '\t') {
                    break;
                }
                seen += if c == '\t' { width } else { 1 };
                cut += 1;
            }
            term_document::Edit {
                range_utf16: start..start + cut,
                text: String::new(),
            }
        };
        match view.document_mut().apply(edit) {
            Ok(change) => changes.push(change),
            Err(error) => term_base::diag::report("indenting for Vim", error),
        }
    }
    for change in &changes {
        view.apply_vim_change(change);
    }
}

/// Indents `lines` of `view`, counted from 0, anew by the rule Return indents a new line with, as one undo step; for
/// Vim's `=`.
pub(crate) fn reindent_lines(view: &EditorTextView, lines: std::ops::Range<usize>) {
    let unit = view.ivars().indent.borrow().clone();
    let text = view.document().text().to_string();
    let Some(start) = view.line_start(lines.start) else {
        return;
    };
    let end = view
        .line_start(lines.end)
        .unwrap_or_else(|| view.document().text().len_utf16());
    let term_document::typing::Step::Replace { range, text: indented } =
        term_document::typing::reindent(&text, start..end, &unit)
    else {
        return;
    };
    let before = text.encode_utf16().skip(range.start).take(range.len());
    if before.eq(indented.encode_utf16()) {
        return;
    }
    let edit = term_document::Edit {
        range_utf16: range,
        text: indented,
    };
    let applied = view.document_mut().apply(edit);
    match applied {
        Ok(change) => view.apply_vim_change(&change),
        Err(error) => term_base::diag::report("indenting anew for Vim", error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_project::editorconfig::IndentSize;

    #[test]
    fn a_charset_fits_only_the_encoding_and_mark_it_names() {
        let cases = [
            (Charset::Utf8, (Encoding::Utf8, false), true),
            (Charset::Utf8, (Encoding::Utf8, true), false),
            (Charset::Utf8Bom, (Encoding::Utf8, true), true),
            (Charset::Utf8Bom, (Encoding::Utf8, false), false),
            (Charset::Utf16Le, (Encoding::Utf16Le, true), true),
            (Charset::Utf16Be, (Encoding::Utf16Le, true), false),
            (Charset::Utf16Be, (Encoding::Utf16Be, false), true),
            (Charset::Latin1, (Encoding::Utf8, false), false),
        ];
        for (charset, read, wanted) in cases {
            assert_eq!(fits(charset, read), wanted, "{charset:?} for {read:?}");
        }
        assert_eq!(
            [EndOfLine::Lf, EndOfLine::Crlf, EndOfLine::Cr].map(ending),
            [LineEnding::Lf, LineEnding::Crlf, LineEnding::Cr]
        );
    }

    #[test]
    fn editorconfig_wins_over_the_lines_and_the_lines_over_the_default() {
        let tabbed = "a\n\tb\n";
        let none = Format::of(&Properties::default(), tabbed, None);
        assert!(none.tabs);
        assert_eq!(none.title(), tf("Tabs: {}", &["4"]));
        let spaces = Properties {
            indent_style: Some(IndentStyle::Space),
            indent_size: Some(IndentSize::Columns(3)),
            ..Properties::default()
        };
        let configured = Format::of(&spaces, tabbed, None);
        assert!(!configured.tabs);
        assert_eq!(configured.unit(), "   ");
        assert_eq!(Format::of(&Properties::default(), "a\n  b\n", None).size, 2);
        assert_eq!(Format::of(&Properties::default(), "plain\n", None), Format::default());
    }

    #[test]
    fn a_line_break_is_chosen_only_for_a_file_without_one() {
        let crlf = Properties {
            end_of_line: Some(EndOfLine::Crlf),
            ..Properties::default()
        };
        assert_eq!(Format::of(&crlf, "", None).newline, Some(LineEnding::Crlf));
        assert_eq!(Format::of(&crlf, "a\nb\n", None).newline, None);
    }

    #[test]
    fn a_charset_is_reported_only_when_the_file_has_another() {
        let bom = Properties {
            charset: Some(Charset::Utf8Bom),
            ..Properties::default()
        };
        assert_eq!(
            Format::of(&bom, "", Some((Encoding::Utf8, false))).other_charset,
            Some(Charset::Utf8Bom)
        );
        assert_eq!(Format::of(&bom, "", Some((Encoding::Utf8, true))).other_charset, None);
        assert_eq!(
            Format::of(&bom, "", None).other_charset,
            None,
            "a new file has no charset yet"
        );
    }

    #[test]
    fn a_save_trims_line_ends_and_adds_the_last_break() {
        let format = Format {
            trim_trailing: true,
            final_newline: true,
            ..Format::default()
        };
        let edits = tidy_edits("a  \nb\t", &format, None, LineEnding::Crlf);
        assert_eq!(
            edits,
            vec![(1..3, String::new()), (5..6, String::new()), (6..6, "\r\n".to_string())]
        );
        let converted = tidy_edits("a\nb", &Format::default(), Some(LineEnding::Crlf), LineEnding::Crlf);
        assert_eq!(converted, vec![(1..2, "\r\n".to_string())]);
        assert!(tidy_edits("a  \n", &Format::default(), None, LineEnding::Lf).is_empty());
    }
}

#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;
