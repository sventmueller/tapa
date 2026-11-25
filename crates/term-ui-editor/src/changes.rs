//! Change marks in the line number column: where the text differs from its file in the last commit (E11), read with
//! splat-git, which starts no process and so works in folders the user does not trust (E22). A click on a mark shows
//! the committed lines in a popover that can put them back.

use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dispatch2::MainThreadBound;
use objc2::rc::{Retained, Weak};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, sel};
use objc2_app_kit::{
    NSColor, NSFont, NSLineBreakMode, NSPopover, NSPopoverBehavior, NSScrollView, NSStackView, NSTextField,
    NSUserInterfaceLayoutOrientation, NSView, NSViewController,
};
use objc2_foundation::{NSEdgeInsets, NSPoint, NSRect, NSRectEdge, NSSize};
use splat_git::Oid;
use splat_git::diff::{ChangeKind, Hunk};
use splat_i18n::t;
use term_document::Snapshot;

use super::view::EditorTextView;
use crate::util::{fixed_size, label, main_soon, ns, pin_inset, set_action};
#[cfg(feature = "selftest")]
mod selftest_probe;
#[cfg(feature = "selftest")]
pub(crate) use selftest_probe::*;

/// How long typing pauses before the marks follow it.
const DELAY: f64 = 0.3;
/// How long the repository settles after a change in `.git` before its commit is read again.
const HEAD_DELAY: f64 = 0.4;
/// The widest and tallest the committed lines grow in the popover before they scroll.
const MAX_WIDTH: f64 = 560.0;
const MAX_HEIGHT: f64 = 260.0;
const PADDING: NSEdgeInsets = NSEdgeInsets {
    top: 10.0,
    left: 12.0,
    bottom: 10.0,
    right: 12.0,
};

/// The file in the last commit, as the marks compare against it.
#[derive(Clone)]
struct Head {
    commit: Oid,
    text: Arc<str>,
}

/// What an editor knows about its file's last commit and where its text differs.
#[derive(Default)]
pub(crate) struct Marks {
    head: Option<Head>,
    hunks: Vec<Hunk>,
    /// The lines of the text the hunks were found in, to place a removal behind the last line.
    lines: usize,
    /// The document's revision the hunks were found in.
    revision: Option<term_document::Revision>,
    /// Counts questions, so that the answer to an older one is dropped.
    asked: u64,
    running: Option<Request>,
    pending: Option<Request>,
    popover: Option<Retained<NSPopover>>,
    /// The hunk the popover shows, with the document revision it was found in.
    shown: Option<(Hunk, term_document::Revision)>,
}

/// How a line is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    Added,
    Modified,
    /// Lines were removed in front of this one.
    RemovedAbove,
    /// Lines were removed behind this one, the last.
    RemovedBelow,
}

impl Mark {
    pub(crate) fn color(self) -> Retained<NSColor> {
        match self {
            Mark::Added => NSColor::systemGreenColor(),
            Mark::Modified => NSColor::systemBlueColor(),
            Mark::RemovedAbove | Mark::RemovedBelow => NSColor::systemRedColor(),
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Mark::Added => t("Seit dem letzten Commit hinzugefügt"),
            Mark::Modified => t("Seit dem letzten Commit geändert"),
            Mark::RemovedAbove | Mark::RemovedBelow => t("Seit dem letzten Commit gelöscht"),
        }
    }
}

/// The text in the commit, as the document would read it, or `None` for a file that is not UTF-8 or UTF-16 text.
fn decode(bytes: Vec<u8>) -> Option<String> {
    let (_, decoded) = term_document::Encoding::read(bytes)?;
    Some(term_document::FileFormat::of(&decoded).1.to_string())
}

/// The file's repository, commit and committed text; `None` outside a repository, before the first commit, for a file
/// the commit does not hold, and for a repository this reader cannot read.
fn read_head(path: &Path, known: Option<Head>) -> Option<Head> {
    let repository = splat_git::find(path)?;
    let commit = repository.head_commit().ok()??;
    if let Some(known) = known.filter(|known| known.commit == commit) {
        return Some(known);
    }
    let bytes = repository.head_version(path).ok()??;
    let text: Arc<str> = decode(bytes)?.into();
    Some(Head { commit, text })
}

