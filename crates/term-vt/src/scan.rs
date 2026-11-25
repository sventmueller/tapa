//! A second look at the raw output for what the terminal emulator drops: shell integration marks (OSC 133), the
//! working directory (OSC 7) and announcements of local web servers in plain text.

use std::collections::HashSet;
use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;

use crate::devserver;

mod osc9;

pub use osc9::{Notification, Progress, ProgressState};

/// Longer OSC strings are cut; the ones read here are short.
const OSC_LIMIT: usize = 4096;

/// Longer lines are not searched for server addresses.
const LINE_LIMIT: usize = 2048;

/// How far ahead whole lines without anything to find are looked for at once.
const WINDOW: usize = 4096;

/// The most line ends that pass after such a look found nothing to skip, before the next one.
const MAX_WAIT: u8 = 63;

/// How many server addresses a session remembers to report each only once.
const SERVER_LIMIT: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    EscapeIntermediate,
    Csi,
    Osc,
    /// DCS, SOS, PM and APC strings, which end at ST.
    Ignore,
    IgnoreEscape,
}

/// What the scanner finds in the output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mark {
    /// The shell shows its prompt (OSC 133 A).
    Prompt,
    /// The prompt ended and the user's input begins (OSC 133 B).
    Input,
    /// The shell started a command line (OSC 133 C), with its text where the integration sent it as `cmdline_url`.
    CommandStarted {
        /// The command line.
        command: Option<String>,
    },
    /// The command line ended (OSC 133 D), with its exit code when the shell reported one.
    CommandFinished {
        /// The exit code.
        exit_code: Option<i32>,
    },
    /// The shell reported its working directory (OSC 7).
    WorkingDirectory(WorkingDirectory),
    /// The output announced a local web server, such as `http://localhost:5173/`; each address once per session.
    DevServer(String),
    /// A program reported its progress (OSC 9;4), or `None` that it is done.
    Progress(Option<Progress>),
    /// A program asked for a notification (OSC 9, OSC 777).
    Notification(Notification),
    /// The shell integration told something about the shell itself.
    Shell(ShellReport),
}

/// What Tapa's shell integration reports about the shell, as properties of VS Code's OSC 633 P with keys of its own,
/// which other terminals ignore.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellReport {
    /// The shell shows suggestions of its own while the user types, as fish and zsh-autosuggestions do.
    OwnSuggestions(bool),
    /// The shell's `PATH`.
    Path(String),
    /// The names the shell runs without looking in the `PATH`, separated by spaces: aliases, functions, builtins and
    /// reserved words. A new list starts here.
    Names(String),
    /// More of those names, as the list does not fit one sequence.
    MoreNames(String),
}

/// A working directory as a shell reports it, possibly on another machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkingDirectory {
    /// The host the shell runs on; empty when the shell left it out.
    pub host: String,
    /// The directory.
    pub path: PathBuf,
}

impl WorkingDirectory {
    /// Whether the directory is on this Mac rather than at the other end of an `ssh` session.
    pub fn is_local(&self) -> bool {
        let host = self.host.as_str();
        if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
            return true;
        }
        let local = crate::host::name();
        let short = |name: &str| name.split('.').next().unwrap_or(name).to_ascii_lowercase();
        host.eq_ignore_ascii_case(local) || (!local.is_empty() && short(host) == short(local))
    }
}

/// Reads output alongside the emulator and collects the marks it finds.
#[derive(Debug)]
pub struct Scanner {
    state: State,
    osc: Vec<u8>,
    line: Vec<u8>,
    line_cut: bool,
    /// Line ends to let pass before whole lines are looked for again, and how many it was the last time.
    wait: u8,
    patience: u8,
    marks_seen: bool,
    command_running: bool,
    servers: HashSet<String>,
    events: Vec<Mark>,
}

impl Default for Scanner {
    fn default() -> Scanner {
        Scanner::new()
    }
}

impl Scanner {
    /// A scanner at the start of the output.
    pub fn new() -> Scanner {
        Scanner {
            state: State::Ground,
            osc: Vec::new(),
            line: Vec::new(),
            line_cut: false,
            wait: 0,
            patience: 0,
            marks_seen: false,
            command_running: false,
            servers: HashSet::new(),
            events: Vec::new(),
        }
    }

