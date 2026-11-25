//! What leaves the terminal for other apps: links in the browser, files in Dango where it is installed (F178) or in
//! their default app, and what other apps hand the terminal (F216): folders, scripts and `ssh://` addresses.

use std::path::{Path, PathBuf};

use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSURL};
use term_session::Program;
use term_ui::kit::ns;

use crate::window::Window;

/// Dango's bundle identifier, which ⌘-clicked paths open in when it is installed (F178).
const DANGO: &str = "local.agentide";

/// Opens `url` in the default browser.
pub(crate) fn url(url: &str) {
    if let Some(url) = NSURL::URLWithString(&ns(url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

/// Opens the file at `path` in Dango, or in its default app where Dango is not installed; something that would run
/// is shown in the Finder instead, since a program chose the path or link that was clicked.
pub(crate) fn file(path: &Path) {
    let workspace = NSWorkspace::sharedWorkspace();
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    if runs(path) {
        workspace.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url]));
        return;
    }
    match workspace.URLForApplicationWithBundleIdentifier(&ns(DANGO)) {
        Some(dango) => {
            let configuration = objc2_app_kit::NSWorkspaceOpenConfiguration::configuration();
            workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                &NSArray::from_retained_slice(&[url]),
                &dango,
                &configuration,
                None,
            );
        }
        None => {
            workspace.openURL(&url);
        }
    }
}

/// Whether opening `path` would start something: an app, an installer, a script or an executable file.
fn runs(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    const RUN: [&str; 10] = [
        "app",
        "command",
        "tool",
        "pkg",
        "mpkg",
        "terminal",
        "workflow",
        "action",
        "scpt",
        "applescript",
    ];
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if extension.is_some_and(|extension| RUN.contains(&extension.as_str())) {
        return true;
    }
    std::fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

/// Opens the settings file in the default editor for TOML, writing the defaults first if it does not exist (F159).
pub(crate) fn settings() {
    let app = crate::state::app();
    let path = app.config_path.clone();
    if !path.exists()
        && let Err(error) = app.config().save(&path)
    {
        term_base::diag::report("writing the settings", &error);
        return;
    }
    let url = NSURL::fileURLWithPath(&ns(&path.to_string_lossy()));
    let workspace = NSWorkspace::sharedWorkspace();
    let editor = workspace.URLForApplicationToOpenURL(&url);
    match editor {
        Some(_) => {
            workspace.openURL(&url);
        }
        None => {
            if let Some(text_edit) = workspace.URLForApplicationWithBundleIdentifier(&ns("com.apple.TextEdit")) {
                let configuration = objc2_app_kit::NSWorkspaceOpenConfiguration::configuration();
                workspace.openURLs_withApplicationAtURL_configuration_completionHandler(
                    &NSArray::from_retained_slice(&[url]),
                    &text_edit,
                    &configuration,
                    None,
                );
            }
        }
    }
}

/// What another app asked the terminal to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Request {
    /// A shell in a folder.
    Folder(PathBuf),
    /// A script to run, as a `.command` or `.tool` file.
    Script(PathBuf),
    /// A connection, as `ssh://user@host:port`.
    Ssh { destination: String, port: Option<u16> },
}

/// What `url` asks for, if the terminal can do it.
pub(crate) fn request(url: &str) -> Option<Request> {
    if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.trim_end_matches('/');
        let (destination, port) = match rest.rsplit_once(':') {
            Some((host, port))
                if port.chars().all(|c| c.is_ascii_digit()) && !host.ends_with(']')
                    || rest.starts_with('[') && rest.contains("]:") =>
            {
                (host.to_string(), port.parse().ok())
            }
            _ => (rest.to_string(), None),
        };
        let destination = percent_decode(&destination);
        let safe = !destination.is_empty()
            && !destination.starts_with('-')
            && destination
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "@.-_[]:%".contains(c));
        return safe.then_some(Request::Ssh { destination, port });
    }
    let path = PathBuf::from(percent_decode(url.strip_prefix("file://")?));
    if path.is_dir() {
        return Some(Request::Folder(path));
    }
    path.is_file().then_some(Request::Script(path))
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(byte) = text
                .get(at + 1..at + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Carries out `request` in a new window.
pub(crate) fn carry_out(request: Request) {
    match request {
        Request::Folder(folder) => {
            Window::open(Some(folder), None);
        }
        Request::Script(script) => {
            let folder = script.parent().map(Path::to_path_buf);
            let shell = term_base::shell_env::user_shell();
            let program = Program::Command {
                program: shell,
                args: vec!["-l".into(), "-c".into(), quoted(&script)],
            };
            Window::open(folder, Some(program));
        }
        Request::Ssh { destination, port } => {
            let mut args = Vec::new();
            if let Some(port) = port {
                args.push("-p".into());
                args.push(port.to_string());
            }
            args.push("--".into());
            args.push(destination);
            Window::open(None, Some(Program::Ssh(args)));
        }
    }
}

/// `path` as one word for a POSIX shell.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_addresses_read_with_and_without_port_and_options_are_refused() {
        assert_eq!(
            request("ssh://me@host.example:2222"),
            Some(Request::Ssh {
                destination: "me@host.example".into(),
                port: Some(2222)
            })
        );
        assert_eq!(
            request("ssh://host/"),
            Some(Request::Ssh {
                destination: "host".into(),
                port: None
            })
        );
        assert_eq!(request("ssh://-oProxyCommand=evil"), None);
        assert_eq!(request("ssh://a;b"), None);
    }

    #[test]
    fn apps_scripts_and_executables_are_shown_not_run() {
        assert!(super::runs(Path::new("/Applications/Safari.app")));
        assert!(super::runs(Path::new("/tmp/x.COMMAND")));
        assert!(super::runs(Path::new("/bin/ls")));
        assert!(!super::runs(Path::new("/etc/hosts")));
        assert!(!super::runs(Path::new("/tmp/notes.md")));
    }

    #[test]
    fn files_and_folders_become_scripts_and_shells() {
        let dir = term_base::testing::scratch("open-request");
        let script = dir.join("run me.command");
        std::fs::write(&script, "echo hi\n").unwrap();
        let encoded = |path: &Path| format!("file://{}", path.to_string_lossy().replace(' ', "%20"));
        assert_eq!(request(&encoded(&dir)), Some(Request::Folder(dir.to_path_buf())));
        assert_eq!(request(&encoded(&script)), Some(Request::Script(script.clone())));
        assert_eq!(quoted(Path::new("/a b/it's")), r"'/a b/it'\''s'");
    }
}
