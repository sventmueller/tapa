//! Progress reports (OSC 9;4) and notifications (OSC 9, OSC 777) as programs send them for the sidebar and the
//! system's notifications.

/// The most characters of a notification's title kept.
const TITLE_LIMIT: usize = 120;
/// The most characters of a notification's text kept.
const BODY_LIMIT: usize = 500;

/// The progress a program reports with ConEmu's OSC 9;4, which Windows Terminal and Ghostty took up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// How the work goes.
    pub state: ProgressState,
    /// How much is done, in percent, where the program said.
    pub percent: Option<u8>,
}

/// How a program's work goes, as its progress report says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    /// It goes on.
    Normal,
    /// It ran into an error.
    Error,
    /// It goes on without knowing how far it is.
    Indeterminate,
    /// It is paused.
    Paused,
}

/// A notification a program asked the terminal to show (OSC 9 as iTerm2 reads it, OSC 777 as urxvt does).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    /// The title, which OSC 9 has none of.
    pub title: Option<String>,
    /// The text.
    pub body: String,
}

/// What an OSC 9 string after its `9;` says: a progress report, or `None` inside to end one; a notification; or
/// nothing for another of ConEmu's commands.
pub(super) fn osc9(rest: &[u8]) -> Option<Osc9> {
    let command = rest.split(|&b| b == b';').next().unwrap_or_default();
    let conemu = std::str::from_utf8(command)
        .ok()
        .and_then(|command| command.parse::<u8>().ok())
        .is_some_and(|command| (1..=12).contains(&command));
    if !conemu {
        let body = clean(rest, BODY_LIMIT);
        return (!body.is_empty()).then_some(Osc9::Notification(Notification { title: None, body }));
    }
    if command != b"4" {
        return None;
    }
    let mut parts = rest.split(|&b| b == b';').skip(1);
    let number = |part: Option<&[u8]>| {
        part.and_then(|part| std::str::from_utf8(part).ok())
            .and_then(|part| part.trim().parse::<u32>().ok())
    };
    let state = number(parts.next()).unwrap_or(0);
    let percent = number(parts.next()).map(|percent| u8::try_from(percent.min(100)).unwrap_or(100));
    let progress = |state, percent| Some(Osc9::Progress(Some(Progress { state, percent })));
    match state {
        0 => Some(Osc9::Progress(None)),
        1 => progress(ProgressState::Normal, Some(percent.unwrap_or(0))),
        2 => progress(ProgressState::Error, percent),
        3 => progress(ProgressState::Indeterminate, None),
        4 => progress(ProgressState::Paused, percent),
        _ => None,
    }
}

/// The notification in an OSC 777 string after its `777;`: `notify;title;body`, the body with any semicolons.
pub(super) fn osc777(rest: &[u8]) -> Option<Notification> {
    let mut parts = rest.splitn(3, |&b| b == b';');
    if parts.next() != Some(b"notify") {
        return None;
    }
    let title = clean(parts.next().unwrap_or_default(), TITLE_LIMIT);
    let body = clean(parts.next().unwrap_or_default(), BODY_LIMIT);
    if title.is_empty() && body.is_empty() {
        return None;
    }
    Some(Notification {
        title: (!title.is_empty()).then_some(title),
        body,
    })
}

/// What OSC 9 reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Osc9 {
    Progress(Option<Progress>),
    Notification(Notification),
}

/// `text` as UTF-8 without control characters, trimmed and cut to `limit` characters.
fn clean(text: &[u8], limit: usize) -> String {
    let text = String::from_utf8_lossy(text);
    let kept: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect();
    kept.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(state: ProgressState, percent: Option<u8>) -> Option<Osc9> {
        Some(Osc9::Progress(Some(Progress { state, percent })))
    }

    #[test]
    fn progress_reports_read_as_conemu_wrote_them() {
        assert_eq!(osc9(b"4;1;42"), progress(ProgressState::Normal, Some(42)));
        assert_eq!(osc9(b"4;1;250"), progress(ProgressState::Normal, Some(100)));
        assert_eq!(osc9(b"4;1"), progress(ProgressState::Normal, Some(0)));
        assert_eq!(osc9(b"4;2;70"), progress(ProgressState::Error, Some(70)));
        assert_eq!(osc9(b"4;2"), progress(ProgressState::Error, None));
        assert_eq!(osc9(b"4;3;10"), progress(ProgressState::Indeterminate, None));
        assert_eq!(osc9(b"4;4;5"), progress(ProgressState::Paused, Some(5)));
        assert_eq!(osc9(b"4;0;0"), Some(Osc9::Progress(None)));
        assert_eq!(osc9(b"4"), Some(Osc9::Progress(None)));
        assert_eq!(osc9(b"4;9;1"), None);
    }

    #[test]
    fn other_conemu_commands_are_no_notification() {
        assert_eq!(osc9(b"1;500"), None);
        assert_eq!(osc9(b"9;/Users/me"), None);
        assert_eq!(osc9(b"12"), None);
    }

    #[test]
    fn any_other_text_is_a_notification_without_its_controls() {
        let notification = |body: &str| {
            Some(Osc9::Notification(Notification {
                title: None,
                body: body.into(),
            }))
        };
        assert_eq!(osc9(b"Build done"), notification("Build done"));
        assert_eq!(
            osc9(b"42 tests passed; 0 failed"),
            notification("42 tests passed; 0 failed")
        );
        assert_eq!(osc9(b"13;x"), notification("13;x"));
        assert_eq!(osc9("bell\u{7}\u{9b}31m ✓".as_bytes()), notification("bell  31m ✓"));
        assert_eq!(osc9(b"   "), None);
        assert_eq!(osc9(&[b'x'; 900]), notification(&"x".repeat(BODY_LIMIT)));
    }

    #[test]
    fn urxvt_notifications_carry_a_title_and_a_text_with_semicolons() {
        assert_eq!(
            osc777(b"notify;Claude;Needs approval; run rm?"),
            Some(Notification {
                title: Some("Claude".into()),
                body: "Needs approval; run rm?".into(),
            })
        );
        assert_eq!(
            osc777(b"notify;;Done"),
            Some(Notification {
                title: None,
                body: "Done".into(),
            })
        );
        assert_eq!(osc777(b"notify"), None);
        assert_eq!(osc777(b"preexec"), None);
    }
}
