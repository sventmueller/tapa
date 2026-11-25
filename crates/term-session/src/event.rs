//! What a terminal session reports to its owner.

use term_base::Ending;
pub use term_vt::WorkingDirectory;
pub use term_vt::scan::{Notification, Progress, ProgressState};

/// A change in a terminal session, delivered on the session's reader thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The screen changed. Sent once until the next `Session::snapshot`, however much output arrives meanwhile.
    Redraw,
    /// The program set the window title, or reset it with `None`; of several in one read of its output, the last.
    Title(Option<String>),
    /// The program rang the bell, once for all its bells in one read of its output.
    Bell,
    /// The program asked to put `text` on a clipboard (OSC 52); of several for one clipboard in one read of its
    /// output, the last, unless it also asked to read a clipboard in that read.
    ClipboardStore {
        /// Which clipboard.
        clipboard: Clipboard,
        /// What to put there.
        text: String,
    },
    /// The program asked to read a clipboard (OSC 52); only sent when the session allows it. The answer goes to
    /// `Session::answer_clipboard`.
    ClipboardLoad(Clipboard),
    /// The shell reported its working directory (OSC 7).
    WorkingDirectory(WorkingDirectory),
    /// The shell shows its prompt (OSC 133 A).
    Prompt,
    /// The prompt ended and the user's input begins (OSC 133 B).
    InputStarted,
    /// The shell started a command line (OSC 133 C), with its text where the integration sent it.
    CommandStarted {
        /// The command line.
        command: Option<String>,
    },
    /// The command line ended (OSC 133 D), with its exit code when the shell reported one.
    CommandFinished {
        /// The exit code.
        exit_code: Option<i32>,
    },
    /// The output announced a local web server, such as `http://localhost:5173/`; each address once per session.
    DevServer(String),
    /// A program reported its progress (OSC 9;4), or `None` that it is done.
    Progress(Option<Progress>),
    /// A program asked for a notification (OSC 9, OSC 777).
    Notification(Notification),
    /// The program in the terminal ended.
    Exited(Ending),
}

/// One of the clipboards OSC 52 addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Clipboard {
    /// The general pasteboard.
    Clipboard,
    /// The X11 primary selection, which a Mac does not have; the app decides what to do with it.
    Selection,
}

impl From<term_vt::Mark> for Event {
    fn from(mark: term_vt::Mark) -> Event {
        match mark {
            term_vt::Mark::Prompt => Event::Prompt,
            term_vt::Mark::Input => Event::InputStarted,
            term_vt::Mark::CommandStarted { command } => Event::CommandStarted { command },
            term_vt::Mark::CommandFinished { exit_code } => Event::CommandFinished { exit_code },
            term_vt::Mark::WorkingDirectory(directory) => Event::WorkingDirectory(directory),
            term_vt::Mark::DevServer(url) => Event::DevServer(url),
            term_vt::Mark::Progress(progress) => Event::Progress(progress),
            term_vt::Mark::Notification(notification) => Event::Notification(notification),
        }
    }
}