    /// Reads `bytes`, which may end in the middle of a sequence.
    pub fn feed(&mut self, bytes: &[u8]) {
        let mut at = 0;
        while at < bytes.len() {
            if self.state == State::Ground {
                let rest = &bytes[at..];
                if self.line_cut {
                    let skipped = self.skip_cut(rest);
                    at += skipped;
                    if skipped > 0 {
                        continue;
                    }
                }
                if rest[0] == 0x1b {
                    at += control_sequence(rest).unwrap_or_else(|| {
                        self.state = State::Escape;
                        1
                    });
                    continue;
                }
                match memchr::memchr3(0x1b, b'\n', b'\r', rest) {
                    None => {
                        self.push_text(rest);
                        at = bytes.len();
                    }
                    Some(special) => {
                        self.push_text(&rest[..special]);
                        at += special;
                        if rest[special] != 0x1b {
                            self.end_line();
                            at += 1;
                            at += self.skip_lines(&bytes[at..]);
                        }
                    }
                }
            } else if self.state == State::Csi {
                // Parameters and intermediates matter to no mark; on to the byte that ends or breaks the sequence.
                let rest = &bytes[at..];
                match rest
                    .iter()
                    .position(|&byte| matches!(byte, 0x40..=0x7e | 0x18 | 0x1a | 0x1b))
                {
                    Some(end) => {
                        self.step(rest[end]);
                        at += end + 1;
                    }
                    None => at = bytes.len(),
                }
            } else {
                self.step(bytes[at]);
                at += 1;
            }
        }
    }

    /// In a line too long to be searched, only its end and a string's start matter: how many bytes of `rest` hold
    /// neither. A sequence that is no string returns to the ground or is cut short by the next ESC, so none before
    /// the last one has to be followed.
    fn skip_cut(&mut self, rest: &[u8]) -> usize {
        let region = &rest[..memchr::memchr2(b'\n', b'\r', rest).unwrap_or(rest.len())];
        let mut from = 0;
        let mut last = None;
        while let Some(found) = memchr::memchr(0x1b, &region[from..]) {
            let escape = from + found;
            match region.get(escape + 1) {
                None | Some(b']' | b'P' | b'X' | b'^' | b'_') => return escape,
                Some(_) => {
                    last = Some(escape);
                    from = escape + 1;
                }
            }
        }
        if let Some(escape) = last {
            self.state = State::Escape;
            for &byte in &region[escape + 1..] {
                if self.state == State::Ground {
                    break;
                }
                self.step(byte);
            }
        }
        region.len()
    }

    /// At the start of a line: how many bytes of `rest` are whole lines without an escape and without anything a
    /// server address needs, which report nothing. Looked for less often while there are none.
    fn skip_lines(&mut self, rest: &[u8]) -> usize {
        if self.wait > 0 {
            self.wait -= 1;
            return 0;
        }
        let window = &rest[..rest.len().min(WINDOW)];
        let plain = &window[..memchr::memchr(0x1b, window).unwrap_or(window.len())];
        let skipped = memchr::memrchr2(b'\n', b'\r', plain)
            .filter(|&last| !devserver::may_announce(&plain[..last]))
            .map_or(0, |last| last + 1);
        self.patience = if skipped > 0 {
            0
        } else {
            (self.patience * 2 + 1).min(MAX_WAIT)
        };
        self.wait = self.patience;
        skipped
    }

    /// The events found since the last call.
    pub fn take_events(&mut self) -> Vec<Mark> {
        std::mem::take(&mut self.events)
    }

    fn step(&mut self, byte: u8) {
        // CAN and SUB abort any sequence.
        if matches!(byte, 0x18 | 0x1a) {
            self.state = State::Ground;
            return;
        }
        self.state = match (self.state, byte) {
            (State::Escape, b'[') => State::Csi,
            (State::Escape, b']') => {
                self.osc.clear();
                State::Osc
            }
            (State::Escape, b'P' | b'X' | b'^' | b'_') => State::Ignore,
            (State::Escape | State::EscapeIntermediate, 0x20..=0x2f) => State::EscapeIntermediate,
            (State::Escape | State::EscapeIntermediate | State::Csi, 0x1b) => State::Escape,
            (State::Csi, 0x40..=0x7e) | (State::Escape | State::EscapeIntermediate, _) => State::Ground,
            (State::Csi, _) => State::Csi,
            // Like vte, an ESC ends the OSC string whatever follows; BEL ends it in xterm's older form.
            (State::Osc, 0x07 | 0x1b) => {
                self.dispatch_osc();
                if byte == 0x1b { State::Escape } else { State::Ground }
            }
            (State::Osc, _) => {
                if self.osc.len() < OSC_LIMIT {
                    self.osc.push(byte);
                }
                State::Osc
            }
            (State::Ignore, 0x1b) => State::IgnoreEscape,
            (State::Ignore, _) | (State::IgnoreEscape, 0x1b) => State::Ignore,
            (State::IgnoreEscape, _) | (State::Ground, _) => State::Ground,
        };
    }

