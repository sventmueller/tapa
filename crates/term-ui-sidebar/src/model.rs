//! What a row of the sidebar and a tab of the tab bar show, decided without AppKit.

use std::path::{Path, PathBuf};
use std::time::Duration;

use splat_i18n::{t, tf};
use term_session::event::{Progress, ProgressState};
use term_ui_icons::Icon;

/// A panel that takes the sidebar's place of the workspaces' list for a workspace (E22): Dango's file list and Git
/// panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Panel {
    /// The files of the base folder.
    Files,
    /// The repository's state.
    Git,
}

impl Panel {
    /// Both panels, in the order of their switches in a row.
    pub const ALL: [Panel; 2] = [Panel::Files, Panel::Git];
}

/// The state of a tab's session, as its dot shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    /// The shell waits for input, nothing ran yet.
    #[default]
    Idle,
    /// A command runs.
    Running,
    /// The last command succeeded.
    Succeeded,
    /// The last command failed.
    Failed,
    /// The program rang the bell or sent a notification while the tab was not in front.
    Attention,
    /// An agent waits for the user's approval (F132).
    Waiting,
    /// The program in the tab ended.
    Ended,
}

impl Status {
    /// How much the status asks of the user, so that where tabs or panes differ the most urgent shows.
    pub fn urgency(self) -> u8 {
        match self {
            Status::Waiting => 6,
            Status::Attention => 5,
            Status::Failed => 4,
            Status::Running => 3,
            Status::Succeeded => 2,
            Status::Ended => 1,
            Status::Idle => 0,
        }
    }

    /// What VoiceOver reads for the status; empty while nothing ran.
    pub fn spoken(self) -> &'static str {
        match self {
            Status::Idle => "",
            Status::Running => t("läuft"),
            Status::Succeeded => t("erfolgreich"),
            Status::Failed => t("fehlgeschlagen"),
            Status::Attention => t("braucht Aufmerksamkeit"),
            Status::Waiting => t("wartet auf Freigabe"),
            Status::Ended => t("beendet"),
        }
    }
}

/// What a tab holds (E20, E22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    /// A terminal with its panes, one session each.
    #[default]
    Terminal,
    /// An agent's session (E21).
    Agent,
    /// A browser session.
    Browser,
    /// An open file.
    File,
}

/// One tab of a workspace, as the tab bar shows it and the workspace's row sums it up.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// The tab's id in its window.
    pub id: u64,
    /// What it holds.
    pub kind: Kind,
    /// The icon of the program in front of its active pane.
    pub icon: Icon,
    /// The icons of the tab's other panes.
    pub others: Vec<Icon>,
    /// The title an agent or program set, if it is worth showing.
    pub title: Option<String>,
    /// The active pane's folder.
    pub folder: PathBuf,
    /// The branch of the folder's repository.
    pub branch: Option<String>,
    /// The last command and how long it ran, or how long it runs so far.
    pub last_command: Option<(String, Duration)>,
    /// The dot.
    pub status: Status,
    /// The progress a program reports (OSC 9;4).
    pub progress: Option<Progress>,
    /// The text of a notification the user has not seen yet (OSC 9, OSC 777).
    pub notice: Option<String>,
}

impl Entry {
    /// The dot of the tab: an agent tab's while it works or waits for the user.
    pub fn mark(&self) -> Option<Mark> {
        match (self.kind, self.status) {
            (Kind::Agent, Status::Waiting) => Some(Mark::Waits),
            (Kind::Agent, Status::Running) => Some(Mark::Works),
            _ => None,
        }
    }
}

/// The dot of a workspace's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// An agent works.
    Works,
    /// An agent waits for the user.
    Waits,
}

impl Mark {
    /// What VoiceOver reads for the dot.
    pub fn spoken(self) -> &'static str {
        match self {
            Mark::Works => t("Arbeitet"),
            Mark::Waits => t("Wartet auf dich"),
        }
    }
}

