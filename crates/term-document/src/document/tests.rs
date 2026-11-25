use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use term_base::testing::thread_time;

use super::*;
use crate::disk::tests::scratch;
use crate::merge::Choice;
use crate::text::{Point, Unit};

/// Lets the clock move on, so that a file written next gets another date even at the same size.
fn later() {
    std::thread::sleep(Duration::from_millis(5));
}

fn edit(document: &mut Document, range_utf16: Range<usize>, text: &str) -> Change {
    document
        .apply(Edit {
            range_utf16,
            text: text.into(),
        })
        .unwrap()
}

/// What a language server holds after the changes it was sent, each applied at its line and UTF-16 column.
fn server_applies(server: &mut String, change: &Change) {
    let lines = Text::new(server);
    let start = lines.lsp_offset(
        u32::try_from(change.start.line).unwrap(),
        u32::try_from(change.start.utf16).unwrap(),
        Unit::Utf16,
    );
    let end = lines.lsp_offset(
        u32::try_from(change.old_end.line).unwrap(),
        u32::try_from(change.old_end.utf16).unwrap(),
        Unit::Utf16,
    );
    server.replace_range(start..end, &change.text);
}

/// What a text view holds after the changes it made, in its own UTF-16 units.
fn view_applies(view: &mut Vec<u16>, change: &Change) {
    let units: Vec<u16> = change.text.encode_utf16().collect();
    assert_eq!(units.len(), change.text_utf16);
    view.splice(change.utf16.clone(), units);
}

#[test]
fn a_change_says_where_it_happened_in_every_unit() {
    let mut document = Document::untitled("aä😀\r\nzweite Zeile");
    let change = edit(&mut document, 2..4, "x\ny");
    assert_eq!(change.bytes, 3..7);
    assert_eq!(change.utf16, 2..4);
    assert_eq!(
        change.start,
        Point {
            line: 0,
            utf8: 3,
            utf16: 2,
            utf32: 2
        }
    );
    assert_eq!(change.old_end.utf16, 4);
    assert_eq!(change.old_end.utf8, 7);
    assert_eq!(change.old_end.utf32, 3);
    assert_eq!(
        change.new_end,
        Point {
            line: 1,
            utf8: 1,
            utf16: 1,
            utf32: 1
        }
    );
    assert_eq!((change.new_bytes(), change.new_utf16()), (3..6, 2..5));
    assert_eq!(change.revision, Revision::new(1));
    assert_eq!(document.text().to_string(), "aäx\ny\r\nzweite Zeile");
}

#[test]
fn an_edit_between_cr_and_lf_widens_to_the_whole_break() {
    let mut document = Document::untitled("a\r\nb");
    let mut server = document.text().to_string();
    let change = edit(&mut document, 2..2, "X");
    assert_eq!((change.bytes.clone(), change.text.as_str()), (1..3, "\rX\n"));
    assert_eq!(document.text().to_string(), "a\rX\nb");
    server_applies(&mut server, &change);
    assert_eq!(server, "a\rX\nb");
}

#[test]
fn deleting_what_stands_between_a_lone_cr_and_an_lf_joins_them_into_one_break() {
    let mut document = Document::untitled("a\rX\nb");
    document.set_language(Language::named("rust"));
    document.highlight(0..3);
    let mut server = document.text().to_string();
    let change = edit(&mut document, 2..3, "");
    server_applies(&mut server, &change);
    assert_eq!(
        (document.text().to_string(), server),
        ("a\r\nb".to_string(), "a\r\nb".to_string())
    );
    assert_eq!(document.text().line_count(), 2);
    document.highlight(0..2);
}

#[test]
fn random_edits_reach_a_language_server_and_a_view_unchanged() {
    let pieces = ["a", "ä", "😀", "\n", "\r\n", "\r", "  ", "x\ny", ""];
    let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
    let mut below = |n: usize| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        usize::try_from(seed % u64::try_from(n.max(1)).unwrap()).unwrap()
    };
    let mut document = Document::untitled("start\r\nline\nend");
    let mut server = document.text().to_string();
    let mut view: Vec<u16> = server.encode_utf16().collect();
    for _ in 0..2000 {
        let len = document.text().len_utf16();
        let a = below(len + 1);
        let b = (a + below(4)).min(len);
        let text: String = (0..below(3)).map(|_| pieces[below(pieces.len())]).collect();
        let Ok(change) = document.apply(Edit {
            range_utf16: a..b,
            text,
        }) else {
            continue;
        };
        server_applies(&mut server, &change);
        view_applies(&mut view, &change);
        let now = document.text().to_string();
        assert_eq!(server, now);
        assert_eq!(String::from_utf16(&view).unwrap(), now);
    }
}