    fn push_text(&mut self, text: &[u8]) {
        if self.line_cut || text.is_empty() {
            return;
        }
        if self.line.len() + text.len() > LINE_LIMIT {
            self.line_cut = true;
            self.line.clear();
            return;
        }
        self.line.extend_from_slice(text);
    }

    fn end_line(&mut self) {
        let searched = !self.line.is_empty() && !self.line_cut && (self.command_running || !self.marks_seen);
        if searched {
            for url in devserver::detect(&self.line) {
                let known = self.servers.contains(devserver::identity(&url));
                if !known && self.servers.len() < SERVER_LIMIT {
                    self.servers.insert(devserver::identity(&url).to_string());
                    self.events.push(Mark::DevServer(url));
                }
            }
        }
        self.line.clear();
        self.line_cut = false;
    }

    fn dispatch_osc(&mut self) {
        let osc = std::mem::take(&mut self.osc);
        let (code, rest) = match osc.iter().position(|&b| b == b';') {
            Some(split) => (&osc[..split], &osc[split + 1..]),
            None => (&osc[..], &[][..]),
        };
        match code {
            b"7" => {
                if let Some(directory) = parse_osc7(rest) {
                    self.events.push(Mark::WorkingDirectory(directory));
                }
            }
            b"133" => self.mark(rest),
            b"633" => {
                if let Some(report) = shell_report(rest) {
                    self.events.push(Mark::Shell(report));
                }
            }
            b"9" => match osc9::osc9(rest) {
                Some(osc9::Osc9::Progress(progress)) => self.events.push(Mark::Progress(progress)),
                Some(osc9::Osc9::Notification(notification)) => self.events.push(Mark::Notification(notification)),
                None => {}
            },
            b"777" => {
                if let Some(notification) = osc9::osc777(rest) {
                    self.events.push(Mark::Notification(notification));
                }
            }
            _ => {}
        }
        self.osc = osc;
        self.osc.clear();
    }

    /// FinalTerm's marks: A prompt, B input, C output, D end of the command with its exit code.
    fn mark(&mut self, mark: &[u8]) {
        let Some(&kind) = mark.first() else { return };
        match kind {
            b'A' => {
                self.marks_seen = true;
                self.command_running = false;
                self.events.push(Mark::Prompt);
            }
            b'B' => {
                self.marks_seen = true;
                self.events.push(Mark::Input);
            }
            b'C' => {
                self.marks_seen = true;
                self.command_running = true;
                let command = mark
                    .get(1..)
                    .and_then(|rest| rest.strip_prefix(b";"))
                    .and_then(|rest| {
                        rest.split(|&b| b == b';')
                            .find_map(|option| option.strip_prefix(b"cmdline_url="))
                    })
                    .map(|encoded| String::from_utf8_lossy(&percent_decode(encoded)).into_owned());
                self.events.push(Mark::CommandStarted { command });
            }
            b'D' => {
                self.marks_seen = true;
                self.command_running = false;
                let exit_code = mark
                    .get(1..)
                    .and_then(|rest| rest.strip_prefix(b";"))
                    .and_then(|rest| rest.split(|&b| b == b';').next())
                    .and_then(|code| std::str::from_utf8(code).ok())
                    .and_then(|code| code.parse::<i32>().ok());
                self.events.push(Mark::CommandFinished { exit_code });
            }
            _ => {}
        }
    }
}

/// A property `P;Key=Value` of OSC 633 with one of Tapa's keys; VS Code's escapes `\\` and `\xNN` are undone.
fn shell_report(rest: &[u8]) -> Option<ShellReport> {
    let property = rest.strip_prefix(b"P;")?;
    let split = property.iter().position(|&b| b == b'=')?;
    let (key, value) = (&property[..split], &property[split + 1..]);
    let mut text = Vec::with_capacity(value.len());
    let mut at = 0;
    while at < value.len() {
        match (value[at], value.get(at + 1)) {
            (b'\\', Some(b'\\')) => {
                text.push(b'\\');
                at += 2;
            }
            (b'\\', Some(b'x'))
                if let Some(byte) = value
                    .get(at + 2..at + 4)
                    .and_then(|hex| std::str::from_utf8(hex).ok())
                    .and_then(|hex| u8::from_str_radix(hex, 16).ok()) =>
            {
                text.push(byte);
                at += 4;
            }
            (byte, _) => {
                text.push(byte);
                at += 1;
            }
        }
    }
    let text = String::from_utf8_lossy(&text).into_owned();
    Some(match key {
        b"TapaOwnSuggestions" => ShellReport::OwnSuggestions(text == "1"),
        b"TapaPath" => ShellReport::Path(text),
        b"TapaNames" => ShellReport::Names(text),
        b"TapaMoreNames" => ShellReport::MoreNames(text),
        _ => return None,
    })
}

