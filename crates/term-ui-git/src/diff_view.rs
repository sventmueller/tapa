//! The views of the diff tab: a header, a table of lines with both line numbers, a marker and the text, and hunk
//! headers with their buttons.

use std::path::Path;

use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSAccessibility, NSBezelStyle, NSBox, NSBoxType, NSButton, NSColor, NSControlSize, NSFont, NSLayoutAttribute,
    NSLineBreakMode, NSScrollView, NSStackView, NSStackViewGravity, NSTableColumn, NSTableColumnResizingOptions,
    NSTableView, NSTableViewColumnAutoresizingStyle, NSTableViewGridLineStyle, NSTableViewStyle, NSTextAlignment,
    NSTextField, NSUserInterfaceItemIdentification, NSUserInterfaceLayoutDirection, NSUserInterfaceLayoutOrientation,
    NSView, NSWorkspace,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSSize};
use splat_i18n::{t, tf};
use term_git::{Change, FileDiff, Hunk, Line, LineKind};

use super::diff::DiffTab;
use term_ui::kit::{center_in, label, ns, pin_sides, set_action};

pub(crate) const HUNK_HEIGHT: f64 = 30.0;
const NUMBER_WIDTH: f64 = 46.0;
const MARK_WIDTH: f64 = 14.0;
/// Longer lines are cut on screen, so a minified file does not make one row as wide as a city.
const MAX_SHOWN_CHARS: usize = 2000;
const TAB_WIDTH: usize = 4;

/// A row of the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Row {
    Hunk(usize),
    Line(usize, usize),
}

/// The rows of `diff`: each hunk's header, then its lines.
pub(crate) fn rows(diff: &FileDiff) -> Vec<Row> {
    let mut rows = vec![];
    for (h, hunk) in diff.hunks.iter().enumerate() {
        rows.push(Row::Hunk(h));
        rows.extend((0..hunk.lines.len()).map(|l| Row::Line(h, l)));
    }
    rows
}

/// What the tab says instead of lines, when there are none to show.
pub(crate) fn notice(diff: Option<&FileDiff>, staged: bool) -> Option<&'static str> {
    let Some(diff) = diff else {
        return Some(if staged {
            t("Von dieser Datei ist nichts mehr vorgemerkt.")
        } else {
            t("Diese Datei hat keine nicht vorgemerkten Änderungen mehr.")
        });
    };
    if diff.binary {
        Some(t("Binärdatei: Ihre Unterschiede lassen sich nicht als Text zeigen."))
    } else if diff.change == Change::Unmerged {
        Some(t("Die Datei hat Konflikte. Öffne sie im Editor, um sie zu lösen."))
    } else if diff.hunks.is_empty() {
        Some(t("Nur die Dateirechte oder der Name haben sich geändert."))
    } else {
        None
    }
}

/// The background of an added or removed line; the marker column says the same in a sign.
pub(crate) fn background(kind: LineKind) -> Option<Retained<NSColor>> {
    let strong = NSWorkspace::sharedWorkspace().accessibilityDisplayShouldIncreaseContrast();
    let alpha = if strong { 0.32 } else { 0.16 };
    match kind {
        LineKind::Added => Some(NSColor::systemGreenColor().colorWithAlphaComponent(alpha)),
        LineKind::Removed => Some(NSColor::systemRedColor().colorWithAlphaComponent(alpha)),
        LineKind::Context => None,
    }
}

pub(crate) fn marker(kind: LineKind) -> &'static str {
    match kind {
        LineKind::Added => "+",
        LineKind::Removed => "−",
        LineKind::Context => " ",
    }
}

pub(crate) fn hunk_title(hunk: &Hunk) -> String {
    let mut title = format!(
        "@@ -{},{} +{},{} @@",
        hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
    );
    if !hunk.section.is_empty() {
        title.push(' ');
        title.push_str(&hunk.section);
    }
    title
}