#[test]
fn replace_all_makes_small_edits_a_view_and_a_server_can_follow() {
    let old = "fn main() {\r\n    println!(\"hello\");\r\n}\r\n\r\nfn other() {}\r\n";
    let new = "fn main() {\r\n    println!(\"hello, world\");\r\n}\r\n\r\nfn other() {}\r\nfn third() {}\r\n";
    let mut document = Document::untitled(old);
    let mut server = old.to_string();
    let mut view: Vec<u16> = old.encode_utf16().collect();
    let changes = document.replace_all(new).unwrap();
    assert_eq!(changes.len(), 2);
    assert_eq!(changes[1].text, ", world");
    for change in &changes {
        server_applies(&mut server, change);
        view_applies(&mut view, change);
    }
    assert_eq!(document.text().to_string(), new);
    assert_eq!(server, new);
    assert_eq!(String::from_utf16(&view).unwrap(), new);
    assert!(document.replace_all(new).unwrap().is_empty());
}

#[test]
fn a_read_only_document_refuses_edits_and_a_lossy_one_stays_read_only() {
    let mut document = Document::untitled("text");
    document.set_read_only(true).unwrap();
    assert_eq!(
        document.apply(Edit {
            range_utf16: 0..0,
            text: "x".into()
        }),
        Err(EditError::ReadOnly(ReadOnly::Requested))
    );
    assert!(matches!(
        document.begin_save(ChangedOnDisk::Refuse),
        Err(SaveError::ReadOnly(ReadOnly::Requested))
    ));
    document.set_read_only(false).unwrap();
    edit(&mut document, 0..0, "x");

    let dir = scratch("lossy");
    let path = dir.join("latin1.txt");
    fs::write(&path, b"Gr\xFC\xDFe").unwrap();
    let mut lossy = Document::open(&path).unwrap();
    assert_eq!(lossy.read_only(), Some(ReadOnly::Undecodable));
    assert_eq!(lossy.set_read_only(false), Err(ReadOnly::Undecodable));
    assert!(matches!(
        lossy.begin_save_as(dir.join("copy.txt")),
        Err(SaveError::ReadOnly(_))
    ));
    assert_eq!(fs::read(&path).unwrap(), b"Gr\xFC\xDFe");
}

#[test]
fn a_protected_file_is_saved_only_once_the_user_allows_it_and_stays_protected() {
    let dir = scratch("protected-save");
    let path = dir.join("locked.txt");
    fs::write(&path, "old\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let mut document = Document::open(&path).unwrap();
    assert_eq!(document.read_only(), Some(ReadOnly::Protected));
    assert!(matches!(document.save(), Err(SaveError::ReadOnly(ReadOnly::Protected))));
    document.set_read_only(false).unwrap();
    edit(&mut document, 0..3, "new");
    document.save().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
    assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o444);
}

#[test]
fn edits_back_to_the_saved_text_make_the_document_clean_again() {
    let dir = scratch("dirty");
    let path = dir.join("a.txt");
    fs::write(&path, "abc").unwrap();
    let mut document = Document::open(&path).unwrap();
    assert!(!document.is_dirty());
    edit(&mut document, 3..3, "d");
    assert!(document.is_dirty());
    edit(&mut document, 3..4, "");
    assert!(!document.is_dirty(), "the text is what the file holds");
    edit(&mut document, 0..1, "x");
    edit(&mut document, 0..1, "y");
    assert!(document.is_dirty(), "same length, other text");
    let untitled = Document::untitled("");
    assert!(!untitled.is_dirty());
    assert!(Document::untitled("x").is_dirty());
}

