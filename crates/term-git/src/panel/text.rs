//! The words of the Git panel for git's errors and for how long ago a commit was made.

use std::path::PathBuf;

use crate::Error;
use splat_i18n::{t, tf};

/// What went wrong in the panel's words, and what git or a hook printed when there is more to read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The sentence the panel shows.
    pub message: String,
    /// What git or a hook printed, when there is more to read.
    pub output: Option<String>,
    /// The output is a hook's, which the panel shows at once.
    pub hook: bool,
    /// It is about conflicts, so it goes once none are left.
    pub conflicts: bool,
}

impl Problem {
    /// A problem that is only `message`.
    pub fn plain(message: impl Into<String>) -> Problem {
        Problem {
            message: message.into(),
            output: None,
            hook: false,
            conflicts: false,
        }
    }

    fn with(message: impl Into<String>, output: &str) -> Problem {
        Problem {
            message: message.into(),
            output: (!output.trim().is_empty()).then(|| output.trim_end().to_string()),
            hook: false,
            conflicts: false,
        }
    }
}

/// What `error` means for the user, in the panel's words.
pub fn problem(error: &Error) -> Problem {
    let mut problem = described(error);
    problem.conflicts = matches!(error, Error::Conflicts { .. });
    problem
}

impl term_base::explain::Explain for Error {
    fn explain(&self) -> String {
        described(self).message
    }
}

fn described(error: &Error) -> Problem {
    match error {
        Error::GitMissing => Problem::plain(t(
            "Git ist nicht installiert. Die Command Line Tools bringen es mit: „xcode-select --install“.",
        )),
        Error::NotARepository { .. } => Problem::plain(t("Der Ordner ist kein Git-Repository.")),
        Error::BareRepository { .. } => Problem::plain(t("Das Repository hat keine Arbeitskopie.")),
        Error::UnsafeRepository { output, .. } => Problem::with(
            t("Git lehnt das Repository ab, weil es einem anderen Benutzer gehört (safe.directory)."),
            output,
        ),
        Error::AuthFailed { output } => Problem::with(t("Die Anmeldung beim Remote ist fehlgeschlagen."), output),
        Error::Network { output } => Problem::with(t("Der Remote ist nicht erreichbar."), output),
        Error::Conflicts { paths, output } if paths.len() == 1 => Problem::with(
            t("Ein Konflikt. Löse ihn im Editor und markiere die Datei dann als gelöst."),
            output,
        ),
        Error::Conflicts { paths, output } => Problem::with(
            tf(
                "Konflikte in {} Dateien. Löse sie im Editor und markiere die Dateien dann als gelöst.",
                &[&paths.len().to_string()],
            ),
            output,
        ),
        Error::LocalChanges { paths, output } => Problem::with(local_changes(paths), output),
        Error::NothingToCommit => Problem::plain(t("Nichts vorgemerkt.")),
        Error::EmptyMessage => Problem::plain(t("Die Commit-Nachricht fehlt.")),
        Error::HookFailed { hook, output } => Problem {
            message: tf("Der Hook „{}“ hat abgelehnt.", &[hook]),
            output: Some(output.trim_end().to_string()),
            hook: true,
            conflicts: false,
        },
        Error::NotFastForward { output } => Problem::with(
            t("Lokaler und entfernter Branch sind auseinandergelaufen; ein Fast-Forward geht nicht."),
            output,
        ),
        Error::NoUpstream { output } => Problem::with(t("Der Branch hat keinen Upstream."), output),
        Error::PushRejected { output, .. } => Problem::with(
            t("Der Remote hat den Push abgelehnt. Hol zuerst seine neuen Commits (Pull)."),
            output,
        ),
        Error::NotMerged { branch, output } => Problem::with(tf("„{}“ ist nicht zusammengeführt.", &[branch]), output),
        Error::PatchDoesNotApply { output } => Problem::with(
            t("Die Datei hat sich inzwischen geändert; der Abschnitt passt nicht mehr."),
            output,
        ),
        Error::Binary { .. } => Problem::plain(t("Eine Binärdatei hat keine Abschnitte.")),
        Error::Untracked { .. } => Problem::plain(t("Nicht versionierte Dateien kommen in den Papierkorb.")),
        Error::InvalidName { name } => Problem::plain(tf("„{}“ ist kein gültiger Branch-Name.", &[name])),
        Error::OutsideFolder { path } => Problem::plain(tf(
            "„{}“ liegt außerhalb des Repositorys.",
            &[&path.display().to_string()],
        )),
        Error::StashMoved => Problem::plain(t(
            "Der Stash hat sich inzwischen geändert. Sieh die Liste noch einmal durch und versuch es erneut.",
        )),
        Error::Cancelled => Problem::plain(t("Abgebrochen.")),
        Error::TimedOut => Problem::plain(t("Git hat zu lange gebraucht und wurde beendet.")),
        Error::OutputTooLarge => Problem::plain(t("Git hat mehr ausgegeben, als die App liest.")),
        Error::Failed { .. } | Error::Unexpected { .. } | Error::UncheckedConfig { .. } => {
            Problem::with(t("Git ist fehlgeschlagen."), &error.to_string())
        }
        Error::Io(io) => Problem::with(t("Eine Datei ließ sich nicht lesen oder schreiben."), &io.to_string()),
    }
}

