//! What runs on the other machine. ssh hands its command to the user's login shell there, which may be sh, bash, zsh,
//! fish or csh, each quoting differently; so the real script travels in base64 and runs in `/bin/sh`, as the shell
//! integration over SSH (F89) does.

/// `text` in single quotes for `/bin/sh`.
pub(crate) fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

pub(crate) fn base64(bytes: &[u8]) -> String {
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

/// A command for ssh that runs `script` in `/bin/sh` on the other machine, whatever the user's login shell there is:
/// on the way it holds only base64 and characters every shell takes literally inside single quotes.
pub fn through_any_shell(script: &str) -> String {
    let encoded = base64(script.as_bytes());
    // macOS's base64 decodes with -D, older ones only with it; GNU and BusyBox with -d.
    format!("exec /bin/sh -c 'eval \"$(printf %s {encoded}|base64 -d 2>/dev/null||printf %s {encoded}|base64 -D)\"'")
}

/// The script that starts `program` with `args` and the variables `env` in `folder` (the home folder when empty):
/// with the `PATH` the user's login shell sets, since a command over ssh gets none of the login files, and the usual
/// folders of user-installed programs after it.
pub fn agent_script(folder: &str, program: &str, args: &[String], env: &[(String, String)]) -> String {
    let mut script = String::from(concat!(
        r#"p=$("${SHELL:-/bin/sh}" -lc 'printf %s "$PATH"' </dev/null 2>/dev/null);"#,
        r#"[ -n "$p" ]&&PATH=$p;PATH="$PATH:$HOME/.local/bin:$HOME/.npm-global/bin:/opt/homebrew/bin:/usr/local/bin";"#,
        "export PATH;",
    ));
    if !folder.is_empty() {
        let folder = match folder.strip_prefix("~/") {
            Some(rest) => format!("\"$HOME\"/{}", quote(rest)),
            None if folder == "~" => "\"$HOME\"".to_string(),
            None => quote(folder),
        };
        script.push_str(&format!("cd {folder}||exit 1;"));
    }
    script.push_str("exec env");
    for (name, value) in env {
        script.push(' ');
        script.push_str(&quote(&format!("{name}={value}")));
    }
    script.push(' ');
    script.push_str(&quote(program));
    for arg in args {
        script.push(' ');
        script.push_str(&quote(arg));
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_keeps_every_character_as_it_is() {
        let tricky = r#"it's "$HOME" `x` \n !"#;
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &format!("printf %s {}", quote(tricky))])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&output.stdout), tricky);
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"fo"), "Zm8=");
    }

    #[test]
    fn the_command_holds_nothing_a_shell_would_read_and_runs_the_script() {
        let script = r#"printf '%s|' "it's" "$((1+1))""#;
        let command = through_any_shell(script);
        let inner = command
            .strip_prefix("exec /bin/sh -c '")
            .and_then(|rest| rest.strip_suffix('\''))
            .unwrap();
        assert!(!inner.contains(['\'', '\\', '!', '\n']), "{inner}");
        for shell in ["/bin/sh", "/bin/zsh", "/bin/bash"] {
            let output = std::process::Command::new(shell)
                .args(["-c", &command])
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&output.stdout), "it's|2|", "{shell}");
        }
    }

    #[test]
    fn the_agent_starts_in_its_folder_with_its_arguments_and_variables() {
        let dir = term_base::testing::scratch("remote-agent-script");
        let folder = dir.join("my project");
        std::fs::create_dir_all(&folder).unwrap();
        let script = agent_script(
            &folder.to_string_lossy(),
            "/bin/sh",
            &[
                "-c".into(),
                r#"printf '%s|' "$PWD" "$ENTRY" "$1""#.into(),
                "x".into(),
                "a b'c".into(),
            ],
            &[("ENTRY".into(), "tapa's".into())],
        );
        let output = std::process::Command::new("/bin/sh")
            .args(["-c", &through_any_shell(&script)])
            .env("SHELL", "/bin/sh")
            .output()
            .unwrap();
        let shown = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(shown.ends_with("my project|tapa's|a b'c|"), "{shown}");
        let missing = agent_script("/no/such/folder", "/bin/echo", &[], &[]);
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", &missing])
            .status()
            .unwrap();
        assert!(!status.success(), "a missing folder ends the start");
    }
}