/// A line as the table shows it: tabs as spaces, no carriage return, cut when very long.
fn shown(line: &Line) -> String {
    let text = line.text();
    let text = text.trim_end_matches('\r');
    let mut out = String::with_capacity(text.len().min(MAX_SHOWN_CHARS));
    for c in text.chars() {
        if c == '\t' {
            out.push_str(&" ".repeat(TAB_WIDTH));
        } else {
            out.push(c);
        }
        if out.len() >= MAX_SHOWN_CHARS {
            out.push('…');
            break;
        }
    }
    out
}

/// How many characters [`shown`] makes of `line`, without building the text.
fn shown_chars(line: &Line) -> usize {
    let text = line.text();
    let text = text.trim_end_matches('\r');
    let (mut bytes, mut chars) = (0, 0);
    for c in text.chars() {
        if c == '\t' {
            bytes += TAB_WIDTH;
            chars += TAB_WIDTH;
        } else {
            bytes += c.len_utf8();
            chars += 1;
        }
        if bytes >= MAX_SHOWN_CHARS {
            return chars + 1;
        }
    }
    chars
}

pub(crate) struct Ui {
    pub(crate) table: Retained<NSTableView>,
    text_column: Retained<NSTableColumn>,
    notice: Retained<NSTextField>,
    message: Retained<NSTextField>,
    font: Retained<NSFont>,
    number_font: Retained<NSFont>,
}

impl Ui {
    pub(crate) fn new(mtm: MainThreadMarker, tab: &DiffTab, path: &Path, side: &str) -> Ui {
        let size = term_ui::kit::ui_size::scaled(term_ui::kit::style::CODE);
        let regular = term_render::constant!(objc2_app_kit::NSFontWeightRegular);
        let font = NSFont::monospacedSystemFontOfSize_weight(size, regular);
        let number_font = NSFont::monospacedDigitSystemFontOfSize_weight((size - 1.0).max(9.0), regular);
        let header = header(mtm, tab, path, side);
        let message = NSTextField::wrappingLabelWithString(&ns(""), mtm);
        message.setTextColor(Some(&NSColor::systemRedColor()));
        message.setHidden(true);
        let line = NSBox::new(mtm);
        line.setBoxType(NSBoxType::Separator);
        let (scroll, table, text_column) = table(mtm, tab, path);
        let notice = NSTextField::wrappingLabelWithString(&ns(""), mtm);
        notice.setTextColor(Some(&NSColor::secondaryLabelColor()));
        notice.setAlignment(NSTextAlignment::Center);
        notice.setPreferredMaxLayoutWidth(360.0);
        notice.setHidden(true);
        let stack = NSStackView::new(mtm);
        stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
        stack.setAlignment(NSLayoutAttribute::Leading);
        stack.setSpacing(term_ui::kit::style::space::NONE);
        stack.setEdgeInsets(NSEdgeInsets {
            top: 0.0,
            left: 0.0,
            bottom: 0.0,
            right: 0.0,
        });
        for (view, inset) in [
            (&*header as &NSView, 12.0),
            (&message, 12.0),
            (&line, 0.0),
            (&scroll, 0.0),
        ] {
            stack.addArrangedSubview(view);
            pin_sides(view, &stack, inset);
        }
        scroll.setContentHuggingPriority_forOrientation(1.0, objc2_app_kit::NSLayoutConstraintOrientation::Vertical);
        tab.addSubview(&stack);
        term_ui::kit::pin_edges(&stack, tab);
        tab.addSubview(&notice);
        center_in(&notice, &scroll);
        Ui {
            table,
            text_column,
            notice,
            message,
            font,
            number_font,
        }
    }

    pub(crate) fn line_height(&self) -> f64 {
        (self.font.ascender() - self.font.descender() + self.font.leading()).ceil() + 3.0
    }

    pub(crate) fn show_message(&self, message: Option<&str>) {
        self.message.setStringValue(&ns(message.unwrap_or("")));
        self.message.setHidden(message.is_none());
    }

