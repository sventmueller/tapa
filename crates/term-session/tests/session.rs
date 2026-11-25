//! Sessions with real programs in real PTYs.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

mod common;

use std::time::{Duration, Instant};

use common::{Harness, command, gone_within, temporary};
use term_base::Ending;
use term_session::event::{Notification, Progress, ProgressState};
use term_session::{
    Agent, CellFlags, CellSide, Clipboard, Event, Foreground, Key, NamedKey, Options, PasteAction, Point,
    SelectionKind, Size, WorkingDirectory,
};

fn interactive_sh(dir: &std::path::Path) -> Harness {
    interactive_sh_with(command("/bin/sh", &["-i"], dir), dir)
}

fn interactive_sh_with(options: Options, dir: &std::path::Path) -> Harness {
    let mut harness = Harness::start(options, dir);
    harness.session.write("PS1='$ '\n");
    assert!(harness.wait_until(|h| h.screen_line("$")), "the shell prompts");
    harness
}

#[test]
fn typed_keys_reach_the_shell_and_its_output_lands_on_the_screen() {
    let dir = temporary("round-trip");
    let mut harness = interactive_sh(&dir);
    for c in "printf '%s-%s\\n' round trip".chars() {
        assert!(harness.session.key(&Key::text(c)));
    }
    harness.session.key(&Key::named(NamedKey::Enter));
    assert!(
        harness.wait_until(|h| h.screen_line("round-trip")),
        "{:?}",
        harness.session.visible_text()
    );
    assert!(harness.saw(&Event::Redraw));
}

#[test]
fn clearing_drops_the_history_and_the_lines_above_the_prompt() {
    let dir = temporary("clear");
    let mut harness = interactive_sh(&dir);
    harness
        .session
        .write("i=0; while [ $i -lt 60 ]; do i=$((i+1)); echo row$i; done; echo done\n");
    assert!(harness.wait_until(|h| h.screen_line("done") && h.session.snapshot().history > 0));
    harness.session.write("printf 'kept'\n");
    assert!(harness.wait_until(|h| h.screen_line("kept$")));
    harness.session.clear(true);
    let screen = harness.session.snapshot();
    assert_eq!(screen.history, 0);
    assert_eq!(screen.cursor.point.line, 0);
    let text = harness.session.visible_text();
    assert_eq!(text[0], "kept$", "{text:?}");
    assert!(text[1..].iter().all(String::is_empty), "{text:?}");
}

/// An interactive shell in `dir` with `sleep 30` as its job in front, and the pids of the job and the shell.
fn shell_with_a_sleeping_job(dir: &std::path::Path) -> (Harness, i32, i32) {
    let mut harness = interactive_sh(dir);
    harness.session.write("/bin/sh -c 'echo pid=$$; exec sleep 30'\n");
    assert!(harness.wait_until(|h| h.number_after("pid=").is_some()));
    let sleeper = harness.number_after("pid=").unwrap();
    assert!(harness.wait_until(|h| h.session.foreground() == Foreground::Other("sleep".into())));
    let shell = i32::try_from(harness.session.pid()).unwrap();
    (harness, sleeper, shell)
}

#[test]
#[ignore = "a timing budget (E25), run by make perf on an idle Mac"]
fn budget_closing_a_session_returns_at_once() {
    let dir = temporary("close-budget");
    let (harness, sleeper, _) = shell_with_a_sleeping_job(&dir);
    let started = Instant::now();
    harness.session.close();
    let took = started.elapsed();
    term_base::perf::durations("term-session: closing a session", &[took]);
    assert!(gone_within(sleeper, Duration::from_secs(5)), "the job survived");
    assert!(took < Duration::from_millis(50), "close took {took:?}");
}

#[test]
fn closing_a_session_ends_the_job_in_front() {
    let dir = temporary("close");
    let (harness, sleeper, shell) = shell_with_a_sleeping_job(&dir);
    harness.session.close();

    assert!(gone_within(sleeper, Duration::from_secs(5)), "the job survived");
    assert!(gone_within(shell, Duration::from_secs(5)), "the shell survived");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !harness.supervisor.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(harness.supervisor.is_empty(), "the spawner still lists the shell");
}

