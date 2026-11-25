//! Why a control request got no usable answer.

/// Why a control request got no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlError {
    /// The CLI refused it, such as for a subtype it does not know.
    Refused(String),
    /// No answer came in time; the request was withdrawn.
    Timeout,
    /// The CLI ended before it answered.
    Closed,
    /// The CLI stopped reading what the app writes to it, so the request was not sent.
    Congested,
    /// The answer did not have the expected shape.
    Unexpected(String),
}

impl std::fmt::Display for ControlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlError::Refused(why) => write!(f, "refused: {why}"),
            ControlError::Timeout => f.write_str("no answer in time"),
            ControlError::Closed => f.write_str("the CLI ended"),
            ControlError::Congested => f.write_str("the CLI does not read its input"),
            ControlError::Unexpected(why) => write!(f, "unexpected answer: {why}"),
        }
    }
}

impl term_base::explain::Explain for ControlError {
    fn explain(&self) -> String {
        use splat_i18n::{t, tf};
        match self {
            ControlError::Refused(why) => tf("Claude Code lehnt ab: {}", &[why]),
            ControlError::Timeout => t("Claude Code hat nicht rechtzeitig geantwortet.").to_string(),
            ControlError::Closed => t("Claude Code ist beendet.").to_string(),
            ControlError::Congested => t("Claude Code liest seine Eingabe nicht mehr.").to_string(),
            ControlError::Unexpected(why) => tf("Unerwartete Antwort: {}", &[why]),
        }
    }
}

impl std::error::Error for ControlError {}