/// The length of the control sequence at the start of `bytes`, where it is whole and made of parameter and
/// intermediate bytes only: the common case, which holds nothing to find.
fn control_sequence(bytes: &[u8]) -> Option<usize> {
    if bytes.get(1) != Some(&b'[') {
        return None;
    }
    let end = 2 + bytes[2..].iter().position(|byte| !(0x20..=0x3f).contains(byte))?;
    (0x40..=0x7e).contains(&bytes[end]).then_some(end + 1)
}

/// The directory in an OSC 7 URL: `file://host/path` with the path percent-encoded, or kitty's
/// `kitty-shell-cwd://host/path` without encoding.
pub(crate) fn parse_osc7(url: &[u8]) -> Option<WorkingDirectory> {
    let (rest, encoded) = if let Some(rest) = url.strip_prefix(b"file://") {
        (rest, true)
    } else {
        (url.strip_prefix(b"kitty-shell-cwd://")?, false)
    };
    let slash = rest.iter().position(|&b| b == b'/')?;
    let path = &rest[slash..];
    let path = if encoded { percent_decode(path) } else { path.to_vec() };
    Some(WorkingDirectory {
        host: String::from_utf8_lossy(&rest[..slash]).into_owned(),
        path: PathBuf::from(OsString::from_vec(path)),
    })
}

fn percent_decode(text: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(text.len());
    let mut at = 0;
    while at < text.len() {
        let hex = text.get(at + 1..at + 3).and_then(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        });
        match (text[at], hex) {
            (b'%', Some(byte)) => {
                decoded.push(byte);
                at += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                at += 1;
            }
        }
    }
    decoded
}