#[test]
fn edits_at_both_ends_compare_only_what_lies_between_them() {
    let dir = scratch("dirty-ends");
    let path = dir.join("a.txt");
    fs::write(&path, "head middle tail").unwrap();
    let mut document = Document::open(&path).unwrap();
    edit(&mut document, 0..4, "HEAD");
    edit(&mut document, 12..16, "TAIL");
    assert_eq!(document.disk.as_ref().map(|disk| disk.agrees), Some((0, 0)));
    assert!(document.is_dirty());
    edit(&mut document, 12..16, "tail");
    assert!(document.is_dirty(), "the start still differs");
    edit(&mut document, 0..4, "head");
    assert!(!document.is_dirty(), "both ends are back");
    edit(&mut document, 5..11, "MIDDLE");
    assert_eq!(document.disk.as_ref().map(|disk| disk.agrees), Some((0, 0)));
    edit(&mut document, 5..11, "middle");
    assert!(!document.is_dirty());
}

#[test]
fn a_save_writes_the_format_it_read_and_typing_during_it_stays_unsaved() {
    let dir = scratch("save");
    let path = dir.join("win.txt");
    fs::write(&path, b"\xEF\xBB\xBFone\r\ntwo\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    assert_eq!(document.text().to_string(), "one\r\ntwo\n");
    assert_eq!(
        document.line_endings(),
        LineEndings {
            dominant: LineEnding::Lf,
            mixed: true
        }
    );
    edit(&mut document, 0..3, "ONE");
    let job = document.begin_save(ChangedOnDisk::Refuse).unwrap();
    edit(&mut document, 0..0, ">");
    let saved = job.run().unwrap();
    document.end_save(saved);
    assert_eq!(fs::read(&path).unwrap(), b"\xEF\xBB\xBFONE\r\ntwo\n");
    assert!(document.is_dirty(), "the edit after the save began");
    document.save().unwrap();
    assert!(!document.is_dirty());
    assert_eq!(fs::read(&path).unwrap(), b"\xEF\xBB\xBF>ONE\r\ntwo\n");
}

#[test]
fn a_save_that_finishes_after_a_newer_one_changes_nothing() {
    let dir = scratch("stale-save");
    let path = dir.join("a.txt");
    fs::write(&path, "base\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    edit(&mut document, 0..0, "1");
    let older = document.begin_save(ChangedOnDisk::Refuse).unwrap().run().unwrap();
    edit(&mut document, 0..0, "2");
    let newer = document.begin_save(ChangedOnDisk::Overwrite).unwrap().run().unwrap();
    document.end_save(newer);
    document.end_save(older);
    assert!(!document.is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "21base\n");
    edit(&mut document, 0..0, "3");
    document.save().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "321base\n");
}

/// What the app's `editor/saving.rs` relies on when the disk lane is full: the lane drops the refused job and its
/// answer unrun, and the copy the editor kept writes the file on its own thread.
#[test]
fn a_save_the_full_disk_lane_refuses_is_written_by_the_copy_kept_for_it() {
    use std::sync::mpsc::channel;
    use std::sync::{Arc, Barrier};
    use term_base::lane::{Lane, Refused};
    static LANE: Lane = Lane::new("test-full-save", 1, 1);
    let gate = Arc::new(Barrier::new(2));
    let held = gate.clone();
    let (started, running) = channel();
    LANE.try_run(move || {
        started.send(()).unwrap();
        held.wait();
    })
    .unwrap();
    running.recv_timeout(Duration::from_secs(5)).unwrap();
    LANE.try_run(|| ()).unwrap();

    let dir = scratch("full-lane");
    let path = dir.join("a.txt");
    fs::write(&path, "base\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    edit(&mut document, 0..0, "mine ");
    let job = document.begin_save(ChangedOnDisk::Refuse).unwrap();
    let kept = job.clone();
    let (answered, answers) = channel();
    let queued = LANE.ask(
        move || job.run(),
        move |written| answered.send(written.is_some()).unwrap(),
    );
    assert_eq!(queued, Err(Refused::Full));
    assert!(
        answers.recv().is_err(),
        "the refused answer is gone without being called"
    );
    document.end_save(kept.run().unwrap());
    gate.wait();
    assert!(!document.is_dirty());
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine base\n");
}

#[test]
fn a_save_refuses_to_overwrite_a_change_another_program_made() {
    let dir = scratch("overwrite");
    let path = dir.join("a.txt");
    fs::write(&path, "base\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    edit(&mut document, 0..0, "mine ");
    std::thread::sleep(Duration::from_millis(10));
    fs::write(&path, "theirs\n").unwrap();
    assert!(matches!(document.save(), Err(SaveError::ChangedOnDisk)));
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs\n");
    let saved = document.begin_save(ChangedOnDisk::Overwrite).unwrap().run().unwrap();
    document.end_save(saved);
    assert_eq!(fs::read_to_string(&path).unwrap(), "mine base\n");

    later();
    fs::write(&path, "mine base\n").unwrap();
    edit(&mut document, 0..0, "+");
    document.save().unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "+mine base\n",
        "a new date with the same bytes is no change"
    );
}

#[test]
fn a_clean_document_reloads_and_a_changed_one_merges_what_changed_on_disk() {
    let dir = scratch("reload");
    let path = dir.join("notes.md");
    fs::write(&path, "one\ntwo\nthree\nfour\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    let mut view: Vec<u16> = document.text().to_string().encode_utf16().collect();
    let probe = document.disk_probe().unwrap();
    assert!(matches!(probe.check().unwrap(), DiskCheck::Unchanged));

    later();
    fs::write(&path, "one\nTWO\nthree\nfour\n").unwrap();
    let Outcome::Reloaded(changes) = document.take_disk(probe.check().unwrap()).unwrap() else {
        panic!("a reload");
    };
    changes.iter().for_each(|change| view_applies(&mut view, change));
    assert_eq!(String::from_utf16(&view).unwrap(), "one\nTWO\nthree\nfour\n");
    assert!(!document.is_dirty());

    view_applies(&mut view, &edit(&mut document, 0..3, "ONE"));
    later();
    fs::write(&path, "one\nTWO\nthree\nFOUR\n").unwrap();
    let check = document.disk_probe().unwrap().check().unwrap();
    let Outcome::Merged(changes) = document.take_disk(check).unwrap() else {
        panic!("a merge");
    };
    changes.iter().for_each(|change| view_applies(&mut view, change));
    assert_eq!(document.text().to_string(), "ONE\nTWO\nthree\nFOUR\n");
    assert_eq!(String::from_utf16(&view).unwrap(), "ONE\nTWO\nthree\nFOUR\n");
    assert!(document.is_dirty());
    document.save().unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "ONE\nTWO\nthree\nFOUR\n");
}

#[test]
fn a_conflict_leaves_the_document_until_the_user_settles_it() {
    let dir = scratch("conflict");
    let path = dir.join("a.txt");
    fs::write(&path, "line\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    edit(&mut document, 0..4, "mine");
    later();
    fs::write(&path, "theirs\n").unwrap();
    let check = document.disk_probe().unwrap().check().unwrap();
    let Outcome::Conflict { conflict, disk } = document.take_disk(check).unwrap() else {
        panic!("a conflict");
    };
    assert_eq!(document.text().to_string(), "mine\n");
    let settled = conflict.resolve(&[Choice::DiskThenBuffer]);
    document.adopt_disk(disk, &settled).unwrap();
    assert_eq!(document.text().to_string(), "theirs\nmine\n");
    assert!(document.is_dirty());
    let check = document.disk_probe().unwrap().check().unwrap();
    assert!(
        matches!(check, DiskCheck::Unchanged),
        "the file's new version is known now"
    );
    document.save().unwrap();
}

#[test]
fn a_removed_file_leaves_its_text_unsaved_and_a_restored_one_is_known_again() {
    let dir = scratch("removed");
    let path = dir.join("a.txt");
    fs::write(&path, "text\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let check = document.disk_probe().unwrap().check().unwrap();
    assert!(matches!(document.take_disk(check).unwrap(), Outcome::Removed));
    assert!(document.is_dirty());
    assert!(!document.has_own_changes());
    edit(&mut document, 0..0, "my ");
    assert!(document.has_own_changes());
    edit(&mut document, 0..3, "");
    assert!(!document.has_own_changes());
    later();
    fs::write(&path, "text\n").unwrap();
    let check = document.disk_probe().unwrap().check().unwrap();
    assert!(matches!(document.take_disk(check).unwrap(), Outcome::Kept));
    assert!(!document.is_dirty());
}

#[test]
fn a_file_that_turns_binary_under_unsaved_text_is_a_conflict_not_a_reload() {
    let dir = scratch("turns-binary");
    let path = dir.join("a.txt");
    fs::write(&path, "text\n").unwrap();
    let mut document = Document::open(&path).unwrap();
    edit(&mut document, 0..0, "my ");
    later();
    fs::write(&path, b"\0\x01\x02").unwrap();
    let check = document.disk_probe().unwrap().check().unwrap();
    let Outcome::Conflict { conflict, disk } = document.take_disk(check).unwrap() else {
        panic!("a conflict");
    };
    assert_eq!(conflict.hunks()[0].buffer, "my text\n");
    document.adopt_disk(disk, &conflict.resolve(&[Choice::Disk])).unwrap();
    assert_eq!(document.read_only(), Some(ReadOnly::Binary));
}

#[test]
fn the_language_follows_the_file_unless_the_user_chose_one() {
    let dir = scratch("language");
    let mut document = Document::untitled("#!/usr/bin/env python3\nprint(1)\n");
    assert_eq!(document.language().map(Language::name), Some("python"));
    document.save_as(dir.join("tool.rs")).unwrap();
    assert_eq!(document.language().map(Language::name), Some("rust"));
    document.set_language(Language::named("toml"));
    document.set_path(dir.join("other.py"));
    assert_eq!(document.language().map(Language::name), Some("toml"));
}

#[test]
fn line_endings_report_the_most_common_break_and_a_mix() {
    let of = |text: &str| LineEndings::of(&Text::new(text));
    assert_eq!(of(""), LineEndings::default());
    assert_eq!(
        of("a\r\nb\r\nc\n"),
        LineEndings {
            dominant: LineEnding::Crlf,
            mixed: true
        }
    );
    assert_eq!(of("a\rb\r").dominant, LineEnding::Cr);
    assert!(!of("a\nb\n").mixed);
    assert_eq!(LineEnding::Crlf.as_str(), "\r\n");
}

#[test]
fn a_document_can_be_shared_between_threads() {
    fn shared<T: Send + Sync>() {}
    shared::<Document>();
    shared::<Snapshot>();
    shared::<SaveJob>();
    shared::<DiskProbe>();
    shared::<crate::recovery::Recovery>();
}

fn rust_lines(count: usize) -> String {
    let shapes = [
        "    let value_{n} = compute(\"label {n}\", {n}) + 0x{n:x}; // note {n}",
        "    if value_{n} > {n} {{ return Err(Error::new(\"too big\")); }}",
        "    /* block comment {n} */ items.push(Item {{ id: {n}, name: String::from(\"x\") }});",
        "fn helper_{n}(input: &str) -> Result<usize, Error> {{",
        "}}",
    ];
    (0..count)
        .map(|n| {
            let shape = shapes[n % shapes.len()];
            shape
                .replace("{n:x}", &format!("{n:x}"))
                .replace("{n}", &n.to_string())
                .replace("{{", "{")
                .replace("}}", "}")
                + "\n"
        })
        .collect()
}

/// `count` lines of Markdown with every kind of block splat colours, a section of 18 lines over and over.
fn markdown_lines(count: usize) -> String {
    let section = [
        "## Section {n}",
        "",
        "Text with **bold**, *emphasis*, `code {n}` and a [link {n}](https://example.org/{n}), long enough to wrap.",
        "More of the same paragraph with _more_ words and ~~struck~~ ones, and a formula $x_{n}^2$ in it.",
        "",
        "- item {n} with `code`",
        "- [ ] task {n}",
        "",
        "> A quote {n} with a <b>tag</b>",
        "",
        "| col | value |",
        "|-----|-------|",
        "| a{n} | {n} |",
        "",
        "```rust",
        "fn f_{n}() -> u32 { let s = \"{n}\"; {n} }",
        "```",
        "",
    ];
    (0..count)
        .map(|n| section[n % section.len()].replace("{n}", &(n / section.len()).to_string()) + "\n")
        .collect()
}

/// Per keystroke, in CPU time: the edit, then lexing the visible lines.
fn type_in(document: &mut Document, keystrokes: usize) -> (Duration, Duration) {
    let mut times = Vec::with_capacity(keystrokes);
    for k in 0..keystrokes {
        let line = 5000 + (k % 40);
        let byte = document.text().line_content(line).unwrap().end;
        let at = document.text().byte_to_utf16(byte).unwrap();
        let typed = if k % 7 == 6 { "\n" } else { "x" };
        let start = thread_time();
        document
            .apply(Edit {
                range_utf16: at..at,
                text: typed.into(),
            })
            .unwrap();
        document.highlight(line.saturating_sub(30)..line + 30);
        times.push(thread_time().saturating_sub(start));
    }
    times.sort();
    (times[times.len() / 2], times[times.len() - 1])
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn budget_typing_in_ten_thousand_lines_of_rust_stays_far_below_a_frame() {
    let mut document = Document::untitled(&rust_lines(10_000));
    document.set_language(Language::named("rust"));
    document.highlight(4970..5040);
    let (median, slowest) = type_in(&mut document, 200);
    term_base::perf::durations("term-document: a keystroke in 10 000 lines of Rust, median", &[median]);
    term_base::perf::durations(
        "term-document: a keystroke in 10 000 lines of Rust, slowest",
        &[slowest],
    );
    assert!(
        slowest < Duration::from_millis(8),
        "median {median:?}, slowest {slowest:?}"
    );
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn budget_typing_in_ten_thousand_lines_of_markdown_stays_below_a_frame() {
    let mut document = Document::untitled(&markdown_lines(10_000));
    document.set_language(Some(Language::MARKDOWN));
    let start = Instant::now();
    document.highlight(4970..5040);
    let opened = start.elapsed();
    while !document.is_highlighted() {
        document.highlight_more(Duration::from_millis(4));
        std::thread::sleep(Duration::from_millis(1));
    }
    let (median, slowest) = type_in(&mut document, 200);
    term_base::perf::durations("term-document: the first screen of 10 000 lines of Markdown", &[opened]);
    term_base::perf::durations(
        "term-document: a keystroke in 10 000 lines of Markdown, median",
        &[median],
    );
    term_base::perf::durations(
        "term-document: a keystroke in 10 000 lines of Markdown, slowest",
        &[slowest],
    );
    println!(
        "Markdown, 10 000 lines: parsed for the first screen in {opened:?}; keystroke median {median:?}, slowest {slowest:?}"
    );
    assert!(
        slowest < Duration::from_millis(8),
        "median {median:?}, slowest {slowest:?}"
    );
}

#[test]
fn opening_a_large_file_lexes_only_what_is_shown() {
    let text = rust_lines(40_000);
    let mut document = Document::untitled(&text);
    document.set_language(Language::named("rust"));
    assert_eq!(document.highlight(0..60), Some(0..60));
    assert!(!document.is_highlighted());
    assert!(document.spans(60).is_empty() && document.spans(39_000).is_empty());
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn budget_the_first_screen_of_a_large_file_is_lexed_at_once() {
    let text = rust_lines(40_000);
    let mut times: Vec<_> = (0..5)
        .map(|_| {
            let mut document = Document::untitled(&text);
            document.set_language(Language::named("rust"));
            let start = Instant::now();
            document.highlight(0..60);
            start.elapsed()
        })
        .collect();
    term_base::perf::durations("term-document: lexing the first screen of 40 000 lines", &times);
    times.sort();
    let first_screen = times[times.len() / 2];
    assert!(first_screen < Duration::from_millis(100), "{first_screen:?}");
}

/// The Markdown part of `budgets`: the first screen of 10 000 lines, typing, and a keystroke that opens a code block
/// and so restyles everything below it.
fn markdown_budgets() {
    let mut document = Document::untitled(&markdown_lines(10_000));
    document.set_language(Some(Language::MARKDOWN));
    while !document.is_highlighted() {
        document.highlight_more(Duration::from_millis(4));
    }
    let (median, slowest) = type_in(&mut document, 2000);
    term_base::perf::durations(
        "term-document: 2000 keystrokes in 10 000 lines of Markdown, median",
        &[median],
    );
    println!("keystroke in 10 000 lines of Markdown: median {median:?}, slowest {slowest:?}");
    let at = document
        .text()
        .byte_to_utf16(document.text().line_bytes(5000).unwrap().start)
        .unwrap();
    let start = thread_time();
    document
        .apply(Edit {
            range_utf16: at..at,
            text: "```\n".into(),
        })
        .unwrap();
    document.highlight(4970..5040);
    println!(
        "opening a code block at line 5000: {:?} for the screen",
        thread_time().saturating_sub(start)
    );
    let start = Instant::now();
    while !document.is_highlighted() {
        document.highlight_more(Duration::from_millis(4));
    }
    println!("and {:?} in slices of 4 ms for the rest", start.elapsed());
    for lines in [10_000, 100_000, 400_000] {
        let text = markdown_lines(lines);
        let start = Instant::now();
        let mut document = Document::untitled(&text);
        document.set_language(Some(Language::MARKDOWN));
        document.highlight(0..60);
        let first = start.elapsed();
        while !document.is_highlighted() {
            document.highlight_more(Duration::from_millis(4));
        }
        println!(
            "Markdown, {lines} lines ({} KB): first screen {first:?}, all styled after {:?}",
            text.len() / 1024,
            start.elapsed()
        );
        if lines == 100_000 {
            let (median, slowest) = type_in(&mut document, 500);
            println!("keystroke in {lines} lines of Markdown: median {median:?}, slowest {slowest:?}");
        }
    }
}

/// Release timings: `cargo test --release -p term-document -- --ignored --nocapture budget`.
#[test]
#[ignore = "a measurement, meaningful only in a release build"]
fn budgets() {
    let ten_k = rust_lines(10_000);
    let start = Instant::now();
    let mut document = Document::untitled(&ten_k);
    document.set_language(Language::named("rust"));
    document.highlight(0..usize::MAX);
    println!(
        "10 000 lines ({} KB): open and lex all {:?}",
        ten_k.len() / 1024,
        start.elapsed()
    );
    let (median, slowest) = type_in(&mut document, 2000);
    term_base::perf::durations(
        "term-document: 2000 keystrokes in 10 000 lines of Rust, median",
        &[median],
    );
    println!("keystroke in 10 000 lines: median {median:?}, slowest {slowest:?}");
    markdown_budgets();

    let dir = scratch("budgets");
    let path = dir.join("big.rs");
    let mut big = String::new();
    while big.len() < 10 << 20 {
        big.push_str(&ten_k);
    }
    fs::write(&path, &big).unwrap();
    let start = Instant::now();
    let mut document = Document::open(&path).unwrap();
    let opened = start.elapsed();
    term_base::perf::durations("term-document: reading and building 10 MB", &[opened]);
    document.highlight(0..60);
    println!(
        "10 MB ({} lines): read and build {opened:?}, first screen lexed {:?} in all",
        document.text().line_count(),
        start.elapsed()
    );
    let start = Instant::now();
    while !document.is_highlighted() {
        document.highlight_more(Duration::from_millis(4));
    }
    println!("10 MB: lexing the rest in slices of 4 ms took {:?}", start.elapsed());
    let (median, slowest) = type_in(&mut document, 500);
    term_base::perf::durations("term-document: a keystroke in 10 MB, median", &[median]);
    println!("keystroke in 10 MB: median {median:?}, slowest {slowest:?}");
    let start = Instant::now();
    let folds = document.folds(4).len();
    println!("10 MB: {folds} folds in {:?}", start.elapsed());
    let edited = document.text().to_string().replacen("value_15 ", "value_fifteen ", 1);
    let start = Instant::now();
    let changes = document.replace_all(&edited).unwrap();
    println!(
        "10 MB: replace_all with one change: {} edit(s) in {:?}",
        changes.len(),
        start.elapsed()
    );
    document
        .apply(Edit {
            range_utf16: 0..0,
            text: "{".into(),
        })
        .unwrap();
    document.highlight(0..1);
    let start = Instant::now();
    let pair = document.matching_bracket(0);
    println!(
        "10 MB: a bracket without partner searched through {} lines in {:?} ({pair:?})",
        crate::syntax::BRACKET_LINES,
        start.elapsed()
    );
}
