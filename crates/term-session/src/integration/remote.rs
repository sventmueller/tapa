//! Shell integration on another host (F89): the command `ssh` runs there instead of the plain login shell, which
//! unpacks the scripts into a private temporary folder and starts the user's shell with them, as the terminal does on
//! this Mac.

use super::{BASH, FISH, ZSH, ZSHENV};

/// The command for the remote login shell; it has to read the same in sh, csh and fish, so the script inside the
/// single quotes holds no quote, backslash, `!` or line break.
pub(crate) fn command(name: &str, version: &str) -> String {
    command_in(name, version, "")
}

/// The command `command` makes, starting the shell in `folder` there: a path from the home folder, `~/…`, or
/// absolute; empty for the home folder. A folder that does not exist leaves the shell in the home folder.
pub(crate) fn command_in(name: &str, version: &str, folder: &str) -> String {
    let folder = folder
        .strip_prefix("~/")
        .unwrap_or(if folder == "~" { "" } else { folder });
    let change = if folder.is_empty() {
        String::new()
    } else {
        let encoded = base64(folder.as_bytes());
        format!(r#"cd "$(printf %s {encoded}|base64 -d 2>/dev/null||printf %s {encoded}|base64 -D)" 2>/dev/null;"#)
    };
    let files = [
        ("zsh/.zshenv", ZSHENV),
        ("zsh/integration.zsh", ZSH),
        ("bash/integration.bash", BASH),
        ("fish/vendor_conf.d/term-shell-integration.fish", FISH),
    ];
    let unpack = files
        .iter()
        .map(|(path, content)| format!("u {path} {}", base64(content.as_bytes())))
        .collect::<Vec<_>>()
        .join("&&");
    let mut identity = String::from("export COLORTERM=truecolor");
    if plain(name) && plain(version) {
        identity.push_str(&format!(" TERM_PROGRAM={name} TERM_PROGRAM_VERSION={version}"));
    }
    let script = [
        r#"umask 077;s=${SHELL:-/bin/sh};d=$(mktemp -d "${TMPDIR:-/tmp}/term-si.XXXXXX")||exec "$s" -l;"#,
        r#"u(){ printf %s "$2"|base64 -d >"$d/$1" 2>/dev/null||printf %s "$2"|base64 -D >"$d/$1" 2>/dev/null; };"#,
        r#"mkdir -p "$d/zsh" "$d/bash" "$d/fish/vendor_conf.d"&&"#,
        &unpack,
        r#"||{ rm -rf "$d";exec "$s" -l; };"#,
        // The shells read the scripts while they start; the folder goes once they surely have.
        r#"(sleep 60;rm -rf "$d")</dev/null >/dev/null 2>&1 &"#,
        &identity,
        ";",
        &change,
        "case ${s##*/} in ",
        r#"zsh) [ -z "${ZDOTDIR+x}" ]||export IDE_ZSH_ZDOTDIR="$ZDOTDIR";export ZDOTDIR="$d/zsh";exec "$s" -l;;"#,
        r#" bash) exec "$s" --init-file "$d/bash/integration.bash" -i;;"#,
        r#" fish) [ -z "${XDG_DATA_DIRS+x}" ]||export IDE_FISH_XDG_DATA_DIRS="$XDG_DATA_DIRS";"#,
        r#"export XDG_DATA_DIRS="$d:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}";exec "$s" -l;;"#,
        r#" esac;exec "$s" -l"#,
    ]
    .concat();
    format!("exec /bin/sh -c '{script}'")
}

/// Whether `text` needs no quoting in any shell.
fn plain(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[(n >> (18 - 6 * i)) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_is_the_standard_alphabet_with_padding() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"\xff\xfe\xfd\x00"), "//79AA==");
    }

    #[test]
    fn the_shell_starts_in_the_folder_asked_for() {
        let dir = term_base::testing::scratch("remote-command-folder");
        let home = dir.join("home");
        std::fs::create_dir_all(home.join("my project")).unwrap();
        let fake = dir.join("zsh");
        std::fs::write(&fake, "#!/bin/sh\npwd\n").unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        for folder in ["~/my project", "my project", &home.join("my project").to_string_lossy()] {
            let command = command_in("Tapa", "1", folder);
            let inner = command
                .strip_prefix("exec /bin/sh -c '")
                .and_then(|rest| rest.strip_suffix('\''))
                .unwrap();
            assert!(!inner.contains(['\'', '\\', '!', '\n']), "{inner}");
            let output = std::process::Command::new("/bin/sh")
                .args(["-c", &command])
                .current_dir(&home)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &home)
                .env("SHELL", &fake)
                .output()
                .unwrap();
            let shown = String::from_utf8_lossy(&output.stdout);
            assert!(shown.trim_end().ends_with("/home/my project"), "{folder}: {shown}");
        }
    }

    #[test]
    fn the_command_reads_the_same_in_every_shell() {
        let command = command("Tapa", "1.0.0");
        let inner = command
            .strip_prefix("exec /bin/sh -c '")
            .and_then(|rest| rest.strip_suffix('\''))
            .expect("one single-quoted script");
        assert!(!inner.contains(['\'', '\\', '!', '\n', '\r']), "{inner}");
        assert!(inner.contains("TERM_PROGRAM=Tapa TERM_PROGRAM_VERSION=1.0.0"));
        assert!(!super::command("Ta pa", "1").contains("TERM_PROGRAM"));
    }

    #[test]
    fn the_command_starts_each_shell_with_its_script_and_others_plain() {
        let dir = term_base::testing::scratch("remote-command");
        let home = dir.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let tmp = dir.join("tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let fake = dir.join("fakeshell");
        std::fs::write(
            &fake,
            "#!/bin/sh\nprintf '%s\\n' \"$@\" \"ZDOTDIR=$ZDOTDIR\" \"COLORTERM=$COLORTERM\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let run_in = |login: &str, shell: &std::path::Path| {
            let output = std::process::Command::new(login)
                .args(["-c", &command("Tapa", "1")])
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .env("HOME", &home)
                .env("TMPDIR", &tmp)
                .env("SHELL", shell)
                .output()
                .unwrap();
            String::from_utf8_lossy(&output.stdout).into_owned()
        };
        let run = |shell: &std::path::Path| run_in("/bin/sh", shell);
        let zsh = dir.join("zsh");
        std::fs::copy(&fake, &zsh).unwrap();
        let out = run(&zsh);
        let zdotdir = out
            .lines()
            .find_map(|line| line.strip_prefix("ZDOTDIR="))
            .expect("zsh gets a ZDOTDIR")
            .to_string();
        assert!(out.starts_with("-l\n"), "{out}");
        assert!(out.contains("COLORTERM=truecolor"));
        assert_eq!(std::fs::read_to_string(format!("{zdotdir}/.zshenv")).unwrap(), ZSHENV);
        assert_eq!(
            std::fs::read_to_string(format!("{zdotdir}/integration.zsh")).unwrap(),
            ZSH
        );
        let mode = std::fs::metadata(std::path::Path::new(&zdotdir).parent().unwrap()).unwrap();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o777,
            0o700
        );

        let bash = dir.join("bash");
        std::fs::copy(&fake, &bash).unwrap();
        let out = run(&bash);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "--init-file");
        assert_eq!(std::fs::read_to_string(lines[1]).unwrap(), BASH);
        assert_eq!(lines[2], "-i");

        let other = dir.join("tcsh");
        std::fs::copy(&fake, &other).unwrap();
        assert!(run(&other).starts_with("-l\nZDOTDIR=\n"));

        for login in ["/bin/csh", "/bin/zsh", "/opt/homebrew/bin/fish"] {
            if std::path::Path::new(login).exists() {
                assert!(run_in(login, &bash).starts_with("--init-file\n"), "{login}");
            }
        }
    }

    /// What `ssh` was called with by the integration's `ssh` for `args`, one call per line, its arguments separated
    /// by `|`; `config` is what `ssh -G` prints, and `tty` runs it in a terminal as a shell would.
    fn wrapped(args: &[&str], config: &str, tty: bool) -> Vec<String> {
        let dir = term_base::testing::scratch("ssh-wrapper");
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let fake = bin.join("ssh");
        std::fs::write(
            &fake,
            "#!/bin/sh\nIFS='|'; printf '%s\\n' \"$*\" >> \"$CALLS\"\n[ \"$1\" = -G ] && printf '%s\\n' \"$CONFIG\"\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let wrapper = dir.join("ssh.sh");
        std::fs::write(&wrapper, super::super::SSH).unwrap();
        let remote = dir.join("remote");
        std::fs::write(&remote, "REMOTE").unwrap();
        let calls = dir.join("calls");
        let mut words = vec![
            wrapper.to_string_lossy().into_owned(),
            remote.to_string_lossy().into_owned(),
        ];
        words.extend(args.iter().map(ToString::to_string));
        let mut command = if tty {
            let mut command = std::process::Command::new("/usr/bin/script");
            command.args(["-q", "/dev/null", "/bin/sh"]);
            command
        } else {
            std::process::Command::new("/bin/sh")
        };
        command
            .args(&words)
            .env("PATH", format!("{}:/usr/bin:/bin", bin.to_string_lossy()))
            .env("CALLS", &calls)
            .env("CONFIG", format!("host x\n{config}"))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        std::fs::read_to_string(&calls)
            .unwrap_or_default()
            .lines()
            .filter(|line| !line.starts_with("-G|"))
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn ssh_brings_the_integration_only_to_an_interactive_shell() {
        let auto = "requesttty auto\nsessiontype default";
        assert_eq!(wrapped(&["host"], auto, true), ["-t|host|REMOTE"]);
        assert_eq!(
            wrapped(&["-p", "2222", "me@host"], auto, true),
            ["-t|-p|2222|me@host|REMOTE"]
        );
        assert_eq!(
            wrapped(&["-vp2222", "host", "-A"], auto, true),
            ["-t|-vp2222|host|-A|REMOTE"]
        );
        assert_eq!(wrapped(&["--", "host"], auto, true), ["-t|--|host|REMOTE"]);
        for plain in [
            &["host", "ls", "-la"][..],
            &["-N", "-L", "8080:localhost:80", "host"],
            &["-W", "db:5432", "jump"],
            &["-T", "git@example.com"],
            &["-O", "check", "host"],
            &["-p"],
            &[],
        ] {
            assert_eq!(wrapped(plain, auto, true), [plain.join("|")], "{plain:?}");
        }
        assert_eq!(wrapped(&["host"], "remotecommand tmux attach", true), ["host"]);
        assert_eq!(wrapped(&["host"], "requesttty no", true), ["host"]);
        assert_eq!(wrapped(&["host"], "sessiontype subsystem", true), ["host"]);
        assert_eq!(wrapped(&["host"], auto, false), ["host"], "not in a terminal");
    }
}