#[cfg(test)]
mod property_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&[u8]]) -> Vec<Mark> {
        let mut scanner = Scanner::new();
        for chunk in chunks {
            scanner.feed(chunk);
        }
        scanner.take_events()
    }

    fn directory(host: &str, path: &str) -> Mark {
        Mark::WorkingDirectory(WorkingDirectory {
            host: host.into(),
            path: path.into(),
        })
    }

    #[test]
    fn osc_7_reports_the_decoded_directory_with_either_terminator() {
        assert_eq!(
            events(&[b"\x1b]7;file://mac.local/Users/me/My%20Project\x07"]),
            [directory("mac.local", "/Users/me/My Project")]
        );
        assert_eq!(events(&[b"\x1b]7;file:///tmp/%C3%A4\x1b\\"]), [directory("", "/tmp/ä")]);
        assert_eq!(
            events(&[b"\x1b]7;kitty-shell-cwd://h/a%20b\x07"]),
            [directory("h", "/a%20b")]
        );
        assert!(events(&[b"\x1b]7;not a url\x07"]).is_empty());
    }

    #[test]
    fn the_shell_reports_its_suggestions_path_and_names_with_escapes_undone() {
        assert_eq!(
            events(&[
                b"\x1b]633;P;TapaOwnSuggestions=1\x07\x1b]633;P;TapaPath=/opt/homebrew/bin:/usr/bin\x07",
                b"\x1b]633;P;TapaNames=ll gst\x1b\\\x1b]633;P;TapaMoreNames=a\\x3bb c\\\\d\x07",
                b"\x1b]633;P;Cwd=/tmp\x07\x1b]633;P;TapaOwnSuggestions=0\x07",
            ]),
            [
                Mark::Shell(ShellReport::OwnSuggestions(true)),
                Mark::Shell(ShellReport::Path("/opt/homebrew/bin:/usr/bin".into())),
                Mark::Shell(ShellReport::Names("ll gst".into())),
                Mark::Shell(ShellReport::MoreNames("a;b c\\d".into())),
                Mark::Shell(ShellReport::OwnSuggestions(false)),
            ]
        );
    }

    #[test]
    fn progress_and_notifications_arrive_across_reads_with_either_terminator() {
        let found = events(&[
            b"building\r\n\x1b]9;4;1;3",
            b"0\x07\x1b]9;4;0\x1b\\\x1b]9;Bu",
            b"ild done\x07\x1b]777;notify;cargo;3 tests failed\x1b\\\x1b]9;1;100\x07",
        ]);
        assert_eq!(
            found,
            [
                Mark::Progress(Some(Progress {
                    state: ProgressState::Normal,
                    percent: Some(30),
                })),
                Mark::Progress(None),
                Mark::Notification(Notification {
                    title: None,
                    body: "Build done".into(),
                }),
                Mark::Notification(Notification {
                    title: Some("cargo".into()),
                    body: "3 tests failed".into(),
                }),
            ]
        );
    }

    #[test]
    fn a_command_line_comes_with_its_start_when_the_integration_sends_it() {
        assert_eq!(
            events(&[b"\x1b]133;C;cmdline_url=git%20log%20-1\x07"]),
            [Mark::CommandStarted {
                command: Some("git log -1".into())
            }]
        );
    }

    #[test]
    fn osc_133_marks_report_prompts_input_commands_and_exit_codes() {
        let found = events(&[
            b"\x1b]133;A\x07$ \x1b]133;B\x07",
            b"\x1b]133;C\x07out\r\n\x1b]133;D;127\x07",
        ]);
        assert_eq!(
            found,
            [
                Mark::Prompt,
                Mark::Input,
                Mark::CommandStarted { command: None },
                Mark::CommandFinished { exit_code: Some(127) }
            ]
        );
        assert_eq!(
            events(&[b"\x1b]133;D\x07"]),
            [Mark::CommandFinished { exit_code: None }]
        );
        assert_eq!(
            events(&[b"\x1b]133;D;0;aid=1\x1b\\"]),
            [Mark::CommandFinished { exit_code: Some(0) }]
        );
    }

    #[test]
    fn a_sequence_split_across_reads_is_still_recognised() {
        let whole: &[u8] = b"x\x1b]133;D;2\x07y\x1b]7;file:///a\x07";
        for split in 0..whole.len() {
            let (first, second) = whole.split_at(split);
            assert_eq!(
                events(&[first, second]),
                [Mark::CommandFinished { exit_code: Some(2) }, directory("", "/a")],
                "split at {split}"
            );
        }
    }

    #[test]
    fn colours_inside_an_address_do_not_hide_it() {
        let vite = b"  \x1b[32m\xe2\x9e\x9c\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r\n";
        assert_eq!(events(&[vite]), [Mark::DevServer("http://localhost:5173/".into())]);
    }

    #[test]
    fn a_server_is_reported_once_per_session() {
        let line: &[u8] = b"Local: http://localhost:3000/\nLocal: http://localhost:3000\n";
        assert_eq!(
            events(&[line, line]),
            [Mark::DevServer("http://localhost:3000/".into())]
        );
    }

    #[test]
    fn with_shell_marks_only_command_output_is_searched_for_servers() {
        let typed = b"\x1b]133;A\x07$ \x1b]133;B\x07curl http://localhost:8080\r\n";
        let output = b"\x1b]133;C\x07listening on port 9000\r\n\x1b]133;D;0\x07";
        let found = events(&[typed, output]);
        assert_eq!(
            found,
            [
                Mark::Prompt,
                Mark::Input,
                Mark::CommandStarted { command: None },
                Mark::DevServer("http://localhost:9000".into()),
                Mark::CommandFinished { exit_code: Some(0) }
            ]
        );
    }

    #[test]
    fn marks_are_found_in_a_line_too_long_to_search_and_after_it() {
        let mut output = vec![b'x'; LINE_LIMIT + 1];
        output.extend_from_slice(b"\x1b[1m http://localhost:1/ \x1b[38;5;1mcell\x1b(0\x1b\x1b]133;C\x07");
        output.extend_from_slice(b"\x1bPignored\x1b]133;B\x07\x1b\\\x1b[12m");
        output.extend_from_slice(b"\nrest http://localhost:2/\r\n\x1b]133;D;3\x07");
        assert_eq!(
            events(&[&output]),
            [
                Mark::CommandStarted { command: None },
                Mark::DevServer("http://localhost:2/".into()),
                Mark::CommandFinished { exit_code: Some(3) }
            ]
        );
    }

    #[test]
    fn a_server_is_found_among_many_plain_lines() {
        let mut output = b"y\r\n".repeat(3 * WINDOW);
        output.extend_from_slice(b"ready on http://localhost:4000\r\n");
        output.extend(b"z\n".repeat(WINDOW));
        output.extend_from_slice(b"half a line, listening on port 4001");
        assert_eq!(
            events(&[&output, b"\n"]),
            [
                Mark::DevServer("http://localhost:4000".into()),
                Mark::DevServer("http://localhost:4001".into())
            ]
        );
    }

    #[test]
    fn other_strings_and_escapes_are_skipped_and_an_aborted_osc_reports_nothing() {
        let noise = b"\x1bP1$r0m\x1b\\\x1b(B\x1b]0;title\x07\x1b]133;C\x18\x1b[?2004h";
        assert!(events(&[noise]).is_empty());
    }
}
