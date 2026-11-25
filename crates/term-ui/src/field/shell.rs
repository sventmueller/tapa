//! What the shell integration reported about the shell (OSC 633 P), for the suggestions in raw mode and the colours of
//! the input field: whether it draws suggestions of its own, its `PATH`, and the names it runs without a program.

use std::path::Path;

use term_session::ShellReport;
use term_suggest::Runnable;

/// The shell of one session, as far as it reported.
#[derive(Debug, Default)]
pub(crate) struct Shell {
    /// The shell draws suggestions of its own, as fish and zsh-autosuggestions do, so the terminal draws none.
    pub(crate) own_suggestions: bool,
    /// The shell's folder is on another machine, whose programs cannot be looked for here.
    pub(crate) remote: bool,
    path: Option<String>,
    names: Option<String>,
    runnable: Option<Runnable>,
}

impl Shell {
    pub(crate) fn report(&mut self, report: ShellReport) {
        match report {
            ShellReport::OwnSuggestions(own) => self.own_suggestions = own,
            ShellReport::Path(path) => {
                self.path = Some(path);
                self.runnable = None;
            }
            ShellReport::Names(names) => {
                self.names = Some(names);
                self.runnable = None;
            }
            ShellReport::MoreNames(more) => {
                let names = self.names.get_or_insert_with(String::new);
                names.push(' ');
                names.push_str(&more);
                self.runnable = None;
            }
        }
    }

    /// Whether the shell runs `word` as a command in `folder`; none before it reported its names and `PATH`, and for
    /// a shell on another machine.
    pub(crate) fn knows(&mut self, word: &str, folder: &Path, home: &Path) -> Option<bool> {
        if self.remote {
            return None;
        }
        let (Some(names), Some(path)) = (&self.names, &self.path) else {
            return None;
        };
        let runnable = self.runnable.get_or_insert_with(|| Runnable::new(names, path));
        Some(runnable.knows(word, folder, home))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_in_parts_join_and_nothing_is_known_before_both_reports() {
        let dir = std::env::temp_dir();
        let mut shell = Shell::default();
        assert_eq!(shell.knows("ll", &dir, &dir), None);
        shell.report(ShellReport::Names("gst".into()));
        shell.report(ShellReport::MoreNames("ll".into()));
        assert_eq!(shell.knows("ll", &dir, &dir), None, "no PATH yet");
        shell.report(ShellReport::Path("/usr/bin:/bin".into()));
        assert_eq!(shell.knows("ll", &dir, &dir), Some(true));
        assert_eq!(shell.knows("ls", &dir, &dir), Some(true));
        assert_eq!(shell.knows("no-such-tool", &dir, &dir), Some(false));
        shell.remote = true;
        assert_eq!(shell.knows("no-such-tool", &dir, &dir), None);
    }
}