#[test]
fn a_job_that_ignores_hang_up_and_sigterm_is_killed_after_the_grace_periods() {
    let dir = temporary("stubborn");
    let options = Options {
        close_grace: Duration::from_secs(1),
        ..command("/bin/sh", &["-i"], &dir)
    };
    let grace = options.close_grace;
    let mut harness = Harness::start(options, &dir);
    harness
        .session
        .write("/bin/sh -c 'trap \"\" HUP TERM; echo pid=$$; exec sleep 30'\n");
    assert!(harness.wait_until(|h| h.number_after("pid=").is_some()));
    let stubborn = harness.number_after("pid=").unwrap();
    assert!(harness.wait_until(|h| h.session.foreground() == Foreground::Other("sleep".into())));
    let closed = Instant::now();
    harness.session.close();
    assert!(gone_within(stubborn, Duration::from_secs(5)), "SIGKILL did not come");
    assert!(
        closed.elapsed() >= grace * 2,
        "hang-up or SIGTERM ended it after {:?}",
        closed.elapsed()
    );
}

#[test]
fn the_end_of_the_program_is_reported_with_its_exit_code_and_the_supervisor_forgets_it() {
    let dir = temporary("exit");
    let mut harness = Harness::start(command("/bin/sh", &["-c", "exit 7"], &dir), &dir);
    assert!(
        harness.wait_until(|h| h.saw(&Event::Exited(Ending::Code(7)))),
        "{:?}",
        harness.events
    );
    assert!(!harness.session.is_running());
    assert!(harness.wait_until(|h| h.supervisor.is_empty()));
}

#[test]
fn osc_sequences_and_server_announcements_become_events() {
    let dir = temporary("osc");
    let script = r"printf '\033]0;hello\007\007\033]52;c;aGk=\007\033]52;c;?\007'; printf '\033]7;file:///tmp/a%%20b\007'; echo 'Local: http://localhost:5173/'";
    let mut harness = Harness::start(command("/bin/sh", &["-c", script], &dir), &dir);
    assert!(
        harness.wait_until(|h| h.saw(&Event::Exited(Ending::Code(0)))),
        "{:?}",
        harness.events
    );
    let wanted = [
        Event::Title(Some("hello".into())),
        Event::Bell,
        Event::ClipboardStore {
            clipboard: Clipboard::Clipboard,
            text: "hi".into(),
        },
        Event::WorkingDirectory(WorkingDirectory {
            host: String::new(),
            path: "/tmp/a b".into(),
        }),
        Event::DevServer("http://localhost:5173/".into()),
    ];
    for event in &wanted {
        assert!(harness.saw(event), "missing {event:?} in {:?}", harness.events);
    }
    assert!(
        !harness
            .events
            .iter()
            .any(|event| matches!(event, Event::ClipboardLoad(_))),
        "reading is denied"
    );
}

#[test]
fn progress_reports_and_notifications_become_events() {
    let dir = temporary("progress");
    let script = r"printf '\033]9;4;1;40\007\033]9;4;0\033\\\033]9;Build done\007\033]777;notify;cargo;3 failed\007'";
    let mut harness = Harness::start(command("/bin/sh", &["-c", script], &dir), &dir);
    assert!(
        harness.wait_until(|h| h.saw(&Event::Exited(Ending::Code(0)))),
        "{:?}",
        harness.events
    );
    let wanted = [
        Event::Progress(Some(Progress {
            state: ProgressState::Normal,
            percent: Some(40),
        })),
        Event::Progress(None),
        Event::Notification(Notification {
            title: None,
            body: "Build done".into(),
        }),
        Event::Notification(Notification {
            title: Some("cargo".into()),
            body: "3 failed".into(),
        }),
    ];
    for event in &wanted {
        assert!(harness.saw(event), "missing {event:?} in {:?}", harness.events);
    }
}