    pub(crate) fn show_notice(&self, notice: Option<&str>) {
        self.notice.setStringValue(&ns(notice.unwrap_or("")));
        self.notice.setHidden(notice.is_none());
    }

    /// Makes the text column as wide as the longest line, so long lines scroll sideways instead of being cut.
    pub(crate) fn fit_text_column(&self, diff: Option<&FileDiff>) {
        let longest = diff
            .into_iter()
            .flat_map(|d| d.hunks.iter())
            .flat_map(|h| h.lines.iter())
            .map(shown_chars)
            .max()
            .unwrap_or(0);
        let chars = u32::try_from(longest).unwrap_or(u32::MAX);
        let width = f64::from(chars) * self.font.maximumAdvancement().width + 24.0;
        self.text_column.setMinWidth(width.max(200.0));
        if self.text_column.width() < width {
            self.text_column.setWidth(width);
        }
    }

    /// The cell of `column` for `line`, reusing one the table keeps.
    pub(crate) fn line_cell(&self, table: &NSTableView, column: &str, line: &Line) -> Retained<NSView> {
        let mtm = table.mtm();
        // SAFETY: The identifier names cells this function made, which are plain labels; no owner is involved.
        let reused = unsafe { table.makeViewWithIdentifier_owner(&ns(column), None) };
        let field = reused
            .and_then(|view| view.downcast::<NSTextField>().ok())
            .unwrap_or_else(|| {
                let field = label(mtm, "");
                field.setIdentifier(Some(&ns(column)));
                field.setLineBreakMode(NSLineBreakMode::ByClipping);
                field
            });
        let number = |n: Option<u32>| n.map_or_else(String::new, |n| n.to_string());
        match column {
            "old" | "new" => {
                let value = if column == "old" {
                    line.old_number
                } else {
                    line.new_number
                };
                field.setFont(Some(&self.number_font));
                field.setTextColor(Some(&NSColor::secondaryLabelColor()));
                field.setAlignment(NSTextAlignment::Right);
                field.setStringValue(&ns(&number(value)));
                term_ui::kit::set_label(&field, None);
            }
            "mark" => {
                field.setFont(Some(&self.font));
                field.setAlignment(NSTextAlignment::Center);
                let color = match line.kind {
                    LineKind::Added => NSColor::systemGreenColor(),
                    LineKind::Removed => NSColor::systemRedColor(),
                    LineKind::Context => NSColor::secondaryLabelColor(),
                };
                field.setTextColor(Some(&color));
                field.setStringValue(&ns(marker(line.kind)));
                let spoken = match line.kind {
                    LineKind::Added => t("hinzugefügt"),
                    LineKind::Removed => t("entfernt"),
                    LineKind::Context => t("unverändert"),
                };
                term_ui::kit::set_label(&field, Some(spoken));
            }
            _ => {
                field.setFont(Some(&self.font));
                field.setTextColor(Some(&NSColor::labelColor()));
                field.setAlignment(NSTextAlignment::Left);
                field.setStringValue(&ns(&shown(line)));
                term_ui::kit::set_label(&field, None);
            }
        }
        field.into_super().into_super()
    }
}

