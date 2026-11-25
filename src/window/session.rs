//! A new session of a window: its view, the program the default profile names, and its environment.

use std::path::{Path, PathBuf};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use term_config::Config;
use term_session::Program;
use term_ui::TerminalView;
use term_ui::kit::WindowId;

use crate::state::app;

/// Variables of the app's own environment a shell must not inherit: the terminal it was started from, if any, and
/// what only makes sense for the app.
const NOT_INHERITED: &[&str] = &[
    "TERM",
    "TERM_PROGRAM",
    "TERM_PROGRAM_VERSION",
    "TERM_SESSION_ID",
    "COLORTERM",
    "ITERM_SESSION_ID",
    "GHOSTTY_RESOURCES_DIR",
    "GHOSTTY_BIN_DIR",
    "WARP_IS_LOCAL_SHELL_SESSION",
    "TAPA_CONFIG",
    "OLDPWD",
    "PWD",
    "SHLVL",
    "__CFBundleIdentifier",
    "XPC_SERVICE_NAME",
];

/// Where a new session starts: the folder of the session in front where `inherit` says so and it still exists,
/// else the default profile's folder, else the home folder (F51).
pub(crate) fn start_folder(config: &Config, current: Option<PathBuf>, inherit: bool) -> PathBuf {
    if inherit && let Some(current) = current.filter(|dir| dir.is_dir()) {
        return current;
    }
    config
        .profile(&config.default_profile)
        .ok()
        .and_then(|profile| profile.directory)
        .map(|dir| expand_home(&dir))
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(crate::host::home)
}

fn expand_home(dir: &Path) -> PathBuf {
    match dir.strip_prefix("~") {
        Ok(rest) => crate::host::home().join(rest),
        Err(_) => dir.to_path_buf(),
    }
}

/// A session view in window `window` that will start in `folder`, running `program` or the default profile's.
pub(crate) fn make(
    mtm: MainThreadMarker,
    window: u64,
    folder: PathBuf,
    program: Option<Program>,
) -> (Retained<TerminalView>, Retained<NSView>) {
    let (view, root) = TerminalView::new(mtm, WindowId::new(window), folder);
    let config = app().config();
    let program = program.or_else(|| {
        let command = config.profile(&config.default_profile).ok()?.command?;
        let (program, args) = command.split_first()?;
        Some(Program::Command {
            program: program.into(),
            args: args.to_vec(),
        })
    });
    if let Some(program) = program {
        view.set_program(program);
    }
    (view, root)
}

/// Turns the input field of `view` on or off as its switch does; new sessions follow that choice, which the settings
/// take over (F209).
pub(crate) fn toggle_field(view: &TerminalView) {
    view.toggle_field();
    let on = view.field_enabled();
    if app().config().input.field == on {
        return;
    }
    let written = crate::settings::apply(|doc| {
        doc.set(&["input", "field"], on);
        Ok(true)
    });
    if let Err(error) = written {
        term_base::diag::report("the input field setting", error);
    }
}

/// Starts the shell of `view` with the app's environment and the default profile's variables.
pub(crate) fn spawn(view: &TerminalView) {
    let config = app().config();
    let mut env: Vec<(String, String)> = std::env::vars()
        .filter(|(name, _)| !NOT_INHERITED.contains(&name.as_str()))
        .collect();
    if let Ok(profile) = config.profile(&config.default_profile) {
        for (name, value) in profile.env {
            env.retain(|(existing, _)| *existing != name);
            env.push((name, value));
        }
    }
    view.spawn_restored(&env, crate::restore::take_rows(view.id()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_session_inherits_only_an_existing_folder_and_only_when_asked() {
        let config = Config::default();
        let here = std::env::temp_dir();
        assert_eq!(start_folder(&config, Some(here.clone()), true), here);
        assert_eq!(start_folder(&config, Some(here), false), crate::host::home());
        assert_eq!(
            start_folder(&config, Some("/no/such/dir".into()), true),
            crate::host::home()
        );
    }
}