fn hunks_of(head: &str, snapshot: &Snapshot) -> (Vec<Hunk>, usize, term_document::Revision) {
    let text = snapshot.text().to_string();
    let lines = text.split_inclusive('\n').count();
    (splat_git::diff::hunks(head, &text), lines, snapshot.revision())
}

/// Reads the file's last commit again and compares; `force` reads it even when the commit is the one already known.
pub(crate) fn reload(view: &EditorTextView, force: bool) {
    if view.document().path().is_some() {
        request(view, if force { Request::Force } else { Request::Reload });
    } else {
        clear(view);
    }
}

/// Compares the text with the commit once typing pauses.
pub(crate) fn schedule(view: &EditorTextView) {
    if view.ivars().marks.borrow().head.is_some() {
        view.later(sel!(termGitMarks), DELAY);
    }
}

/// Something in a `.git` folder changed: the commit may have moved.
pub(crate) fn schedule_head(view: &EditorTextView) {
    view.later(sel!(termGitHead), HEAD_DELAY);
}

/// Compares the text as it is now with the commit known.
pub(crate) fn compare(view: &EditorTextView) {
    request(view, Request::Compare);
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Request {
    Compare,
    Reload,
    Force,
}

fn request(view: &EditorTextView, request: Request) {
    let mut marks = view.ivars().marks.borrow_mut();
    marks.asked += 1;
    // An outstanding HEAD read can carry information a plain comparison still needs.
    marks.pending = Some(
        request
            .max(marks.running.unwrap_or(request))
            .max(marks.pending.unwrap_or(request)),
    );
    drop(marks);
    start(view);
}

fn start(view: &EditorTextView) {
    let (request, asked, head) = {
        let mut marks = view.ivars().marks.borrow_mut();
        if marks.running.is_some() {
            return;
        }
        let Some(request) = marks.pending.take() else { return };
        marks.running = Some(request);
        (request, marks.asked, marks.head.clone())
    };
    let path = view.document().path().map(Path::to_path_buf);
    let snapshot = view.document().snapshot();
    let editor = MainThreadBound::new(Weak::from(view), view.mtm());
    let queued = term_base::lane::WORK.ask(
        move || {
            let head = match request {
                Request::Compare => head,
                Request::Reload => path.and_then(|path| read_head(&path, head)),
                Request::Force => path.and_then(|path| read_head(&path, None)),
            }?;
            let (hunks, lines, revision) = hunks_of(&head.text, &snapshot);
            Some((head, hunks, lines, revision))
        },
        move |answer| {
            main_soon(move || {
                if let Some(view) = MainThreadMarker::new().and_then(|mtm| editor.get(mtm).load())
                    && view.window().is_some()
                {
                    deliver(&view, asked, answer);
                }
            })
        },
    );
    if queued.is_err() {
        let mut marks = view.ivars().marks.borrow_mut();
        marks.running = None;
        marks.pending = Some(request);
        let editor = MainThreadBound::new(Weak::from(view), view.mtm());
        crate::util::main_after(Duration::from_millis(50), move || {
            if let Some(view) = MainThreadMarker::new().and_then(|mtm| editor.get(mtm).load())
                && view.window().is_some()
            {
                start(&view);
            }
        });
    }
}

type Diff = (Head, Vec<Hunk>, usize, term_document::Revision);

fn deliver(view: &EditorTextView, asked: u64, answer: Option<Option<Diff>>) {
    let changed = {
        let mut marks = view.ivars().marks.borrow_mut();
        marks.running = None;
        if let Some(answer) = answer.filter(|_| marks.asked == asked) {
            if let Some((head, hunks, lines, revision)) = answer {
                marks.head = Some(head);
                marks.hunks = hunks;
                marks.lines = lines;
                marks.revision = Some(revision);
            } else {
                marks.head = None;
                marks.hunks.clear();
                marks.revision = None;
            }
            true
        } else {
            false
        }
    };
    if changed {
        super::redraw_gutter(view);
    }
    start(view);
}

fn clear(view: &EditorTextView) {
    {
        let mut marks = view.ivars().marks.borrow_mut();
        marks.asked += 1;
        marks.pending = None;
        marks.head = None;
        marks.hunks.clear();
    }
    super::redraw_gutter(view);
}

/// The hunk that `line`, counted from 0, belongs to, with how it is marked there.
fn hunk_at(hunks: &[Hunk], lines: usize, line: usize) -> Option<(&Hunk, Mark)> {
    let after = hunks.partition_point(|h| h.change.lines.start <= line);
    let covering = hunks[..after].iter().rev().find(|h| h.change.lines.contains(&line));
    if let Some(hunk) = covering {
        let mark = match hunk.change.kind {
            ChangeKind::Added => Mark::Added,
            _ => Mark::Modified,
        };
        return Some((hunk, mark));
    }
    let removed = |at: usize| {
        hunks[hunks.partition_point(|h| h.change.lines.start < at)..]
            .iter()
            .take_while(|h| h.change.lines.start == at)
            .find(|h| h.change.kind == ChangeKind::Removed)
    };
    if let Some(hunk) = removed(line) {
        return Some((hunk, Mark::RemovedAbove));
    }
    removed(line + 1)
        .filter(|_| line + 1 >= lines)
        .map(|hunk| (hunk, Mark::RemovedBelow))
}

/// How `line`, counted from 0, is marked.
pub(crate) fn mark_at(view: &EditorTextView, line: usize) -> Option<Mark> {
    let marks = view.ivars().marks.borrow();
    hunk_at(&marks.hunks, marks.lines, line).map(|(_, mark)| mark)
}

/// Draws `mark` at the right edge of `row` in the line number column, which is `width` wide.
pub(crate) fn draw(mark: Mark, row: NSRect, width: f64) {
    mark.color().setFill();
    let path = objc2_app_kit::NSBezierPath::bezierPath();
    match mark {
        Mark::Added | Mark::Modified => {
            let bar = NSRect::new(
                NSPoint::new(width - 4.0, row.origin.y),
                NSSize::new(3.0, row.size.height),
            );
            objc2_app_kit::NSRectFill(bar);
            return;
        }
        Mark::RemovedAbove | Mark::RemovedBelow => {
            let y = if mark == Mark::RemovedAbove {
                row.origin.y
            } else {
                row.origin.y + row.size.height
            };
            path.moveToPoint(NSPoint::new(width - 7.0, y - 4.0));
            path.lineToPoint(NSPoint::new(width - 1.0, y));
            path.lineToPoint(NSPoint::new(width - 7.0, y + 4.0));
            path.closePath();
        }
    }
    path.fill();
}

/// The column was clicked on `line`, counted from 0, at `at` in `column`: shows the committed lines of its hunk.
pub(crate) fn clicked(view: &EditorTextView, line: usize, column: &NSView, at: NSRect) -> bool {
    let Some(head) = view.ivars().marks.borrow().head.as_ref().map(|h| h.text.clone()) else {
        return false;
    };
    let (hunks, lines, revision) = {
        let document = view.document();
        let text = document.text().to_string();
        let lines = text.split_inclusive('\n').count();
        (splat_git::diff::hunks(&head, &text), lines, document.revision())
    };
    let Some((hunk, mark)) = hunk_at(&hunks, lines, line).map(|(h, m)| (h.clone(), m)) else {
        return false;
    };
    let committed = committed_lines(&head, &hunk.old_lines);
    show(view, mark, &committed, column, at);
    let mut marks = view.ivars().marks.borrow_mut();
    marks.shown = Some((hunk, revision));
    marks.hunks = hunks;
    marks.lines = lines;
    true
}

fn committed_lines(head: &str, lines: &Range<usize>) -> String {
    head.split_inclusive('\n')
        .skip(lines.start)
        .take(lines.end - lines.start)
        .collect()
}

fn show(view: &EditorTextView, mark: Mark, committed: &str, column: &NSView, at: NSRect) {
    dismiss(view);
    let mtm = view.mtm();
    let content = content(mtm, view, mark, committed);
    let size = content.fittingSize();
    let controller = NSViewController::new(mtm);
    controller.setView(&content);
    let popover = NSPopover::new(mtm);
    popover.setBehavior(NSPopoverBehavior::Transient);
    popover.setAnimates(false);
    popover.setContentViewController(Some(&controller));
    popover.setContentSize(size);
    popover.showRelativeToRect_ofView_preferredEdge(at, column, NSRectEdge::MaxX);
    view.ivars().marks.borrow_mut().popover = Some(popover);
}

/// What the popover holds: what happened, the committed lines and the button that puts them back.
fn content(mtm: MainThreadMarker, view: &EditorTextView, mark: Mark, committed: &str) -> Retained<NSView> {
    let title = label(mtm, mark.description());
    title.setFont(Some(&NSFont::boldSystemFontOfSize(term_ui::kit::style::LABEL)));
    let stack = NSStackView::new(mtm);
    stack.setOrientation(NSUserInterfaceLayoutOrientation::Vertical);
    stack.setAlignment(objc2_app_kit::NSLayoutAttribute::Leading);
    stack.setSpacing(term_ui::kit::style::space::M);
    stack.addArrangedSubview(&title);
    if mark != Mark::Added {
        stack.addArrangedSubview(&committed_view(mtm, view, committed));
    }
    // SAFETY: A nil target until `set_action` below, which checks that the view answers the action.
    let button =
        unsafe { objc2_app_kit::NSButton::buttonWithTitle_target_action(&ns(t("Zurücksetzen")), None, None, mtm) };
    button.setToolTip(Some(&ns(t(
        "Diesen Abschnitt auf den Stand des letzten Commits zurücksetzen",
    ))));
    set_action(&button, view, sel!(termRevertHunk:));
    stack.addArrangedSubview(&button);
    let container = NSView::new(mtm);
    container.addSubview(&stack);
    pin_inset(&stack, &container, PADDING);
    container
}

/// The committed lines in the code font, scrolling when there are many.
fn committed_view(mtm: MainThreadMarker, view: &EditorTextView, committed: &str) -> Retained<NSView> {
    let shown = committed.strip_suffix('\n').unwrap_or(committed);
    let shown = shown.strip_suffix('\r').unwrap_or(shown);
    let field = NSTextField::wrappingLabelWithString(&ns(shown), mtm);
    field.setFont(Some(&super::code_font(view.ivars().font_size.get() * 0.92)));
    field.setTextColor(Some(&NSColor::secondaryLabelColor()));
    field.setLineBreakMode(NSLineBreakMode::ByClipping);
    field.setSelectable(true);
    crate::util::set_label(&field, Some(t("Stand im letzten Commit")));
    let unbounded = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1.0e5, 1.0e5));
    let fitted = field.cell().map_or(NSSize::new(MAX_WIDTH, 40.0), |cell| {
        let size = cell.cellSizeForBounds(unbounded);
        NSSize::new(size.width.ceil(), size.height.ceil())
    });
    let size = NSSize::new(fitted.width.min(MAX_WIDTH).ceil(), fitted.height.min(MAX_HEIGHT).ceil());
    field.setFrame(NSRect::new(NSPoint::new(0.0, 0.0), fitted));
    let scroll = NSScrollView::initWithFrame(NSScrollView::alloc(mtm), NSRect::new(NSPoint::new(0.0, 0.0), size));
    scroll.setHasVerticalScroller(fitted.height > MAX_HEIGHT);
    scroll.setHasHorizontalScroller(fitted.width > MAX_WIDTH);
    scroll.setAutohidesScrollers(true);
    scroll.setDrawsBackground(false);
    scroll.setDocumentView(Some(&field));
    scroll.setTranslatesAutoresizingMaskIntoConstraints(false);
    fixed_size(&scroll, size.width, size.height);
    Retained::into_super(scroll)
}

