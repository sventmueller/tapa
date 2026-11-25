//! ssh's questions while a connection opens (E25): passwords, an unknown host key, one-time codes. ssh starts the
//! program `SSH_ASKPASS` names with the question as its argument and reads the answer from its output. Tapa names
//! itself; started that way it asks the running app over a socket the app listens on, and the app answers from the
//! keychain or a sheet at the workspace's window.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The socket the helper asks at.
pub const SOCKET_VAR: &str = "TAPA_ASKPASS_SOCKET";
/// Which connection asks, so that another program on the Mac cannot pose as one.
pub const TOKEN_VAR: &str = "TAPA_ASKPASS_TOKEN";

/// The longest question or answer taken, far beyond any ssh asks.
const MOST: u64 = 16 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct Request {
    token: String,
    prompt: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Reply {
    answer: Option<String>,
}

/// What ssh asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A password or a key's passphrase, typed hidden.
    Secret,
    /// Whether to trust a host key ssh has not seen, answered yes or no.
    HostKey,
    /// Anything else, such as a one-time code.
    Text,
}

/// What `prompt` asks for, by ssh's wording.
pub fn kind(prompt: &str) -> Kind {
    let lower = prompt.to_lowercase();
    if lower.contains("(yes/no") {
        Kind::HostKey
    } else if lower.contains("password") || lower.contains("passphrase") {
        Kind::Secret
    } else {
        Kind::Text
    }
}

/// The variables that have ssh ask through `program`, the app's own executable, at `socket` for connection `token`.
pub fn environment(program: &Path, socket: &Path, token: &str) -> Vec<(String, String)> {
    vec![
        ("SSH_ASKPASS".into(), program.to_string_lossy().into_owned()),
        // Also where a terminal is at hand, as a program started from one inherits it.
        ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        (SOCKET_VAR.into(), socket.to_string_lossy().into_owned()),
        (TOKEN_VAR.into(), token.into()),
    ]
}

/// A new token for a connection: 16 random bytes in hex.
pub fn token() -> String {
    let mut bytes = [0u8; 16];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .is_err()
    {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        bytes[..16].copy_from_slice(&now.as_nanos().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Run as ssh's askpass: asks the app for the answer to `prompt` and prints it; the exit status ssh reads, 1 for no
/// answer.
pub fn helper(prompt: &str) -> i32 {
    let (Some(socket), Some(token)) = (std::env::var_os(SOCKET_VAR), std::env::var(TOKEN_VAR).ok()) else {
        return 1;
    };
    match ask(Path::new(&socket), &token, prompt) {
        Ok(Some(answer)) => {
            println!("{answer}");
            0
        }
        _ => 1,
    }
}

fn ask(socket: &Path, token: &str, prompt: &str) -> io::Result<Option<String>> {
    let mut stream = UnixStream::connect(socket)?;
    let request = Request {
        token: token.into(),
        prompt: prompt.into(),
    };
    let mut line = serde_json::to_vec(&request).map_err(io::Error::other)?;
    line.push(b'\n');
    stream.write_all(&line)?;
    let mut reply = String::new();
    BufReader::new(stream.take(MOST)).read_line(&mut reply)?;
    let reply: Reply = serde_json::from_str(&reply).map_err(io::Error::other)?;
    Ok(reply.answer)
}

/// A question from ssh, answered once; dropped without an answer, it tells ssh there is none.
pub struct Question {
    /// The connection that asks.
    pub token: String,
    /// What ssh asks, as it words it.
    pub prompt: String,
    stream: Mutex<Option<UnixStream>>,
}

impl Question {
    /// Answers ssh with `answer`, or with none.
    pub fn answer(&self, answer: Option<String>) {
        let Some(mut stream) = self.stream.lock().ok().and_then(|mut stream| stream.take()) else {
            return;
        };
        if let Ok(mut line) = serde_json::to_vec(&Reply { answer }) {
            line.push(b'\n');
            let _ = stream.write_all(&line);
        }
    }
}

impl Drop for Question {
    fn drop(&mut self) {
        self.answer(None);
    }
}

/// The app's side: listens at a socket only the user can reach and hands each question to a handler.
pub struct Listener {
    path: PathBuf,
}

impl Listener {
    /// Listens at `path`, replacing a socket an earlier run left, and hands each question to `asked` on a thread of
    /// its own.
    pub fn start(path: PathBuf, asked: impl Fn(Question) + Send + 'static) -> io::Result<Listener> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        std::thread::Builder::new()
            .name("remote-askpass".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    if let Some(question) = read(stream) {
                        asked(question);
                    }
                }
            })?;
        Ok(Listener { path })
    }

    /// Where it listens.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read(stream: UnixStream) -> Option<Question> {
    let mut line = String::new();
    BufReader::new(stream.try_clone().ok()?.take(MOST))
        .read_line(&mut line)
        .ok()?;
    let request: Request = serde_json::from_str(&line).ok()?;
    Some(Question {
        token: request.token,
        prompt: request.prompt,
        stream: Mutex::new(Some(stream)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_wording_tells_what_it_asks_for() {
        assert_eq!(kind("sven@box's password: "), Kind::Secret);
        assert_eq!(
            kind("Enter passphrase for key '/Users/x/.ssh/id_ed25519': "),
            Kind::Secret
        );
        assert_eq!(
            kind("Are you sure you want to continue connecting (yes/no/[fingerprint])? "),
            Kind::HostKey
        );
        assert_eq!(kind("Verification code: "), Kind::Text);
        assert_eq!(token().len(), 32);
        assert_ne!(token(), token());
    }

    #[test]
    fn a_question_reaches_the_app_and_its_answer_the_helper() {
        let dir = term_base::testing::scratch("remote-askpass");
        let path = dir.join("a.sock");
        let listener = Listener::start(path.clone(), |question| {
            let answer = (question.token == "t1").then(|| format!("answer to {}", question.prompt));
            question.answer(answer);
        })
        .unwrap();
        assert_eq!(
            ask(listener.path(), "t1", "password?").unwrap().as_deref(),
            Some("answer to password?")
        );
        assert_eq!(ask(listener.path(), "other", "password?").unwrap(), None);
        let silent = Listener::start(dir.join("b.sock"), drop).unwrap();
        assert_eq!(ask(silent.path(), "t1", "x").unwrap(), None, "dropped means no answer");
        drop(listener);
        assert!(!path.exists());
    }
}