#[test]
fn a_clipboard_read_is_asked_for_only_when_allowed_and_answered_in_osc_52() {
    let dir = temporary("clipboard");
    let script = r"stty raw -echo; printf '\033]52;c;?\007'; reply=$(dd bs=1 count=12 2>/dev/null); stty sane; echo; echo got:$(printf '%s' $reply | tr -d '\033\007')";
    let options = Options {
        allow_clipboard_read: true,
        ..command("/bin/sh", &["-c", script], &dir)
    };
    let mut harness = Harness::start(options, &dir);
    assert!(harness.wait_until(|h| h.saw(&Event::ClipboardLoad(Clipboard::Clipboard))));
    harness.session.answer_clipboard("hi");
    assert!(
        harness.wait_until(|h| h.screen_has("got:")),
        "{:?}",
        harness.session.visible_text()
    );
    assert!(
        harness.screen_line("got:]52;c;aGk="),
        "{:?}",
        harness.session.visible_text()
    );
}

#[test]
fn the_snapshot_has_resolved_colours_attributes_cursor_and_selection() {
    let dir = temporary("snapshot");
    let script = r"printf '\033[1;31mred\033[0m \033[4mline\033[0m'; sleep 5";
    let mut harness = Harness::start(command("/bin/sh", &["-c", script], &dir), &dir);
    assert!(harness.wait_until(|h| h.screen_has("red line")));
    let palette = term_session::Palette::default();
    let snapshot = harness.session.snapshot();
    assert_eq!((snapshot.columns, snapshot.lines), (80, 24));
    let first = &snapshot.rows[0][0];
    assert_eq!(first.ch, 'r');
    assert_eq!(first.fg, palette.ansi[9], "bold red is drawn bright");
    assert!(first.flags.contains(CellFlags::BOLD));
    assert!(snapshot.rows[0][4].flags.contains(CellFlags::UNDERLINE));
    assert_eq!(snapshot.rows[0][3].bg, palette.background);
    assert_eq!(snapshot.cursor.point, Point { line: 0, column: 8 });
    assert!(snapshot.cursor.visible);
    assert_eq!(snapshot.damage, term_session::Damage::Full);

    harness
        .session
        .select(SelectionKind::Word, Point { line: 0, column: 5 }, CellSide::Left);
    assert_eq!(harness.session.selected_text().as_deref(), Some("line"));
    let selection = harness.session.snapshot().selection.expect("a visible selection");
    assert_eq!((selection.start.column, selection.end.column), (4, 7));
    harness
        .session
        .select(SelectionKind::Line, Point { line: 0, column: 0 }, CellSide::Left);
    assert_eq!(harness.session.selected_text().as_deref(), Some("red line\n"));
    harness.session.clear_selection();
    assert_eq!(harness.session.selected_text(), None);
}

#[test]
fn a_resize_reaches_the_program() {
    let dir = temporary("resize");
    let mut harness = interactive_sh(&dir);
    harness.session.resize(Size {
        columns: 100,
        lines: 30,
        cell_width: 9,
        cell_height: 18,
    });
    harness.session.write("stty size\n");
    assert!(
        harness.wait_until(|h| h.screen_line("30 100")),
        "{:?}",
        harness.session.visible_text()
    );
    assert_eq!(harness.session.snapshot().columns, 100);
}

#[test]
fn an_agent_in_front_gets_ctrl_v_and_a_shell_gets_the_images_path() {
    let dir = temporary("agent");
    let agent = dir.join("claude");
    std::os::unix::fs::symlink("/bin/sleep", &agent).unwrap();
    let images = dir.join("images");
    let wide = Options {
        size: Size {
            columns: 300,
            ..Size::default()
        },
        ..command("/bin/sh", &["-i"], &dir)
    };
    let mut harness = interactive_sh_with(wide, &dir);

    assert!(harness.wait_until(|h| h.session.foreground() == Foreground::Shell("sh".into())));
    let action = harness.session.paste_image(b"png", &images).unwrap();
    let PasteAction::InsertPath(path) = action else {
        panic!("a shell gets a path, not {action:?}");
    };
    let name = path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        harness.wait_until(|h| h.screen_has(&name)),
        "{:?}",
        harness.session.visible_text()
    );
    harness.session.key(&Key::text('u').with(term_session::Modifiers {
        control: true,
        ..Default::default()
    }));

    harness.session.write(format!("{} 30\n", agent.display()));
    assert!(harness.wait_until(|h| h.session.foreground() == Foreground::Agent(Agent::Claude)));
    assert_eq!(
        harness.session.paste_image(b"png", &images).unwrap(),
        PasteAction::SendCtrlV
    );
}