/// Closes the popover of `view`, if one is shown.
pub(crate) fn dismiss(view: &EditorTextView) {
    let popover = view.ivars().marks.borrow_mut().popover.take();
    if let Some(popover) = popover {
        popover.close();
    }
}

/// Puts the committed lines of the hunk the popover shows back, as one undoable step.
pub(crate) fn revert(view: &EditorTextView) {
    let shown = view.ivars().marks.borrow_mut().shown.take();
    dismiss(view);
    let Some((hunk, revision)) = shown else { return };
    let Some(head) = view.ivars().marks.borrow().head.as_ref().map(|h| h.text.clone()) else {
        return;
    };
    let edit = {
        let document = view.document();
        if document.revision() != revision {
            return;
        }
        let text = document.text();
        let (Ok(start), Ok(end)) = (
            text.byte_to_utf16(hunk.change.bytes.start),
            text.byte_to_utf16(hunk.change.bytes.end),
        ) else {
            return;
        };
        let ending = document.line_endings().dominant.as_str();
        (
            start..end,
            with_breaks(&committed_lines(&head, &hunk.old_lines), ending),
        )
    };
    crate::text_view::apply_text_edits(view, vec![edit]);
    compare(view);
}

/// `text` with every line break made `ending`, so that a hunk from a commit with other breaks fits the file.
fn with_breaks(text: &str, ending: &str) -> String {
    text.split_inclusive('\n')
        .map(|line| match line.strip_suffix('\n') {
            Some(content) => format!("{}{ending}", content.strip_suffix('\r').unwrap_or(content)),
            None => line.to_string(),
        })
        .collect()
}