/// Up to three of the files git named, so the message stays one line.
fn local_changes(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        return t("Lokale Änderungen würden überschrieben. Committe oder verwirf sie zuerst.").to_string();
    }
    let mut names: Vec<String> = paths.iter().take(3).map(|p| p.display().to_string()).collect();
    if paths.len() > 3 {
        names.push("…".into());
    }
    tf(
        "Änderungen an {} würden überschrieben. Committe oder verwirf sie zuerst.",
        &[&names.join(", ")],
    )
}

/// How long ago something happened that is `seconds` old, as a commit list says it.
pub fn ago(seconds: i64) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;
    let count = |value: i64| value.to_string();
    if seconds < 60 {
        t("gerade eben").into()
    } else if minutes < 60 {
        tf("vor {} Min.", &[&count(minutes)])
    } else if hours < 24 {
        tf("vor {} Std.", &[&count(hours)])
    } else if days < 2 {
        t("gestern").into()
    } else if days < 7 {
        tf("vor {} Tagen", &[&count(days)])
    } else if days < 14 {
        t("vor einer Woche").into()
    } else if days < 30 {
        tf("vor {} Wochen", &[&count(days / 7)])
    } else if days < 60 {
        t("vor einem Monat").into()
    } else if days < 365 {
        tf("vor {} Monaten", &[&count(days / 30)])
    } else if days < 730 {
        t("vor einem Jahr").into()
    } else {
        tf("vor {} Jahren", &[&count(days / 365)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_fall_into_the_unit_a_person_would_name() {
        const MINUTE: i64 = 60;
        const DAY: i64 = 24 * 60 * MINUTE;
        assert_eq!(ago(-5), "gerade eben");
        assert_eq!(ago(59), "gerade eben");
        assert_eq!(ago(MINUTE), "vor 1 Min.");
        assert_eq!(ago(59 * MINUTE), "vor 59 Min.");
        assert_eq!(ago(60 * MINUTE), "vor 1 Std.");
        assert_eq!(ago(DAY + 1), "gestern");
        assert_eq!(ago(3 * DAY), "vor 3 Tagen");
        assert_eq!(ago(8 * DAY), "vor einer Woche");
        assert_eq!(ago(15 * DAY), "vor 2 Wochen");
        assert_eq!(ago(45 * DAY), "vor einem Monat");
        assert_eq!(ago(100 * DAY), "vor 3 Monaten");
        assert_eq!(ago(400 * DAY), "vor einem Jahr");
        assert_eq!(ago(800 * DAY), "vor 2 Jahren");
    }

    #[test]
    fn a_failed_hook_carries_its_output_and_a_long_list_of_files_is_cut() {
        let hook = problem(&Error::HookFailed {
            hook: "pre-commit".into(),
            output: "lint failed\n".into(),
        });
        assert!(hook.hook);
        assert_eq!(hook.output.as_deref(), Some("lint failed"));
        let paths: Vec<PathBuf> = ["a", "b", "c", "d"].iter().map(PathBuf::from).collect();
        let changes = problem(&Error::LocalChanges {
            paths,
            output: String::new(),
        });
        assert!(changes.message.contains("a, b, c, …"), "{}", changes.message);
        assert_eq!(changes.output, None);
    }

    #[test]
    fn what_git_printed_comes_along_unless_it_is_blank() {
        let output = || "fatal: details\n".to_string();
        let printed = [
            Error::UnsafeRepository {
                path: PathBuf::from("/r"),
                output: output(),
            },
            Error::AuthFailed { output: output() },
            Error::Network { output: output() },
            Error::LocalChanges {
                paths: Vec::new(),
                output: output(),
            },
            Error::NotFastForward { output: output() },
            Error::NoUpstream { output: output() },
            Error::PushRejected {
                refs: Vec::new(),
                output: output(),
            },
            Error::NotMerged {
                branch: "topic".into(),
                output: output(),
            },
            Error::PatchDoesNotApply { output: output() },
        ];
        for error in printed {
            let problem = problem(&error);
            assert_eq!(problem.output.as_deref(), Some("fatal: details"), "{error:?}");
            assert!(
                !problem.message.is_empty() && !problem.hook && !problem.conflicts,
                "{error:?}"
            );
        }
        assert_eq!(problem(&Error::Network { output: " \n".into() }).output, None);
        let failed = problem(&Error::Failed {
            command: "git push".into(),
            code: Some(1),
            output: "denied".into(),
        });
        assert_eq!(failed.output.as_deref(), Some("git push failed (1): denied"));
        let io = problem(&Error::Io(std::io::Error::other("disk full")));
        assert_eq!(io.output.as_deref(), Some("disk full"));
    }

    #[test]
    fn errors_without_output_name_what_they_are_about() {
        let plain = [
            (Error::GitMissing, "xcode-select --install"),
            (
                Error::NotARepository {
                    path: PathBuf::from("/r"),
                },
                "kein Git-Repository",
            ),
            (
                Error::BareRepository {
                    path: PathBuf::from("/r"),
                },
                "keine Arbeitskopie",
            ),
            (Error::NothingToCommit, "Nichts vorgemerkt"),
            (Error::EmptyMessage, "Commit-Nachricht"),
            (
                Error::Binary {
                    path: PathBuf::from("a.png"),
                },
                "Binärdatei",
            ),
            (Error::Untracked { paths: Vec::new() }, "Papierkorb"),
            (Error::InvalidName { name: "a..b".into() }, "„a..b“"),
            (
                Error::OutsideFolder {
                    path: PathBuf::from("/etc/hosts"),
                },
                "„/etc/hosts“",
            ),
            (Error::StashMoved, "Stash"),
            (Error::Cancelled, "Abgebrochen"),
            (Error::TimedOut, "zu lange"),
            (Error::OutputTooLarge, "mehr ausgegeben"),
        ];
        for (error, words) in plain {
            let problem = problem(&error);
            assert!(problem.message.contains(words), "{error:?}: {}", problem.message);
            assert_eq!(problem.output, None, "{error:?}");
        }
    }

    #[test]
    fn conflicts_are_counted_and_marked_so_they_go_once_resolved() {
        let one = problem(&Error::Conflicts {
            paths: vec![PathBuf::from("a")],
            output: String::new(),
        });
        let three = problem(&Error::Conflicts {
            paths: ["a", "b", "c"].iter().map(PathBuf::from).collect(),
            output: String::new(),
        });
        assert!(one.conflicts && three.conflicts);
        assert!(one.message.starts_with("Ein Konflikt"), "{}", one.message);
        assert!(three.message.contains("Konflikte in 3 Dateien"), "{}", three.message);
        let two = problem(&Error::LocalChanges {
            paths: vec![PathBuf::from("a"), PathBuf::from("b")],
            output: String::new(),
        });
        assert!(
            two.message.contains("a, b") && !two.message.contains('…'),
            "{}",
            two.message
        );
    }
}