#[test]
fn zsh_integration_marks_commands_and_still_reads_the_users_startup_files() {
    let home = temporary("zsh-home");
    std::fs::write(home.join(".zshenv"), "export MARK_ENV=1\n").unwrap();
    std::fs::write(home.join(".zprofile"), "export MARK_PROFILE=1\n").unwrap();
    std::fs::write(
        home.join(".zshrc"),
        "export MARK_RC=1\nPS1='%# '\nprecmd() { USER_PRECMD=1 }\n",
    )
    .unwrap();
    let scripts = temporary("zsh-scripts");
    let options = Options {
        program: term_session::Program::LoginShell("/bin/zsh".into()),
        shell_integration: Some(scripts.to_path_buf()),
        ..Options::new(&*home)
    };
    let mut harness = Harness::start(options, &home);
    assert!(
        harness.wait_until(|h| h.saw(&Event::Prompt)),
        "{:?}",
        harness.session.visible_text()
    );
    harness
        .session
        .write("echo \"marks:$MARK_ENV$MARK_PROFILE$MARK_RC$USER_PRECMD:${ZDOTDIR-unset}\"; (exit 3)\n");
    assert!(
        harness.wait_until(|h| h.saw(&Event::CommandFinished { exit_code: Some(3) })),
        "{:?}",
        harness.events
    );
    assert!(
        harness.screen_line("marks:1111:unset"),
        "{:?}",
        harness.session.visible_text()
    );
    let zsh_line = "echo \"marks:$MARK_ENV$MARK_PROFILE$MARK_RC$USER_PRECMD:${ZDOTDIR-unset}\"; (exit 3)";
    assert!(
        harness.saw(&Event::CommandStarted {
            command: Some(zsh_line.into())
        }),
        "{:?}",
        harness.events
    );
    let cwd = harness.events.iter().find_map(|event| match event {
        Event::WorkingDirectory(directory) => Some(directory.clone()),
        _ => None,
    });
    let cwd = cwd.expect("zsh reports its directory");
    assert_eq!(cwd.path, *home);
    assert!(cwd.is_local());
}

fn bash_integration(bash: &str) {
    let home = temporary("bash-home");
    std::fs::write(home.join(".bash_profile"), "export MARK_PROFILE=1\n. ~/.bashrc\n").unwrap();
    let rc = "export MARK_RC=1\nPS1='$ '\nPROMPT_COMMAND='USER_PC=$((USER_PC+1))'\ntrap 'USER_TRAP=$((USER_TRAP+1))' DEBUG\n";
    std::fs::write(home.join(".bashrc"), rc).unwrap();
    let scripts = temporary("bash-scripts");
    let options = Options {
        program: term_session::Program::LoginShell(bash.into()),
        shell_integration: Some(scripts.to_path_buf()),
        ..Options::new(&*home)
    };
    let mut harness = Harness::start(options, &home);
    assert!(
        harness.wait_until(|h| h.saw(&Event::Prompt)),
        "{:?}",
        harness.session.visible_text()
    );
    harness.session.write("trap_before=$USER_TRAP; pc_before=$USER_PC\n");
    harness.session.write(
        "echo \"marks:$MARK_PROFILE$MARK_RC:$((USER_TRAP > trap_before)):$((USER_PC > pc_before))\"; (exit 4)\n",
    );
    assert!(
        harness.wait_until(|h| h.saw(&Event::CommandFinished { exit_code: Some(4) })),
        "{:?}",
        harness.events
    );
    assert!(
        harness.screen_line("marks:11:1:1"),
        "{:?}",
        harness.session.visible_text()
    );
    assert!(
        harness
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandStarted { command: Some(line) } if line.contains("marks:"))),
        "{:?}",
        harness.events
    );
    harness.events.clear();
    harness.session.write("\n");
    assert!(harness.wait_until(|h| h.saw(&Event::Prompt)));
    assert!(
        !harness
            .events
            .iter()
            .any(|event| matches!(event, Event::CommandFinished { .. })),
        "an empty line is no command: {:?}",
        harness.events
    );
    assert!(
        harness
            .events
            .iter()
            .any(|event| matches!(event, Event::WorkingDirectory(d) if d.path == *home))
    );
}

