//! Markdown's colours as splat's source view gives them: comrak parses the text and splat styles what it parsed
//! (headings, emphasis, links, code spans, code blocks in their language's colours, tables, HTML, front matter,
//! formulas). After an edit only the top-level blocks around it are parsed again, and only the lines of those blocks
//! are cut anew from the styled stretches, the lines asked for first and the rest a slice at a time.
//!
//! The first parse of a long text takes longer than a frame, so it runs on a thread of its own; until it is done, the
//! lines asked for are cut from a parse of the text's start, and edits made meanwhile are parsed into it once it is.
//! An edit that makes a long text be parsed anew as a whole, such as a code fence opened or closed above most of it,
//! is parsed on such a thread as well, and the lines keep their stretches until it is done.

use std::borrow::Cow;
use std::ops::Range;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use term_base::LockIgnoringPoison;

use splat_markdown::doc::Doc;
pub use splat_markdown::highlight::{Style, Token};
use splat_markdown::incremental::{
    Parsed, parse_and_color, parse_shared_and_color, reparse_edit_and_color, reparse_part_and_color,
};
use splat_markdown::text::bytes;

use super::{Changed, Columns, LINES_PER_STEP, LineEdit, splice_lines};
use crate::edit::Change;
use crate::text::Text;

/// The longest text parsed on the spot; about 7 ms of a release build.
const PARSED_AT_ONCE: usize = 128 * 1024;

/// The most edits at once merged into the one edit handed to splat; a new text of many edits mostly changes much of
/// it, so splat compares it with the parsed one as a whole instead.
const MOST_SPLICES: usize = 8;

/// How the text differs from the one parsed last, or being parsed on a thread of its own.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Since {
    Unchanged,
    /// The bytes `old` of the parsed text are the bytes `old.start..new_end` of the text now.
    Edit {
        old: Range<usize>,
        new_end: usize,
    },
    Unknown,
}

impl Since {
    /// Takes in the edit that replaced the bytes `r` of the text as it was with `added` bytes.
    fn merge(&mut self, r: &Range<usize>, added: usize) {
        *self = match std::mem::replace(self, Since::Unknown) {
            Since::Unchanged => Since::Edit {
                old: r.clone(),
                new_end: r.start + added,
            },
            Since::Edit { old, new_end } => {
                let end = r.end.max(new_end);
                Since::Edit {
                    old: r.start.min(old.start)..old.end + (end - new_end),
                    new_end: end - r.end + r.start + added,
                }
            }
            Since::Unknown => Since::Unknown,
        };
    }
}

/// A stretch of a Markdown line in one style, in UTF-16 columns of that line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Styled {
    /// The first column.
    pub start: u32,
    /// The column after the last.
    pub end: u32,
    /// Its colour and font traits.
    pub style: Style,
}

/// A line's stretches; `fresh` once they are cut from the parse of the current text.
#[derive(Debug, Clone)]
struct Cut {
    styles: Box<[Styled]>,
    fresh: bool,
}

/// A parse of the whole of a long text, under way on a thread of its own.
struct Opening {
    result: Arc<Mutex<Option<Parsed>>>,
    /// The text changed since the parse began.
    edited: bool,
}

/// The parse of a Markdown document and its lines' stretches.
pub(crate) struct Markdown {
    parsed: Option<Parsed>,
    opening: Option<Opening>,
    /// While the first parse is under way: a parse of the start of the text as it is now, and the lines it covers;
    /// while a later one is, none.
    interim: Option<Parsed>,
    interim_lines: usize,
    /// The text changed since it was parsed.
    stale: bool,
    lines: Vec<Option<Cut>>,
    /// No line before this one waits to be cut.
    todo: usize,
    since: Since,
}

impl Clone for Markdown {
    /// A copy without the parse, which it makes again when asked for colours; its lines stay as they were cut.
    fn clone(&self) -> Markdown {
        let mut lines = self.lines.clone();
        lines.iter_mut().flatten().for_each(|line| line.fresh = false);
        Markdown {
            lines,
            ..Markdown::new(0)
        }
    }
}

impl std::fmt::Debug for Markdown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Markdown")
            .field("parsed", &self.parsed.is_some())
            .field("opening", &self.opening.is_some())
            .field("stale", &self.stale)
            .field("lines", &self.lines.len())
            .field("todo", &self.todo)
            .finish_non_exhaustive()
    }
}

impl Markdown {
    pub(crate) fn new(line_count: usize) -> Markdown {
        Markdown {
            parsed: None,
            opening: None,
            interim: None,
            interim_lines: 0,
            stale: true,
            lines: vec![None; line_count],
            todo: 0,
            since: Since::Unknown,
        }
    }

