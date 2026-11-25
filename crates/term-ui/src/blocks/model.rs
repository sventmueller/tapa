//! What the command blocks show, decided without AppKit: the colour of a block's gutter (F82), its status line of exit
//! code, duration and folder (F79), the block under a line, and the name its saved output gets (F81).

use std::path::Path;
use std::time::{Duration, SystemTime};

use splat_i18n::tf;
use term_session::session::{Block, BlockState, BlocksView, ShownBlock};

/// How a block's gutter is marked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gutter {
    /// The command ended with an exit code other than 0.
    Failed,
    /// The command runs.
    Running,
    /// The command ended well, or without an exit code.
    Done,
}

/// The gutter of `block`; none for a prompt without a command.
pub(crate) fn gutter(block: &Block) -> Option<Gutter> {
    match block.state {
        BlockState::Prompt => None,
        BlockState::Running => Some(Gutter::Running),
        BlockState::Finished if block.failed() => Some(Gutter::Failed),
        BlockState::Finished => Some(Gutter::Done),
    }
}

/// A duration as a person reads it: milliseconds below a second, tenths below a minute, then minutes and hours.
pub(crate) fn duration_text(duration: Duration) -> String {
    let millis = duration.as_millis();
    let seconds = duration.as_secs();
    match seconds {
        0 => format!("{millis} ms"),
        1..60 => format!("{:.1} s", duration.as_secs_f64()),
        60..3600 => format!("{} min {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// `path` with the home folder as `~`.
pub(crate) fn folder_text(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// The parts of a block's status line that need no clock face: the exit code, how long it ran by `now`, the folder.
pub(crate) fn status_parts(block: &Block, now: SystemTime, home: &Path) -> Vec<String> {
    let mut parts = Vec::new();
    match (block.state, block.exit_code) {
        (BlockState::Finished, Some(0)) => parts.push("✓".to_string()),
        (BlockState::Finished, Some(code)) => parts.push(tf("✗ Exit-Code {}", &[&code.to_string()])),
        _ => {}
    }
    if let Some(duration) = block.duration(now) {
        parts.push(duration_text(duration));
    }
    if let Some(directory) = &block.directory {
        parts.push(folder_text(&directory.path, home));
    }
    parts
}

/// The block that covers view line `line`.
pub(crate) fn block_at(view: &BlocksView, line: usize) -> Option<&ShownBlock> {
    view.blocks
        .iter()
        .find(|shown| (shown.lines.0..=shown.lines.1).contains(&line))
}

/// The name a saved output is offered under: the command's first word and `output.txt`.
pub(crate) fn file_name(command: Option<&str>) -> String {
    let word: String = command
        .and_then(|command| command.split_whitespace().next())
        .map(|word| word.rsplit('/').next().unwrap_or(word))
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let word = word.trim_matches('.');
    if word.is_empty() {
        "output.txt".into()
    } else {
        format!("{word}-output.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use term_session::WorkingDirectory;

    fn block(state: BlockState, exit_code: Option<i32>) -> Block {
        let mut terminal = term_vt_like();
        terminal.state = state;
        terminal.exit_code = exit_code;
        terminal
    }

    fn term_vt_like() -> Block {
        Block {
            id: 1,
            prompt: 0,
            input: None,
            output: Some(1),
            finish: None,
            end: None,
            command: Some("ls".into()),
            exit_code: None,
            state: BlockState::Prompt,
            started: None,
            finished: None,
            directory: None,
        }
    }

    #[test]
    fn the_gutter_marks_failed_running_and_done_commands() {
        assert_eq!(gutter(&block(BlockState::Prompt, None)), None);
        assert_eq!(gutter(&block(BlockState::Running, None)), Some(Gutter::Running));
        assert_eq!(gutter(&block(BlockState::Finished, Some(0))), Some(Gutter::Done));
        assert_eq!(gutter(&block(BlockState::Finished, None)), Some(Gutter::Done));
        assert_eq!(gutter(&block(BlockState::Finished, Some(127))), Some(Gutter::Failed));
    }

    #[test]
    fn durations_read_in_the_unit_that_fits() {
        assert_eq!(duration_text(Duration::from_millis(42)), "42 ms");
        assert_eq!(duration_text(Duration::from_millis(2_340)), "2.3 s");
        assert_eq!(duration_text(Duration::from_secs(125)), "2 min 5 s");
        assert_eq!(duration_text(Duration::from_secs(7_260)), "2 h 1 min");
    }

    #[test]
    fn the_status_has_exit_code_duration_and_folder() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let mut failed = block(BlockState::Finished, Some(2));
        failed.started = Some(start);
        failed.finished = Some(start + Duration::from_millis(1_500));
        failed.directory = Some(Arc::new(WorkingDirectory {
            host: String::new(),
            path: PathBuf::from("/Users/u/src/app"),
        }));
        let parts = status_parts(&failed, SystemTime::now(), Path::new("/Users/u"));
        assert_eq!(parts, ["✗ Exit-Code 2", "1.5 s", "~/src/app"]);
        let mut running = block(BlockState::Running, None);
        running.started = Some(start);
        let parts = status_parts(&running, start + Duration::from_secs(3), Path::new("/Users/u"));
        assert_eq!(parts, ["3.0 s"]);
        assert_eq!(folder_text(Path::new("/Users/u"), Path::new("/Users/u")), "~");
        assert_eq!(folder_text(Path::new("/tmp"), Path::new("/Users/u")), "/tmp");
    }

    #[test]
    fn a_saved_output_is_named_after_the_command() {
        assert_eq!(file_name(Some("cargo test -p x")), "cargo-output.txt");
        assert_eq!(file_name(Some("/usr/bin/git log")), "git-output.txt");
        assert_eq!(file_name(Some("../")), "output.txt");
        assert_eq!(file_name(None), "output.txt");
    }
}