#[test]
fn bash_3_integration_marks_commands_and_keeps_the_users_prompt_command_and_debug_trap() {
    bash_integration("/bin/bash");
}

#[test]
fn bash_5_integration_marks_commands_and_keeps_the_users_prompt_command_and_debug_trap() {
    let bash = "/opt/homebrew/bin/bash";
    if !std::path::Path::new(bash).exists() {
        eprintln!("skipped: no {bash}");
        return;
    }
    bash_integration(bash);
}

/// A program that switches on mouse reports in SGR, focus reports and bracketed paste, then shows its input lines
/// with control characters made visible.
fn reporting_cat(dir: &std::path::Path) -> Harness {
    let script = r"printf '\033[?1000h\033[?1006h\033[?1004h\033[?2004hready\n'; stty -echo; exec cat -v";
    let mut harness = Harness::start(command("/bin/sh", &["-c", script], dir), dir);
    assert!(harness.wait_until(|h| h.screen_line("ready") && h.session.input_modes().bracketed_paste));
    harness
}

#[test]
fn a_program_that_asked_for_them_gets_mouse_focus_and_paste_reports() {
    let dir = temporary("reports");
    let mut harness = reporting_cat(&dir);
    assert!(harness.session.is_running());
    assert!(harness.session.pid() > 0);
    let modes = harness.session.input_modes();
    assert!(modes.focus_reporting && modes.mouse != term_session::input::MouseTracking::Off);
    let click = term_session::MouseEvent {
        action: term_session::MouseAction::Press(term_session::MouseButton::Left),
        line: 1,
        column: 2,
        modifiers: term_session::Modifiers::default(),
    };
    assert!(harness.session.mouse(&click), "the program takes the click");
    harness.session.focus(true);
    harness
        .session
        .wheel(1, Point { line: 0, column: 0 }, term_session::Modifiers::default());
    harness.session.paste("hi");
    harness.session.write("\n");
    assert!(
        harness.wait_until(|h| h.screen_has("^[[<0;3;2M^[[I^[[<64;1;1M^[[200~hi^[[201~")),
        "{:?}",
        harness.session.visible_text()
    );
    harness.session.paste_paths(&[dir.join("a file.txt")]);
    harness.session.write("\n");
    let quoted = format!("^[[200~'{}'^[[201~", dir.join("a file.txt").display());
    assert!(
        harness.wait_until(|h| h.session.visible_text().concat().contains(&quoted)),
        "{quoted} in {:?}",
        harness.session.visible_text()
    );
}

#[test]
fn the_history_scrolls_and_typing_returns_to_the_live_screen() {
    let dir = temporary("scroll");
    let mut harness = interactive_sh(&dir);
    harness
        .session
        .write("i=0; while [ $i -lt 60 ]; do i=$((i+1)); echo row$i; done; echo done\n");
    assert!(harness.wait_until(|h| h.screen_line("done") && h.session.snapshot().history > 30));
    harness.session.scroll(term_session::Scroll::Top);
    let top = harness.session.snapshot();
    assert_eq!(top.display_offset, top.history);
    assert!(harness.screen_has("row1"), "{:?}", harness.session.visible_text());
    harness.session.scroll(term_session::Scroll::PageDown);
    let page = harness.session.snapshot().display_offset;
    assert_eq!(page, top.history.saturating_sub(top.lines));
    harness.session.scroll(term_session::Scroll::Lines(-2));
    assert_eq!(harness.session.snapshot().display_offset, page - 2);
    harness.session.scroll(term_session::Scroll::PageUp);
    assert_eq!(
        harness.session.snapshot().display_offset,
        (page - 2 + top.lines).min(top.history)
    );

    harness
        .session
        .select(SelectionKind::Simple, Point { line: 0, column: 0 }, CellSide::Left);
    let first = harness.session.visible_text()[0].clone();
    harness
        .session
        .extend_selection(Point { line: 0, column: 2 }, CellSide::Right);
    assert_eq!(harness.session.selected_text().as_deref(), Some(&first[..3]));

    harness.session.scroll(term_session::Scroll::Bottom);
    assert_eq!(harness.session.snapshot().display_offset, 0);
    harness.session.scroll(term_session::Scroll::Lines(5));
    assert!(harness.session.key(&Key::text('x')));
    assert_eq!(
        harness.session.snapshot().display_offset,
        0,
        "typing shows the live screen"
    );
    assert!(!harness.session.key(&Key::named(NamedKey::Modifier(
        term_session::input::ModifierKey::Shift,
        term_session::input::Side::Left
    ))));
    harness.session.scroll(term_session::Scroll::Lines(5));
    harness
        .session
        .wheel(-5, Point { line: 0, column: 0 }, term_session::Modifiers::default());
    assert_eq!(
        harness.session.snapshot().display_offset,
        0,
        "without mouse reports the wheel scrolls"
    );
}