    /// The lines of `edits` were replaced, one edit after the other, by the text `changes` made.
    pub(crate) fn edited(&mut self, edits: &[LineEdit], changes: &[Change]) {
        if changes.len() > MOST_SPLICES {
            self.since = Since::Unknown;
        }
        changes
            .iter()
            .for_each(|change| self.since.merge(&change.bytes, change.text.len()));
        let Some(first) = edits.iter().map(|edit| edit.first).min() else {
            return;
        };
        splice_lines(&mut self.lines, edits);
        self.todo = self.todo.min(first);
        match &mut self.opening {
            Some(opening) => {
                opening.edited = true;
                // The start parsed meanwhile changes only with an edit in it.
                self.stale |= first < self.interim_lines;
            }
            None => self.stale = true,
        }
    }

    pub(crate) fn styles(&self, line: usize) -> &[Styled] {
        self.lines
            .get(line)
            .and_then(Option::as_ref)
            .map_or(&[], |cut| &cut.styles)
    }

    pub(crate) fn is_complete(&self) -> bool {
        !self.stale && self.opening.is_none() && self.todo >= self.lines.len()
    }

    /// Parses what changed and cuts `lines`; the lines whose stretches changed.
    pub(crate) fn highlight(&mut self, text: &Text, lines: Range<usize>, changed: &mut Changed) {
        self.parse(text);
        for n in lines.start..lines.end.min(self.lines.len()) {
            self.cut(text, n, changed);
        }
        self.skip_fresh();
    }

    /// Parses what changed and cuts the lines still waiting until `deadline`.
    pub(crate) fn highlight_until(&mut self, text: &Text, deadline: Instant, changed: &mut Changed) {
        self.parse(text);
        if self.opening.is_some() {
            return;
        }
        while self.todo < self.lines.len() {
            let step_end = self.lines.len().min(self.todo + LINES_PER_STEP);
            for n in self.todo..step_end {
                self.cut(text, n, changed);
            }
            self.todo = step_end;
            if Instant::now() >= deadline {
                break;
            }
        }
    }

    fn skip_fresh(&mut self) {
        while self
            .lines
            .get(self.todo)
            .is_some_and(|l| l.as_ref().is_some_and(|c| c.fresh))
        {
            self.todo += 1;
        }
    }

    /// Brings the parse in line with `text`, and marks the lines of the blocks parsed again for cutting.
    fn parse(&mut self, text: &Text) {
        if !self.took_opening() {
            if self.stale {
                self.stale = false;
                self.parse_start(text);
            }
            return;
        }
        if !self.stale {
            return;
        }
        self.stale = false;
        let parsed = match self.parsed.take() {
            Some(old) => match self.reparse(old, text) {
                Ok(parsed) => parsed,
                Err((old, whole)) if whole.len() > PARSED_AT_ONCE => {
                    let shared: Arc<str> = whole.into();
                    if self.parse_aside(shared.clone(), Some(old)) {
                        return;
                    }
                    parse_shared_and_color(shared)
                }
                Err((_, whole)) => parse_and_color(&whole),
            },
            None if text.len_bytes() <= PARSED_AT_ONCE => parse_and_color(&whole_of(text)),
            None => return self.open(whole_of(text).into(), text),
        };
        let lines = match &parsed.splice {
            Some(splice) if !crosses(&parsed, &splice.window) => {
                let (start, end) = (splice.window.start.get(), splice.window.end.get());
                text.line_of(start)..text.line_of(end.saturating_sub(1).max(start)) + 1
            }
            _ => 0..self.lines.len(),
        };
        self.recut(lines);
        self.parsed = Some(parsed);
        self.since = Since::Unchanged;
    }

    /// Parses `old` again around what changed since; the tree as it was and the whole text where splat has to parse
    /// all of it.
    fn reparse(&mut self, old: Parsed, text: &Text) -> Result<Parsed, (Box<Doc>, String)> {
        let parsed_len = old.doc.text().len();
        let edit = match std::mem::replace(&mut self.since, Since::Unknown) {
            Since::Unchanged => Some((0..0, 0)),
            Since::Edit { old, new_end } => Some((old, new_end)),
            Since::Unknown => None,
        };
        let with = edit.filter(|(replaced, new_end)| {
            replaced.end <= parsed_len && parsed_len - replaced.len() + (new_end - replaced.start) == text.len_bytes()
        });
        let Some((replaced, with)) = with.and_then(|(replaced, new_end)| {
            Some((replaced.clone(), text.rope().get_byte_slice(replaced.start..new_end)?))
        }) else {
            let whole = whole_of(text);
            return reparse_part_and_color(old.doc, &old.spans, &whole).map_err(|old| (old, whole));
        };
        let reparsed = reparse_edit_and_color(old.doc, &old.spans, bytes(replaced), &Cow::from(with));
        if let Ok(parsed) = &reparsed {
            debug_assert!(
                parsed.doc.text() == text.to_string(),
                "the edits since the parse make the text"
            );
        }
        reparsed.map_err(|old| (old, whole_of(text)))
    }