/// The progress ring of a tab.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ring {
    /// How much of the ring is filled, from 0 to 1; none for work that does not know how far it is.
    pub filled: Option<f64>,
    /// Its colour.
    pub tone: Tone,
}

/// The colour of a progress ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// The work goes on.
    Normal,
    /// It ran into an error.
    Error,
    /// It is paused.
    Paused,
}

impl Entry {
    /// The first line: the title, else the whole folder from `~`, which the row cuts in the middle where it is too
    /// long.
    pub fn heading(&self, home: &Path) -> String {
        self.title.as_deref().map_or_else(
            || tilde(&self.folder.to_string_lossy(), home),
            |title| tilde(title, home),
        )
    }

    /// The second line: the folder below a title; then a notification not seen yet, else the branch and the last
    /// command.
    pub fn detail(&self, home: &Path) -> String {
        let mut parts = Vec::new();
        if self.title.is_some() {
            parts.push(tilde(&self.folder.to_string_lossy(), home));
        }
        if let Some(notice) = &self.notice {
            parts.push(notice.clone());
            return parts.join(" · ");
        }
        if let Some(branch) = &self.branch {
            parts.push(branch.clone());
        }
        if let Some((command, took)) = &self.last_command {
            parts.push(format!("{} · {}", tilde(command, home), duration(*took)));
        }
        parts.join(" · ")
    }

    /// The progress ring, while a program reports progress.
    pub fn ring(&self) -> Option<Ring> {
        let progress = self.progress?;
        let filled = progress.percent.map(|percent| f64::from(percent) / 100.0);
        let (filled, tone) = match progress.state {
            ProgressState::Normal => (filled, Tone::Normal),
            ProgressState::Indeterminate => (None, Tone::Normal),
            ProgressState::Error => (Some(filled.unwrap_or(1.0)), Tone::Error),
            ProgressState::Paused => (Some(filled.unwrap_or(1.0)), Tone::Paused),
        };
        Some(Ring { filled, tone })
    }