/// The editors whose file lies below `root` look at their commit again.
pub(crate) fn git_changed(root: &Path) {
    for view in super::open_editors() {
        if view.document().path().is_some_and(|p| p.starts_with(root)) {
            schedule_head(&view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hunks(old: &str, new: &str) -> (Vec<Hunk>, usize) {
        (splat_git::diff::hunks(old, new), new.split_inclusive('\n').count())
    }

    #[test]
    fn lines_are_marked_by_the_hunk_they_belong_to() {
        let (found, lines) = hunks("a\nb\nc\nd\n", "a\nB\nc\nnew\nd\n");
        assert_eq!(hunk_at(&found, lines, 1).map(|(_, m)| m), Some(Mark::Modified));
        assert_eq!(hunk_at(&found, lines, 3).map(|(_, m)| m), Some(Mark::Added));
        assert_eq!(hunk_at(&found, lines, 0), None);
        let (found, lines) = hunks("a\nb\nc\n", "a\nc\n");
        assert_eq!(hunk_at(&found, lines, 1).map(|(_, m)| m), Some(Mark::RemovedAbove));
        let (found, lines) = hunks("a\nb\n", "a\n");
        assert_eq!(
            hunk_at(&found, lines, 0).map(|(_, m)| m),
            Some(Mark::RemovedBelow),
            "a removal behind the last line marks that line"
        );
    }

    #[test]
    fn committed_lines_take_the_breaks_of_the_file() {
        assert_eq!(committed_lines("a\nb\nc", &(1..3)), "b\nc");
        assert_eq!(with_breaks("b\r\nc\nd", "\r\n"), "b\r\nc\r\nd");
        assert_eq!(with_breaks("b\r\n", "\n"), "b\n");
    }

    #[test]
    fn committed_text_loses_its_byte_order_mark() {
        assert_eq!(decode(b"\xEF\xBB\xBFtext".to_vec()).as_deref(), Some("text"));
        assert_eq!(decode(vec![0xFF, 0xFE, b'a', 0]).as_deref(), Some("a"));
        assert_eq!(decode(vec![0xC3, 0x28]), None);
    }
}