#[test]
fn a_new_palette_colours_the_next_snapshot() {
    let dir = temporary("palette");
    let mut harness = Harness::start(command("/bin/sh", &["-c", "printf 'plain'; sleep 5"], &dir), &dir);
    assert!(harness.wait_until(|h| h.screen_has("plain")));
    let palette = term_session::Palette {
        foreground: term_session::Rgb { r: 1, g: 2, b: 3 },
        background: term_session::Rgb { r: 250, g: 240, b: 230 },
        ..term_session::Palette::default()
    };
    harness.session.set_palette(palette.clone());
    let snapshot = harness.session.snapshot();
    assert_eq!(snapshot.damage, term_session::Damage::Full);
    assert_eq!(
        (snapshot.rows[0][0].fg, snapshot.background),
        (palette.foreground, palette.background)
    );
}

#[test]
fn queries_are_answered_on_the_programs_input_and_an_unfinished_synchronized_update_still_shows() {
    let dir = temporary("queries");
    // The terminal echoes the answers it writes to the program's input, so they show on the screen.
    let script = r"printf '\033]4;1;rgb:12/34/56\007\033]4;1;?\007\033]10;?\007\033[6n\033[14t\033[22t\033[23t'; sleep 1; printf '\033[?2026hsynced'; sleep 1; kill -TERM $$";
    let mut harness = Harness::start(command("/bin/sh", &["-c", script], &dir), &dir);
    let text = |h: &Harness| h.session.visible_text().concat();
    assert!(
        harness.wait_until(|h| text(h).contains("^[[4;") && text(h).contains("synced")),
        "{:?}",
        harness.session.visible_text()
    );
    let screen = text(&harness);
    assert!(
        screen.contains("^[]4;1;rgb:1212/3434/5656^G"),
        "the program's own colour: {screen}"
    );
    let foreground = term_session::Palette::default().foreground;
    let expected = format!(
        "^[]10;rgb:{0:02x}{0:02x}/{1:02x}{1:02x}/{2:02x}{2:02x}^G",
        foreground.r, foreground.g, foreground.b
    );
    assert!(
        screen.contains(&expected),
        "the palette's text colour {expected}: {screen}"
    );
    assert!(
        screen.contains("R^[[4;"),
        "the cursor position comes before the size: {screen}"
    );
    assert!(
        harness.wait_until(|h| h.saw(&Event::Exited(Ending::Signal(15)))),
        "{:?}",
        harness.events
    );
    assert!(
        harness.saw(&Event::Title(None)),
        "popping the pushed empty title resets it: {:?}",
        harness.events
    );
}

/// A renderer that counts the frames it was told of.
#[derive(Default)]
struct Counting(std::sync::atomic::AtomicUsize);