    /// What VoiceOver reads for the tab.
    pub fn spoken(&self, home: &Path) -> String {
        let parts = [
            self.heading(home),
            self.detail(home),
            self.status.spoken().to_string(),
            spoken_progress(self.progress),
        ];
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// One row of the sidebar: a workspace with its base folder and its tabs (E20).
#[derive(Debug, Clone, PartialEq)]
pub struct Workspace {
    /// The workspace's id in its window.
    pub id: u64,
    /// Its name, by default the base folder's.
    pub name: String,
    /// The folder its tabs start in.
    pub base: PathBuf,
    /// The branch of the base folder's repository.
    pub branch: Option<String>,
    /// Its tabs in the order of the tab bar.
    pub tabs: Vec<Entry>,
    /// Whether the user trusts it, so that its agents load the project's configuration (E21).
    pub trusted: bool,
    /// Whether it is the window's Terminal, which the user can neither rename nor remove.
    pub fixed: bool,
    /// The panel that shows in place of the list while it is the selected workspace.
    pub panels: Vec<Panel>,
    /// How many files of its repository have changes, as its Git panel counted them last.
    pub changes: usize,
}

impl Workspace {
    /// The second line: the base folder from `~` and its branch, or a notification of a tab not seen yet.
    pub fn detail(&self, home: &Path) -> String {
        let folder = tilde(&self.base.to_string_lossy(), home);
        // The home folder's title already reads `~`.
        let mut parts: Vec<String> = (folder != "~").then_some(folder).into_iter().collect();
        if let Some(notice) = self.tabs.iter().find_map(|tab| tab.notice.clone()) {
            parts.push(notice);
        } else if let Some(branch) = &self.branch {
            parts.push(branch.clone());
        }
        parts.join(" · ")
    }

    /// The dot of the row, as Dango marks an agent's tab: red while one of its agents waits for the user, else green
    /// while one works, also on subagents or background work; none otherwise. Terminals mark nothing.
    pub fn mark(&self) -> Option<Mark> {
        let marks = || self.tabs.iter().filter_map(Entry::mark);
        if marks().any(|mark| mark == Mark::Waits) {
            Some(Mark::Waits)
        } else {
            marks().next()
        }
    }

    /// What VoiceOver reads for the row.
    pub fn spoken(&self, home: &Path) -> String {
        let mut parts = vec![self.name.clone(), self.detail(home)];
        parts.extend(self.mark().map(|mark| mark.spoken().to_string()));
        parts
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(", ")
    }
}

/// The name a workspace gets from its base folder: the folder's own name, `~` for the home folder.
pub fn folder_name(base: &Path, home: &Path) -> String {
    if base == home {
        return "~".into();
    }
    base.file_name().map_or_else(
        || base.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn spoken_progress(progress: Option<Progress>) -> String {
    let Some(progress) = progress else {
        return String::new();
    };
    let percent = progress.percent.map(|percent| percent.to_string());
    match (progress.state, percent) {
        (ProgressState::Normal, Some(percent)) => tf("{} % erledigt", &[&percent]),
        (ProgressState::Normal | ProgressState::Indeterminate, _) => t("in Arbeit").to_string(),
        (ProgressState::Error, Some(percent)) => tf("Fehler bei {} %", &[&percent]),
        (ProgressState::Error, None) => t("Fehler").to_string(),
        (ProgressState::Paused, Some(percent)) => tf("angehalten bei {} %", &[&percent]),
        (ProgressState::Paused, None) => t("angehalten").to_string(),
    }
}

/// `folder` with the home folder as `~` and only its last two parts beyond.
pub fn short_folder(folder: &Path, home: &Path) -> String {
    if folder == home {
        return "~".into();
    }
    let parts: Vec<String> = folder
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    if folder.starts_with(home) && parts.len() <= home.components().count() + 2 {
        return format!("~/{}", folder.strip_prefix(home).unwrap_or(folder).display());
    }
    match parts.as_slice() {
        [.., parent, last] if parts.len() > 3 => format!("…/{parent}/{last}"),
        _ => folder.display().to_string(),
    }
}

/// `text` with each path that starts at the home folder written from `~`, as a shell shows it.
pub fn tilde(text: &str, home: &Path) -> String {
    // Canonical paths go through macOS's `/private` firmlinks, which people know as `/tmp`, `/var` and `/etc`.
    let text = &["tmp", "var", "etc"].iter().fold(text.to_string(), |text, dir| {
        text.replace(&format!("/private/{dir}"), &format!("/{dir}"))
    });
    let home = home.to_string_lossy();
    let home = home.trim_end_matches('/');
    if home.is_empty() {
        return text.to_string();
    }
    let starts = |before: Option<char>| before.is_none_or(|c| c.is_whitespace() || "\"'=:(".contains(c));
    let ends = |after: Option<char>| after.is_none_or(|c| c == '/' || c.is_whitespace() || "\"')".contains(c));
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    for (at, _) in text.match_indices(home) {
        let end = at + home.len();
        if at < copied || !starts(text[..at].chars().last()) || !ends(text[end..].chars().next()) {
            continue;
        }
        out.push_str(&text[copied..at]);
        out.push('~');
        copied = end;
    }
    out.push_str(&text[copied..]);
    out
}

/// A duration as people read it: `850 ms`, `12 s`, `3 min 4 s`, `2 h 5 min`.
pub fn duration(took: Duration) -> String {
    let seconds = took.as_secs();
    match seconds {
        0 => format!("{} ms", took.as_millis()),
        1..60 => format!("{seconds} s"),
        60..3600 => format!("{} min {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// The command line as the sidebar shows it: its first line, at most 80 characters, cut in the middle so that both
/// the program and the end of its arguments stay.
pub fn short_command(line: &str) -> String {
    const MOST: usize = 80;
    let first = line.lines().next().unwrap_or_default().trim();
    let count = first.chars().count();
    if count <= MOST {
        return first.to_string();
    }
    let head = (MOST - 1) / 2;
    let tail = MOST - 1 - head;
    let start: String = first.chars().take(head).collect();
    let end: String = first.chars().skip(count - tail).collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry() -> Entry {
        Entry {
            id: 1,
            kind: Kind::Terminal,
            icon: term_ui_icons::SHELL,
            others: Vec::new(),
            title: None,
            folder: "/Users/me/Repos/tapa".into(),
            branch: Some("main".into()),
            last_command: Some(("cargo test".into(), Duration::from_secs(75))),
            status: Status::Succeeded,
            progress: None,
            notice: None,
        }
    }

    fn progress(state: ProgressState, percent: Option<u8>) -> Option<Progress> {
        Some(Progress { state, percent })
    }

    #[test]
    fn a_row_without_title_shows_the_folder_and_below_branch_and_command() {
        let home = Path::new("/Users/me");
        let entry = entry();
        assert_eq!(entry.heading(home), "~/Repos/tapa");
        assert_eq!(entry.detail(home), "main · cargo test · 1 min 15 s");
    }

    #[test]
    fn an_agents_title_moves_the_folder_to_the_second_line() {
        let home = Path::new("/Users/me");
        let mut entry = entry();
        entry.title = Some("Fixing the parser".into());
        entry.last_command = None;
        assert_eq!(entry.heading(home), "Fixing the parser");
        assert_eq!(entry.detail(home), "~/Repos/tapa · main");
    }

    #[test]
    fn an_unseen_notification_takes_the_place_of_branch_and_command() {
        let home = Path::new("/Users/me");
        let mut entry = entry();
        entry.notice = Some("Build done".into());
        assert_eq!(entry.detail(home), "Build done");
        entry.title = Some("Claude".into());
        assert_eq!(entry.detail(home), "~/Repos/tapa · Build done");
    }

    #[test]
    fn the_ring_fills_with_the_progress_and_shows_how_the_work_goes() {
        let mut entry = entry();
        assert_eq!(entry.ring(), None);
        let ring = |filled, tone| Some(Ring { filled, tone });
        entry.progress = progress(ProgressState::Normal, Some(40));
        assert_eq!(entry.ring(), ring(Some(0.4), Tone::Normal));
        entry.progress = progress(ProgressState::Indeterminate, None);
        assert_eq!(entry.ring(), ring(None, Tone::Normal));
        entry.progress = progress(ProgressState::Error, None);
        assert_eq!(entry.ring(), ring(Some(1.0), Tone::Error));
        entry.progress = progress(ProgressState::Paused, Some(70));
        assert_eq!(entry.ring(), ring(Some(0.7), Tone::Paused));
    }

    #[test]
    fn voiceover_reads_status_and_progress_and_skips_what_is_empty() {
        let home = Path::new("/Users/me");
        let mut entry = entry();
        entry.last_command = None;
        entry.branch = None;
        entry.status = Status::Running;
        entry.progress = progress(ProgressState::Normal, Some(40));
        assert_eq!(entry.spoken(home), "~/Repos/tapa, läuft, 40 % erledigt");
        entry.status = Status::Idle;
        entry.progress = None;
        assert_eq!(entry.spoken(home), "~/Repos/tapa");
    }

    fn workspace(statuses: &[Status]) -> Workspace {
        Workspace {
            id: 7,
            name: "tapa".into(),
            base: "/Users/me/Repos/tapa".into(),
            branch: Some("main".into()),
            tabs: statuses
                .iter()
                .map(|status| Entry {
                    status: *status,
                    ..entry()
                })
                .collect(),
            trusted: false,
            fixed: false,
            panels: Vec::new(),
            changes: 0,
        }
    }

    #[test]
    fn a_workspace_row_marks_its_agents_as_dango_does() {
        let home = Path::new("/Users/me");
        let mut row = workspace(&[Status::Succeeded, Status::Running, Status::Attention]);
        assert_eq!(row.detail(home), "~/Repos/tapa · main");
        assert_eq!(row.mark(), None, "terminals mark nothing");
        row.tabs[1].kind = Kind::Agent;
        assert_eq!(row.mark(), Some(Mark::Works));
        assert_eq!(row.spoken(home), "tapa, ~/Repos/tapa · main, Arbeitet");
        row.tabs[0].kind = Kind::Agent;
        row.tabs[0].status = Status::Waiting;
        assert_eq!(row.mark(), Some(Mark::Waits), "waiting for the user comes first");
        row.tabs[1].notice = Some("Build done".into());
        assert_eq!(row.detail(home), "~/Repos/tapa · Build done");
        let mut resting = workspace(&[Status::Idle, Status::Attention]);
        resting.tabs[0].kind = Kind::Agent;
        resting.tabs[1].kind = Kind::Agent;
        assert_eq!(resting.mark(), None);
    }

    #[test]
    fn a_workspace_is_named_after_its_folder() {
        let home = Path::new("/Users/me");
        assert_eq!(folder_name(Path::new("/Users/me/Repos/tapa"), home), "tapa");
        assert_eq!(folder_name(home, home), "~");
        assert_eq!(folder_name(Path::new("/"), home), "/");
    }

    #[test]
    fn paths_in_titles_and_commands_start_from_the_home_folder() {
        let home = Path::new("/Users/me");
        assert_eq!(tilde("cd /Users/me/Library/Caches", home), "cd ~/Library/Caches");
        assert_eq!(tilde("/Users/me", home), "~");
        assert_eq!(tilde("cd /private/tmp/x", home), "cd /tmp/x");
        assert_eq!(tilde("cp /Users/me/a \"/Users/me/b c\"", home), "cp ~/a \"~/b c\"");
        assert_eq!(tilde("ls --dir=/Users/me/x", home), "ls --dir=~/x");
        assert_eq!(
            tilde("ls /Users/meyer /x/Users/me", home),
            "ls /Users/meyer /x/Users/me"
        );
        assert_eq!(tilde("echo hi", home), "echo hi");
        assert_eq!(tilde("ls /Users/me/", Path::new("/Users/me/")), "ls ~/");
        let mut entry = entry();
        entry.title = Some("vim /Users/me/notes.md".into());
        entry.last_command = Some(("cd /Users/me/Library/Caches".into(), Duration::from_millis(5)));
        assert_eq!(entry.heading(home), "vim ~/notes.md");
        assert_eq!(entry.detail(home), "~/Repos/tapa · main · cd ~/Library/Caches · 5 ms");
        entry.title = None;
        entry.folder = "/Users/me/Library/Caches/some/deep/folder".into();
        assert_eq!(entry.heading(home), "~/Library/Caches/some/deep/folder");
        entry.folder = "/opt/homebrew/var/log".into();
        assert_eq!(entry.heading(home), "/opt/homebrew/var/log");
    }

    #[test]
    fn folders_and_durations_read_short() {
        let home = Path::new("/Users/me");
        assert_eq!(short_folder(home, home), "~");
        assert_eq!(short_folder(Path::new("/opt/homebrew/var/log"), home), "…/var/log");
        assert_eq!(duration(Duration::from_millis(850)), "850 ms");
        assert_eq!(duration(Duration::from_secs(7500)), "2 h 5 min");
        let long = format!("cd /{}/end", "x".repeat(100));
        assert_eq!(short_command(&long).chars().count(), 80);
        assert!(short_command(&long).starts_with("cd /x") && short_command(&long).ends_with("x/end"));
        assert_eq!(short_command("ls\nmore"), "ls");
    }
}
