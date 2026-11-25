//! Shell integration: small scripts that make zsh, bash and fish mark prompts and commands (OSC 133) and report
//! their working directory (OSC 7), loaded around the user's own startup files without changing them.

use std::io;
use std::path::{Path, PathBuf};

const ZSHENV: &str = include_str!("shell/zshenv.zsh");
const ZSH: &str = include_str!("shell/integration.zsh");
const BASH: &str = include_str!("shell/integration.bash");
const FISH: &str = include_str!("shell/integration.fish");
const SSH: &str = include_str!("shell/ssh.sh");

mod remote;

/// The scripts and where they go below the integration folder.
const FILES: &[(&str, &str)] = &[
    ("zsh/.zshenv", ZSHENV),
    ("zsh/integration.zsh", ZSH),
    ("bash/integration.bash", BASH),
    ("fish/vendor_conf.d/term-shell-integration.fish", FISH),
    ("ssh/ssh.sh", SSH),
];

/// The file below the integration folder that holds the command `ssh` runs on the other host.
const REMOTE: &str = "ssh/remote";

/// Where the app keeps the scripts: `shell-integration` in its cache folder.
pub(crate) fn default_dir() -> PathBuf {
    term_base::cache_dir().join("shell-integration")
}

/// Writes the scripts into `dir`, leaving files alone that are already current so running shells never read a
/// half-written one.
pub(crate) fn install(dir: &Path) -> io::Result<()> {
    let product = term_base::identity();
    let remote = remote::command(&product.name, &product.version);
    for (name, content) in FILES.iter().copied().chain([(REMOTE, remote.as_str())]) {
        let path = dir.join(name);
        if std::fs::read(&path).is_ok_and(|current| current == content.as_bytes()) {
            continue;
        }
        term_base::atomic_write::write(&path, content.as_bytes())?;
    }
    Ok(())
}

/// How to start `ssh` with `args`: through the wrapper in `scripts` that takes the integration along when given, else
/// as it is.
pub(crate) fn ssh(scripts: Option<&Path>, args: &[String]) -> (PathBuf, Vec<String>) {
    let Some(dir) = scripts else {
        return ("/usr/bin/ssh".into(), args.to_vec());
    };
    let ssh = dir.join("ssh");
    let mut wrapped = vec![
        ssh.join("ssh.sh").to_string_lossy().into_owned(),
        ssh.join(REMOTE.trim_start_matches("ssh/"))
            .to_string_lossy()
            .into_owned(),
    ];
    wrapped.extend(args.iter().cloned());
    ("/bin/sh".into(), wrapped)
}

/// How to start a login shell: its arguments and the variables to set on top of the user's environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
}

/// Shells known to take `-l` for a login shell.
const LOGIN_FLAG: &[&str] = &[
    "zsh", "bash", "fish", "sh", "dash", "ksh", "mksh", "tcsh", "csh", "nu", "xonsh", "pwsh",
];

/// How to start `shell` as a login shell, with the scripts in `scripts` when given, and with `ssh` taking them to other
/// hosts when `ssh` is set; `env` is the user's environment, whose `ZDOTDIR` and `XDG_DATA_DIRS` the scripts restore.
pub(crate) fn launch(shell: &Path, scripts: Option<&Path>, ssh: bool, env: &[(String, String)]) -> Launch {
    let mut launch = launch_shell(shell, scripts, env);
    let integrated = matches!(shell_name(shell).as_str(), "zsh" | "bash" | "fish");
    if let Some(dir) = scripts.filter(|_| ssh && integrated) {
        launch
            .env
            .push(("IDE_SSH_DIR".into(), dir.join("ssh").to_string_lossy().into_owned()));
    }
    launch
}

fn shell_name(shell: &Path) -> String {
    shell
        .file_name()
        .map(|name| name.to_string_lossy().trim_start_matches('-').to_string())
        .unwrap_or_default()
}