    /// Starts the first parse of `whole` on a thread of its own, and parses the start of the text meanwhile.
    fn open(&mut self, whole: Arc<str>, text: &Text) {
        if !self.parse_aside(whole.clone(), None) {
            self.parsed = Some(parse_shared_and_color(whole));
            self.since = Since::Unchanged;
            self.recut(0..self.lines.len());
            return;
        }
        self.parse_start(text);
    }

    /// Parses `whole` on a thread of its own, which also drops the tree `old` of the text before; whether the thread
    /// started.
    fn parse_aside(&mut self, whole: Arc<str>, old: Option<Box<Doc>>) -> bool {
        let result = Arc::new(Mutex::new(None));
        let slot = result.clone();
        let started = std::thread::Builder::new()
            .name("markdown-parse".into())
            .spawn(move || {
                drop(old);
                let parsed = parse_shared_and_color(whole);
                *slot.lock_ignoring_poison() = Some(parsed);
            });
        if started.is_err() {
            return false;
        }
        self.opening = Some(Opening { result, edited: false });
        self.since = Since::Unchanged;
        self.interim = None;
        self.interim_lines = 0;
        true
    }

    fn parse_start(&mut self, text: &Text) {
        let start = start_of(text);
        self.interim_lines = text.line_of(start.len());
        self.interim = Some(parse_and_color(&start));
    }

    /// Takes the first parse once its thread is done; whether none is under way any more.
    fn took_opening(&mut self) -> bool {
        let Some(opening) = &self.opening else {
            return true;
        };
        let Some(parsed) = opening.result.lock_ignoring_poison().take() else {
            return false;
        };
        self.stale = opening.edited;
        self.opening = None;
        self.interim = None;
        self.parsed = Some(parsed);
        self.recut(0..self.lines.len());
        true
    }

    /// Marks `lines` to be cut again.
    fn recut(&mut self, lines: Range<usize>) {
        let count = self.lines.len();
        for line in self.lines[lines.start.min(count)..lines.end.min(count)]
            .iter_mut()
            .flatten()
        {
            line.fresh = false;
        }
        self.todo = self.todo.min(lines.start);
    }

    /// Cuts line `n` from the parse unless it is cut already; while the first parse is under way from the parse of
    /// the text's start, to be cut again once it is done, and while a later one is not at all where it was cut before.
    fn cut(&mut self, text: &Text, n: usize, changed: &mut Changed) {
        let aside = self.opening.is_some() && self.interim.is_none();
        if self.lines[n].as_ref().is_some_and(|c| c.fresh || aside) {
            return;
        }
        let (parsed, fresh) = match &self.opening {
            None => (self.parsed.as_ref(), true),
            Some(_) => (self.interim.as_ref(), false),
        };
        let styles = parsed.map_or_else(Box::default, |p| line_styles(p, text, n));
        if self.lines[n].as_ref().is_none_or(|old| old.styles != styles) {
            changed.add(n);
        }
        self.lines[n] = Some(Cut { styles, fresh });
    }
}

fn whole_of(text: &Text) -> String {
    let mut whole = String::with_capacity(text.len_bytes());
    text.chunks().for_each(|chunk| whole.push_str(chunk));
    whole
}

/// The text up to the start of the line in which the first `PARSED_AT_ONCE` bytes end.
fn start_of(text: &Text) -> String {
    let end = text
        .line_bytes(text.line_of(PARSED_AT_ONCE.min(text.len_bytes())))
        .map_or(text.len_bytes(), |line| line.start);
    let mut start = String::with_capacity(end);
    for chunk in text.chunks() {
        let room = end - start.len();
        if room == 0 {
            break;
        }
        start.push_str(&chunk[..room.min(chunk.len())]);
    }
    start
}

/// Whether a stretch reaches across an edge of the window parsed again: then splat styled the whole text anew, and
/// lines outside the window may have changed as well.
fn crosses(parsed: &Parsed, window: &Range<splat_markdown::text::Byte>) -> bool {
    let spans = &parsed.spans;
    let across = |edge: splat_markdown::text::Byte| {
        let at = spans.partition_point(|(r, _)| r.end <= edge);
        spans.get(at).is_some_and(|(r, _)| r.start < edge)
    };
    across(window.start) || across(window.end)
}

/// The stretches of line `n` in the parse.
fn line_styles(parsed: &Parsed, text: &Text, n: usize) -> Box<[Styled]> {
    let (Some(bytes), Some(content)) = (text.line_content(n), text.line(n)) else {
        return Box::default();
    };
    let spans = &parsed.spans;
    let first = spans.partition_point(|(r, _)| r.end.get() <= bytes.start);
    let mut columns = Columns::new(&content[..bytes.end - bytes.start]);
    spans[first..]
        .iter()
        .take_while(|(r, _)| r.start.get() < bytes.end)
        .filter_map(|(r, style)| {
            let (from, to) = (r.start.get().max(bytes.start), r.end.get().min(bytes.end));
            (from < to).then(|| Styled {
                start: columns.at(from - bytes.start),
                end: columns.at(to - bytes.start),
                style: *style,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
