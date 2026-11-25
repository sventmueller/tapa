//! What went wrong, in the user's language, for alerts, sheets and status lines. An error's `Debug` form is for the
//! system log; [`Explain`] is what the user reads, and every error that reaches the user has it.
//!
//! ```
//! use term_base::explain::Explain;
//!
//! let full = std::io::Error::from(std::io::ErrorKind::StorageFull);
//! assert!(!full.explain().contains("StorageFull"));
//! ```

use std::io;

use splat_i18n::t;

/// An error that can say what went wrong in a sentence for the user.
pub trait Explain {
    /// What went wrong, in the user's language.
    fn explain(&self) -> String;
}

/// An error already put in the user's words, for code that only passes it on to be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Explained(pub String);

impl Explained {
    /// What `error` says to the user.
    pub fn of(error: &(impl Explain + ?Sized)) -> Explained {
        Explained(error.explain())
    }
}

impl Explain for Explained {
    fn explain(&self) -> String {
        self.0.clone()
    }
}

impl std::fmt::Display for Explained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Explained {}

impl Explain for io::Error {
    fn explain(&self) -> String {
        let known = match self.kind() {
            io::ErrorKind::NotFound => t("Die Datei oder der Ordner existiert nicht."),
            io::ErrorKind::PermissionDenied => t("Dafür fehlt die Berechtigung."),
            io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded => t("Der Datenträger ist voll."),
            io::ErrorKind::ReadOnlyFilesystem => t("Der Datenträger ist schreibgeschützt."),
            io::ErrorKind::AlreadyExists => t("Es gibt dort schon eine Datei dieses Namens."),
            io::ErrorKind::IsADirectory => t("Das ist ein Ordner, keine Datei."),
            io::ErrorKind::NotADirectory => t("Ein Teil des Pfads ist kein Ordner."),
            io::ErrorKind::DirectoryNotEmpty => t("Der Ordner ist nicht leer."),
            io::ErrorKind::TimedOut => t("Der Vorgang hat zu lange gedauert."),
            io::ErrorKind::ConnectionRefused => t("Die Gegenstelle hat die Verbindung abgelehnt."),
            io::ErrorKind::FileTooLarge => t("Die Datei ist zu groß."),
            io::ErrorKind::InvalidFilename => t("Der Dateiname ist ungültig."),
            io::ErrorKind::ResourceBusy => t("Die Datei ist gerade belegt."),
            _ => return self.to_string(),
        };
        known.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_kinds_say_what_happened_without_the_error_type() {
        for kind in [
            io::ErrorKind::NotFound,
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::StorageFull,
        ] {
            let text = io::Error::from(kind).explain();
            assert!(!text.is_empty() && !text.contains(&format!("{kind:?}")), "{text}");
        }
    }

    #[test]
    fn other_errors_keep_the_systems_words_not_their_debug_form() {
        // ENODEV, which has no kind of its own.
        let text = io::Error::from_raw_os_error(19).explain();
        assert!(!text.is_empty() && !text.contains("Os {"), "{text}");
    }
}