fn launch_shell(shell: &Path, scripts: Option<&Path>, env: &[(String, String)]) -> Launch {
    let name = shell_name(shell);
    let original = |key: &str| env.iter().find(|(name, _)| name == key).map(|(_, value)| value.clone());
    let path = |relative: &str| scripts.map(|dir| dir.join(relative).to_string_lossy().into_owned());
    let login = || {
        if LOGIN_FLAG.contains(&name.as_str()) {
            vec!["-l".to_string()]
        } else {
            Vec::new()
        }
    };
    match (name.as_str(), scripts) {
        ("zsh", Some(_)) => {
            let mut env = vec![("ZDOTDIR".to_string(), path("zsh").unwrap_or_default())];
            if let Some(zdotdir) = original("ZDOTDIR") {
                env.push(("IDE_ZSH_ZDOTDIR".into(), zdotdir));
            }
            Launch { args: login(), env }
        }
        ("bash", Some(_)) => Launch {
            args: vec![
                "--init-file".into(),
                path("bash/integration.bash").unwrap_or_default(),
                "-i".into(),
            ],
            env: Vec::new(),
        },
        ("fish", Some(dir)) => {
            let previous = original("XDG_DATA_DIRS");
            // Without XDG_DATA_DIRS fish reads the specification's default, which has to stay in the list.
            let rest = previous.clone().unwrap_or_else(|| "/usr/local/share:/usr/share".into());
            let mut env = vec![("XDG_DATA_DIRS".to_string(), format!("{}:{rest}", dir.to_string_lossy()))];
            if let Some(previous) = previous {
                env.push(("IDE_FISH_XDG_DATA_DIRS".into(), previous));
            }
            Launch { args: login(), env }
        }
        _ => Launch {
            args: login(),
            env: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use term_base::testing::scratch as temporary;

    #[test]
    fn zsh_gets_the_scripts_through_zdotdir_and_keeps_the_users_one_for_later() {
        let dir = Path::new("/cache/si");
        let plain = launch(Path::new("/bin/zsh"), Some(dir), false, &[]);
        assert_eq!(plain.args, ["-l"]);
        assert_eq!(plain.env, [("ZDOTDIR".to_string(), "/cache/si/zsh".to_string())]);
        let own = launch(
            Path::new("/bin/zsh"),
            Some(dir),
            false,
            &[("ZDOTDIR".into(), "/u/.config/zsh".into())],
        );
        assert!(own.env.contains(&("IDE_ZSH_ZDOTDIR".into(), "/u/.config/zsh".into())));
    }

    #[test]
    fn bash_reads_the_script_as_its_init_file() {
        let bash = launch(Path::new("/opt/homebrew/bin/bash"), Some(Path::new("/c")), false, &[]);
        assert_eq!(bash.args, ["--init-file", "/c/bash/integration.bash", "-i"]);
        assert!(bash.env.is_empty());
    }

    #[test]
    fn fish_finds_the_scripts_first_in_xdg_data_dirs() {
        let fish = launch(Path::new("/opt/homebrew/bin/fish"), Some(Path::new("/c")), false, &[]);
        assert_eq!(fish.args, ["-l"]);
        assert_eq!(
            fish.env,
            [(
                "XDG_DATA_DIRS".to_string(),
                "/c:/usr/local/share:/usr/share".to_string()
            )]
        );
        let own = launch(
            Path::new("/bin/fish"),
            Some(Path::new("/c")),
            false,
            &[("XDG_DATA_DIRS".into(), "/x".into())],
        );
        assert_eq!(
            own.env,
            [
                ("XDG_DATA_DIRS".to_string(), "/c:/x".to_string()),
                ("IDE_FISH_XDG_DATA_DIRS".to_string(), "/x".to_string())
            ]
        );
    }

    #[test]
    fn without_integration_or_for_other_shells_only_the_login_flag_is_passed() {
        assert_eq!(launch(Path::new("/bin/zsh"), None, false, &[]).args, ["-l"]);
        assert!(launch(Path::new("/bin/zsh"), None, false, &[]).env.is_empty());
        assert_eq!(launch(Path::new("/bin/bash"), None, false, &[]).args, ["-l"]);
        assert_eq!(
            launch(Path::new("/usr/local/bin/nu"), Some(Path::new("/c")), false, &[]).args,
            ["-l"]
        );
        assert!(
            launch(Path::new("/usr/local/bin/elvish"), Some(Path::new("/c")), false, &[])
                .args
                .is_empty()
        );
    }

    #[test]
    fn ssh_takes_the_scripts_along_only_when_asked_and_for_shells_that_have_them() {
        let dir = Path::new("/c");
        let ssh_dir = ("IDE_SSH_DIR".to_string(), "/c/ssh".to_string());
        assert!(
            launch(Path::new("/bin/zsh"), Some(dir), true, &[])
                .env
                .contains(&ssh_dir)
        );
        assert!(
            launch(Path::new("/bin/bash"), Some(dir), true, &[])
                .env
                .contains(&ssh_dir)
        );
        assert!(
            !launch(Path::new("/bin/zsh"), Some(dir), false, &[])
                .env
                .contains(&ssh_dir)
        );
        assert!(launch(Path::new("/bin/zsh"), None, true, &[]).env.is_empty());
        assert!(launch(Path::new("/bin/ksh"), Some(dir), true, &[]).env.is_empty());
    }

    #[test]
    fn installing_writes_every_script_once_and_repairs_a_changed_one() {
        let dir = temporary("install");
        install(&dir).unwrap();
        for (name, content) in FILES {
            assert_eq!(std::fs::read_to_string(dir.join(name)).unwrap(), *content);
        }
        let zshenv = dir.join("zsh/.zshenv");
        std::fs::write(&zshenv, "broken").unwrap();
        install(&dir).unwrap();
        assert_eq!(std::fs::read_to_string(&zshenv).unwrap(), ZSHENV);
        let remote = std::fs::read_to_string(dir.join(REMOTE)).unwrap();
        assert!(remote.starts_with("exec /bin/sh -c '"));
        let leftovers = std::fs::read_dir(dir.join("zsh")).unwrap().count();
        assert_eq!(leftovers, 2, "no temporary files stay behind");
    }
}