fn header(mtm: MainThreadMarker, tab: &DiffTab, path: &Path, side: &str) -> Retained<NSStackView> {
    let title = label(mtm, &path.display().to_string());
    title.setFont(Some(&NSFont::boldSystemFontOfSize(NSFont::systemFontSize())));
    title.setLineBreakMode(NSLineBreakMode::ByTruncatingHead);
    title.setContentCompressionResistancePriority_forOrientation(
        200.0,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
    let side = label(mtm, side);
    side.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let open = button(mtm, tab, t("Im Editor öffnen"), sel!(termDiffOpenFile:), -1);
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(term_ui::kit::style::space::M);
    row.setEdgeInsets(NSEdgeInsets {
        top: 8.0,
        left: 0.0,
        bottom: 8.0,
        right: 0.0,
    });
    row.addView_inGravity(&title, NSStackViewGravity::Leading);
    row.addView_inGravity(&side, NSStackViewGravity::Leading);
    row.addView_inGravity(&open, NSStackViewGravity::Trailing);
    row
}

fn table(
    mtm: MainThreadMarker,
    tab: &DiffTab,
    path: &Path,
) -> (Retained<NSScrollView>, Retained<NSTableView>, Retained<NSTableColumn>) {
    let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(600.0, 400.0));
    let table = NSTableView::initWithFrame(NSTableView::alloc(mtm), frame);
    let mut text_column = None;
    for (id, width, title) in [
        ("old", NUMBER_WIDTH, t("Alt")),
        ("new", NUMBER_WIDTH, t("Neu")),
        ("mark", MARK_WIDTH, ""),
        ("text", 600.0, t("Inhalt")),
    ] {
        let column = NSTableColumn::initWithIdentifier(NSTableColumn::alloc(mtm), &ns(id));
        column.setWidth(width);
        column.setMinWidth(width);
        column.setTitle(&ns(title));
        if id == "text" {
            column.setResizingMask(NSTableColumnResizingOptions::AutoresizingMask);
            text_column = Some(column.clone());
        } else {
            column.setResizingMask(NSTableColumnResizingOptions::NoResizing);
            column.setMaxWidth(width);
        }
        table.addTableColumn(&column);
    }
    table.setHeaderView(None);
    table.setStyle(NSTableViewStyle::Plain);
    table.setIntercellSpacing(NSSize::new(4.0, 0.0));
    table.setGridStyleMask(NSTableViewGridLineStyle::empty());
    table.setFloatsGroupRows(true);
    table.setAllowsMultipleSelection(true);
    table.setColumnAutoresizingStyle(NSTableViewColumnAutoresizingStyle::LastColumnOnlyAutoresizingStyle);
    table.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    table.setAccessibilityLabel(Some(&ns(&tf("Unterschiede in {}", &[&path.display().to_string()]))));
    // SAFETY: The table holds its data source and delegate weakly; the tab owns the table, so it outlives neither.
    unsafe {
        table.setDataSource(Some(ProtocolObject::from_ref(tab)));
        table.setDelegate(Some(ProtocolObject::from_ref(tab)));
    }
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), frame);
    scroll.setHasVerticalScroller(true);
    scroll.setHasHorizontalScroller(true);
    scroll.setAutohidesScrollers(true);
    scroll.setUserInterfaceLayoutDirection(NSUserInterfaceLayoutDirection::LeftToRight);
    scroll.setDocumentView(Some(&table));
    let text_column = text_column.expect("the loop above adds the text column");
    (scroll, table, text_column)
}

/// The header row of the hunk at `index`, with its buttons first, so they stay in view when long lines scroll.
pub(crate) fn hunk_header(
    tab: &DiffTab,
    diff: &FileDiff,
    index: usize,
    staged: bool,
    read_only: bool,
) -> Retained<NSView> {
    let mtm = tab.mtm();
    let row = NSStackView::new(mtm);
    row.setOrientation(NSUserInterfaceLayoutOrientation::Horizontal);
    row.setSpacing(term_ui::kit::style::space::S);
    row.setEdgeInsets(NSEdgeInsets {
        top: 0.0,
        left: 8.0,
        bottom: 0.0,
        right: 8.0,
    });
    let tag = isize::try_from(index).unwrap_or(-1);
    if !diff.binary && !read_only {
        if staged {
            row.addArrangedSubview(&button(
                mtm,
                tab,
                t("Abschnitt zurücknehmen"),
                sel!(termDiffUnstageHunk:),
                tag,
            ));
        } else {
            row.addArrangedSubview(&button(
                mtm,
                tab,
                t("Abschnitt vormerken"),
                sel!(termDiffStageHunk:),
                tag,
            ));
            if super::diff::can_discard(diff) {
                row.addArrangedSubview(&button(
                    mtm,
                    tab,
                    t("Abschnitt verwerfen"),
                    sel!(termDiffDiscardHunk:),
                    tag,
                ));
            }
        }
    }
    let Some(hunk) = diff.hunks.get(index) else {
        return row.into_super();
    };
    let title = label(mtm, &hunk_title(hunk));
    title.setFont(Some(&NSFont::monospacedSystemFontOfSize_weight(
        NSFont::smallSystemFontSize(),
        term_render::constant!(objc2_app_kit::NSFontWeightRegular),
    )));
    title.setTextColor(Some(&NSColor::secondaryLabelColor()));
    let spoken = tf(
        "Abschnitt {} von {}: alte Zeilen ab {}, neue Zeilen ab {}",
        &[
            &(index + 1).to_string(),
            &diff.hunks.len().to_string(),
            &hunk.old_start.to_string(),
            &hunk.new_start.to_string(),
        ],
    );
    term_ui::kit::set_label(&title, Some(&spoken));
    row.addArrangedSubview(&title);
    row.into_super()
}

