//! The transcripts the Claude CLI keeps on the other machine (E25): an agent tab that opens again shows its
//! conversation from the session's transcript, which for a remote workspace lies there.

use std::io;
use std::path::Path;
use std::process::Stdio;

use crate::registry::bin_for;
use crate::script::quote;
use crate::stand_in::{FORWARD, Forward};

/// The exit code of the script when the machine has no transcript of the session.
const NOT_THERE: i32 = 3;

/// The transcript of Claude's session `session` on the machine of the remote workspace `folder` belongs to: `None`
/// where `folder` is no remote workspace's or the machine has no such transcript, an error while its connection is
/// not open.
pub fn claude_transcript(folder: &Path, session: &str) -> io::Result<Option<Vec<u8>>> {
    let Some(bin) = bin_for(folder) else {
        return Ok(None);
    };
    let forward: Forward = serde_json::from_slice(&std::fs::read(bin.join(FORWARD))?).map_err(io::Error::other)?;
    claude_transcript_through(&forward, session)
}

/// `claude_transcript` through `forward`.
pub fn claude_transcript_through(forward: &Forward, session: &str) -> io::Result<Option<Vec<u8>>> {
    if session.is_empty()
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Ok(None);
    }
    let output = forward.command(&script(session)).stdin(Stdio::null()).output()?;
    match output.status.code() {
        Some(0) => Ok(Some(output.stdout)),
        Some(NOT_THERE) => Ok(None),
        _ => Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        )),
    }
}

/// Prints the transcript of `session` from the CLI's configuration folder there, whatever project folder it is in.
fn script(session: &str) -> String {
    format!(
        "f=$(find \"${{CLAUDE_CONFIG_DIR:-$HOME/.claude}}/projects\" -maxdepth 2 -type f -name {} 2>/dev/null | head -n 1); \
         [ -n \"$f\" ] || exit {NOT_THERE}; exec cat \"$f\"",
        quote(&format!("{session}.jsonl"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_looks_for_the_sessions_file_in_the_configuration_there() {
        let script = script("51e1f7e5-4000");
        assert!(
            script.contains("${CLAUDE_CONFIG_DIR:-$HOME/.claude}/projects"),
            "{script}"
        );
        assert!(script.contains("-name '51e1f7e5-4000.jsonl'"), "{script}");
        assert!(script.contains("exit 3"), "{script}");
    }

    #[test]
    fn a_session_id_that_could_reach_the_shell_asks_nothing() {
        let forward = Forward {
            target: crate::Target {
                host: "box".into(),
                port: None,
                user: None,
                folder: String::new(),
            },
            control: "/nonexistent/c".into(),
            options: Vec::new(),
        };
        for session in ["", "a;b", "../x", "a b", "$(x)"] {
            assert!(
                claude_transcript_through(&forward, session).unwrap().is_none(),
                "{session}"
            );
        }
        assert!(claude_transcript(Path::new("/not/remote"), "abc").unwrap().is_none());
    }
}