impl term_session::FrameSink for Counting {
    fn frame_ready(&self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

#[test]
fn the_reader_thread_builds_frames_for_the_renderer_and_the_next_once_one_is_taken() {
    let home = temporary("frames");
    let options = Options {
        program: term_session::Program::Command {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "printf one; read _; printf two; sleep 5".into()],
        },
        shell_integration: None,
        ..Options::new(&*home)
    };
    let mut harness = Harness::start(options, &home);
    let sink = std::sync::Arc::new(Counting::default());
    let source = harness.session.render_to(sink.clone());
    assert!(
        sink.0.load(std::sync::atomic::Ordering::SeqCst) >= 1,
        "a first frame at once"
    );
    let deadline = Instant::now() + common::PATIENCE;
    let mut seen = String::new();
    while !seen.contains("one") && Instant::now() < deadline {
        if let Some(frame) = source.take() {
            seen = frame.rows.iter().flatten().map(|cell| cell.ch).collect();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(seen.contains("one"), "{seen:?}");
    assert!(source.take().is_none(), "a frame is taken once");
    harness.session.write("\n");
    while !seen.contains("two") && Instant::now() < deadline {
        if let Some(frame) = source.take() {
            seen = frame.rows.iter().flatten().map(|cell| cell.ch).collect();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(seen.contains("two"), "{seen:?}");
    assert!(harness.session.latest().is_some());
    let _ = &mut harness;
}

#[test]
fn replacing_a_renderer_keeps_the_terminal_and_invalidates_the_old_frame_source() {
    let dir = temporary("replace-renderer");
    let mut harness = interactive_sh(&dir);
    let old_sink = std::sync::Arc::new(Counting::default());
    let old = harness.session.render_to(old_sink.clone());
    assert!(old.latest().is_some());

    harness.session.detach_renderer();
    assert!(harness.session.latest().is_none());
    harness.session.write("printf 'detached %s\\n' output\n");
    assert!(harness.wait_until(|h| h.screen_line("detached output")));
    assert!(harness.session.latest().is_none());

    let new = harness
        .session
        .render_to_when_ready(std::sync::Arc::new(Counting::default()));
    assert!(harness.wait_until(|_| new.latest().is_some()));
    assert!(old.latest().is_none());
    assert!(old.take().is_none());
    let frame = new.take().expect("the old source cannot take the new frame");
    let text: String = frame.rows.iter().flatten().map(|cell| cell.ch).collect();
    assert!(text.contains("detached output"), "{text:?}");
}

#[test]
fn attaching_to_an_exited_terminal_announces_its_last_frame_to_the_view() {
    let dir = temporary("redraw-after-exit");
    let mut harness = Harness::start(command("/usr/bin/printf", &["last frame"], &dir), &dir);
    assert!(harness.wait_until(|h| h.saw(&Event::Exited(Ending::Code(0)))));
    harness.events.clear();

    let source = harness
        .session
        .render_to_when_ready(std::sync::Arc::new(Counting::default()));
    let frame = source.take().expect("the reader has already stopped");
    let text: String = frame.rows.iter().flatten().map(|cell| cell.ch).collect();
    assert!(text.contains("last frame"), "{text:?}");
    assert!(harness.wait_until(|h| h.saw(&Event::Redraw)));
}

#[test]
fn queued_resize_reaches_the_grid_and_pty_at_the_latest_size() {
    let dir = temporary("queued-resize");
    let mut harness = interactive_sh(&dir);
    for columns in [96, 112, 127] {
        assert!(harness.session.request_resize(Size {
            columns,
            lines: 32,
            ..Size::default()
        }));
    }
    assert!(harness.wait_until(|h| h.session.snapshot().columns == 127));
    harness.session.write("stty size\n");
    assert!(harness.wait_until(|h| h.screen_line("32 127")));
}

#[test]
fn restore_reads_the_bounded_tail_before_the_cursor() {
    let dir = temporary("restore-tail");
    let mut harness = Harness::start(
        command("/usr/bin/printf", &["one\r\ntwo\r\nthree\r\n\r\nprompt"], &dir),
        &dir,
    );
    assert!(harness.wait_until(|h| !h.session.is_running()));
    assert_eq!(harness.session.restore_rows(2, 1024), ["two", "three"]);
    assert_eq!(harness.session.restore_rows(100, 6), ["three"]);
    assert!(harness.session.restore_rows(100, 5).is_empty());
}