fn button(mtm: MainThreadMarker, tab: &DiffTab, title: &str, action: Sel, tag: isize) -> Retained<NSButton> {
    // SAFETY: Without a target and an action the button sends nothing until `set_action` gives it both.
    let button = unsafe { NSButton::buttonWithTitle_target_action(&ns(title), None, None, mtm) };
    set_action(&button, tab, action);
    button.setBezelStyle(NSBezelStyle::Push);
    button.setControlSize(NSControlSize::Small);
    button.setFont(Some(&term_ui::kit::style::small_system()));
    button.setTag(tag);
    button
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: LineKind, text: &str) -> Line {
        Line {
            kind,
            old_number: Some(1),
            new_number: Some(1),
            content: text.as_bytes().to_vec(),
            missing_newline: false,
        }
    }

    #[test]
    fn tabs_become_spaces_and_a_carriage_return_goes() {
        assert_eq!(shown(&line(LineKind::Context, "\tx\r")), "    x");
        let long = "a".repeat(MAX_SHOWN_CHARS * 2);
        assert_eq!(
            shown(&line(LineKind::Added, &long)).chars().count(),
            MAX_SHOWN_CHARS + 1
        );
    }

    #[test]
    fn the_width_of_a_line_is_that_of_its_shown_text() {
        let long_tabs = "\t".repeat(MAX_SHOWN_CHARS);
        let long_wide = "ü".repeat(MAX_SHOWN_CHARS);
        for text in [
            "",
            "x",
            "\tx\r",
            "a\tb\tc",
            "grüße\r\r",
            long_tabs.as_str(),
            long_wide.as_str(),
        ] {
            let context = line(LineKind::Context, text);
            assert_eq!(shown_chars(&context), shown(&context).chars().count(), "{text:?}");
        }
        let long = line(LineKind::Added, &"a".repeat(MAX_SHOWN_CHARS * 2));
        assert_eq!(shown_chars(&long), shown(&long).chars().count());
    }

    #[test]
    fn each_hunk_brings_its_header_then_its_lines() {
        let hunk = |n: usize| Hunk {
            old_start: 1,
            old_count: 1,
            new_start: 1,
            new_count: 1,
            section: String::new(),
            lines: (0..n).map(|_| line(LineKind::Context, "x")).collect(),
        };
        let diff = FileDiff {
            old_path: Some("a".into()),
            new_path: Some("a".into()),
            change: Change::Modified,
            old_mode: None,
            new_mode: None,
            binary: false,
            hunks: vec![hunk(2), hunk(1)],
        };
        assert_eq!(
            rows(&diff),
            [
                Row::Hunk(0),
                Row::Line(0, 0),
                Row::Line(0, 1),
                Row::Hunk(1),
                Row::Line(1, 0)
            ]
        );
        assert_eq!(notice(Some(&diff), false), None);
        assert!(notice(None, true).is_some());
    }
}
