//! What the errors of the document model say to the user.

use splat_i18n::{t, tf};
use term_base::explain::Explain;

use crate::{EditError, PositionError, ReadOnly, SaveError};

impl Explain for ReadOnly {
    fn explain(&self) -> String {
        match self {
            ReadOnly::Binary => t("Die Datei enthält Binärdaten und wird nie gesichert."),
            ReadOnly::Undecodable => t("Die Kodierung der Datei ist unbekannt; sie wird nie gesichert."),
            ReadOnly::Protected => t("Die Datei ist schreibgeschützt."),
            ReadOnly::Requested => t("Das Dokument ist schreibgeschützt."),
        }
        .to_string()
    }
}

impl Explain for SaveError {
    fn explain(&self) -> String {
        match self {
            SaveError::NoPath => t("Das Dokument hat noch keine Datei.").to_string(),
            SaveError::ReadOnly(why) => why.explain(),
            SaveError::ChangedOnDisk => t("Die Datei wurde inzwischen außerhalb geändert.").to_string(),
            SaveError::Io(error) => error.explain(),
        }
    }
}

impl Explain for EditError {
    fn explain(&self) -> String {
        match self {
            EditError::ReadOnly(why) => why.explain(),
            EditError::Position(error) => error.explain(),
        }
    }
}

impl Explain for PositionError {
    fn explain(&self) -> String {
        tf("Die Stelle liegt außerhalb des Textes: {}", &[&self.to_string()])
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use super::*;

    #[test]
    fn a_failed_save_says_why_in_words() {
        let full = io::Error::from(io::ErrorKind::StorageFull);
        assert_eq!(
            SaveError::Io(io::Error::from(io::ErrorKind::StorageFull)).explain(),
            full.explain()
        );
        assert!(!SaveError::ReadOnly(ReadOnly::Binary).explain().contains("Binary"));
    }
}
