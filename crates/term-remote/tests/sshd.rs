//! The connection against a real sshd in a Linux container (P5c): a password asked through the askpass helper, an
//! unknown host key confirmed, the programs found there, a command run there, and the end once the container stops.
//! Needs Docker and the network for the image; runs only with `TAPA_SSHD_TESTS=1`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use term_remote::{Connection, State, Target};

const IMAGE: &str = "tapa-sshd-test";
const DOCKERFILE: &str = "FROM alpine:3.20
RUN apk add --no-cache openssh bash \\
 && ssh-keygen -A \\
 && adduser -D -s /bin/bash tapa \\
 && echo 'tapa:secret' | chpasswd \\
 && mkdir -p /home/tapa/project \\
 && chown -R tapa /home/tapa \\
 && sed -i 's/^#\\?PasswordAuthentication.*/PasswordAuthentication yes/' /etc/ssh/sshd_config
EXPOSE 22
CMD [\"/usr/sbin/sshd\", \"-D\", \"-e\"]
";

fn docker(args: &[&str]) -> Option<String> {
    let output = Command::new("docker").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

struct Container(String);

impl Drop for Container {
    fn drop(&mut self) {
        let _ = docker(&["rm", "-f", &self.0]);
    }
}

fn wait_for(connection: &Connection, states: &Mutex<Vec<State>>, done: impl Fn(&State) -> bool) -> State {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let state = connection.state();
        if done(&state) || Instant::now() > deadline {
            let _ = states;
            return state;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn askpass_script(dir: &Path) -> PathBuf {
    let script = dir.join("askpass");
    std::fs::write(
        &script,
        "#!/bin/sh\necho \"$1\" >>\"$(dirname \"$0\")/asked\"\ncase \"$1\" in *yes/no*) echo yes;; *assword*) echo secret;; *) exit 1;; esac\n",
    )
    .expect("the askpass script is written");
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
        .expect("the askpass script runs");
    script
}

#[test]
fn a_workspace_connects_runs_there_and_notices_the_end() {
    if std::env::var_os("TAPA_SSHD_TESTS").is_none() {
        eprintln!("skipped: set TAPA_SSHD_TESTS=1 to run against a container");
        return;
    }
    let dir = std::env::temp_dir().join(format!("tapa-sshd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Dockerfile"), DOCKERFILE).unwrap();
    docker(&["build", "-q", "-t", IMAGE, &dir.to_string_lossy()]).expect("the image builds");
    let container = Container(docker(&["run", "-d", "-p", "127.0.0.1::22", IMAGE]).expect("the container starts"));
    let port: u16 = docker(&["port", &container.0, "22"])
        .and_then(|mapped| mapped.lines().next()?.rsplit(':').next()?.parse().ok())
        .expect("a port");
    std::thread::sleep(Duration::from_secs(1));
    let config = dir.join("config");
    std::fs::write(
        &config,
        format!(
            "Host *\n  UserKnownHostsFile {}\n  StrictHostKeyChecking ask\n  PubkeyAuthentication no\n  PreferredAuthentications password\n",
            dir.join("known_hosts").display()
        ),
    )
    .unwrap();
    let askpass = askpass_script(&dir);
    let target = Target {
        host: "127.0.0.1".into(),
        port: Some(port),
        user: Some("tapa".into()),
        folder: "project".into(),
    };
    let connection = Connection::with_options(
        target,
        dir.join("c"),
        vec![
            ("SSH_ASKPASS".into(), askpass.to_string_lossy().into_owned()),
            ("SSH_ASKPASS_REQUIRE".into(), "force".into()),
        ],
        vec!["-F".into(), config.to_string_lossy().into_owned()],
    );
    let states = Arc::new(Mutex::new(Vec::new()));
    let heard = states.clone();
    connection.open(move |state| heard.lock().unwrap().push(state.clone()));
    let state = wait_for(&connection, &states, |state| !matches!(state, State::Connecting));
    assert_eq!(state, State::Connected, "{:?}", states.lock().unwrap());
    let asked = std::fs::read_to_string(dir.join("asked")).unwrap();
    assert!(asked.contains("yes/no"), "the unknown host key was asked: {asked}");
    assert!(asked.contains("password"), "{asked}");

    let probe = connection.probe(&["bash", "no-such-program"]);
    assert_eq!(probe.installed, ["bash"]);
    assert_eq!(probe.home.as_deref(), Some("/home/tapa"));
    let mut local = Command::new("/usr/local/bin/sh");
    local
        .args(["-c", "printf '%s|%s' \"$PWD\" \"$KEPT\""])
        .env("KEPT", "it's")
        .env("DROPPED", "x");
    let output = connection
        .wrap(&local, &["KEPT"])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&output.stdout), "/home/tapa/project|it's");

    let wrote = connection
        .command("mkdir -p ~/.claude/projects/-home-tapa-project && echo '{}' > ~/.claude/projects/-home-tapa-project/abc-123.jsonl")
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();
    assert!(wrote.status.success(), "{}", String::from_utf8_lossy(&wrote.stderr));
    let forward = connection.forward();
    let transcript = term_remote::claude_transcript_through(&forward, "abc-123").unwrap();
    assert_eq!(transcript.as_deref(), Some(&b"{}\n"[..]));
    assert_eq!(
        term_remote::claude_transcript_through(&forward, "missing-1").unwrap(),
        None
    );

    docker(&["stop", "-t", "0", &container.0]);
    let state = wait_for(&connection, &states, |state| matches!(state, State::Disconnected(_)));
    assert!(matches!(state, State::Disconnected(_)), "{state:?}");
    assert!(
        term_remote::claude_transcript_through(&forward, "abc-123").is_err(),
        "a closed connection is an error, which a waiting tab tries again"
    );
    connection.close();
    let _ = std::fs::remove_dir_all(&dir);
}
