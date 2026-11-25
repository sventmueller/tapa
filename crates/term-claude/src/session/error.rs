//! How a session error reads, for logs and for the user.

use super::SessionError;

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionError::NotFound(path) => write!(f, "{} not found", path.display()),
            SessionError::Start(why) => write!(f, "claude did not start: {why}"),
            SessionError::SignInRequired(why) => write!(f, "sign in by running claude in a terminal: {why}"),
            SessionError::RewindRejected(why) => write!(f, "the rewind was refused: {why}"),
            SessionError::UnknownRequest(id) => write!(f, "no request {id} waits for an answer"),
            SessionError::Closed => f.write_str("the claude process has ended"),
            SessionError::Congested => f.write_str("the claude process does not read its input"),
        }
    }
}

impl term_base::explain::Explain for SessionError {
    fn explain(&self) -> String {
        use splat_i18n::{t, tf};
        match self {
            SessionError::NotFound(path) => tf("{} wurde nicht gefunden.", &[&path.display().to_string()]),
            SessionError::Start(why) => tf("Claude Code ist nicht gestartet: {}", &[why]),
            SessionError::SignInRequired(why) => tf("Melde dich an, indem du claude im Terminal startest: {}", &[why]),
            SessionError::RewindRejected(why) => tf("Claude Code hat das Zurückspulen abgelehnt: {}", &[why]),
            SessionError::UnknownRequest(_) => t("Diese Anfrage wartet nicht mehr auf eine Antwort.").to_string(),
            SessionError::Closed => t("Claude Code ist beendet.").to_string(),
            SessionError::Congested => t("Claude Code liest seine Eingabe nicht mehr.").to_string(),
        }
    }
}

impl std::error::Error for SessionError {}
