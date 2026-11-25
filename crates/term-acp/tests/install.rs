//! Installing npm and uv packages and archives with stand-ins for npm, node, uv and the network: what each helper
//! must leave behind before the installation counts, and what the user is told when it does not.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use term_base::testing::scratch;

use term_acp::install::{InstallError, Progress, Runtime, Source};
use term_acp::{Installer, Installs, Plan};
use term_net::{Download, Http, Net};
use term_process::Supervisor;

/// Logs each call with whether it may run package scripts to `$NPM_LOG`; `rebuild` is where scripts run.
const NPM: &str = r#"#!/bin/sh
command="$1"; shift; prefix=""; spec=""; scripts=yes
while [ $# -gt 0 ]; do
  case "$1" in --prefix) prefix="$2"; shift ;; --ignore-scripts) scripts=no ;; --*) ;; *) spec="$1" ;; esac
  shift
done
[ -n "$NPM_LOG" ] && echo "$command scripts=$scripts" >> "$NPM_LOG"
if [ "$command" = rebuild ]; then
  if [ -n "$NPM_SCRIPT_FAIL" ]; then echo "postinstall failed" >&2; exit 1; fi
  exit 0
fi
package="${spec%@*}"; version="${spec##*@}"; short="${package##*/}"
if [ -n "$NPM_FAIL" ]; then echo "npm error 404 $spec" >&2; exit 1; fi
mkdir -p "$prefix/node_modules/$package" "$prefix/node_modules/.bin"
printf '{"name":"%s","bin":%s}' "$package" "${NPM_BIN:-\"cli.js\"}" > "$prefix/node_modules/$package/package.json"
printf '#!/bin/sh\necho %s\n' "$version" > "$prefix/node_modules/.bin/$short"
chmod +x "$prefix/node_modules/.bin/$short"
printf '{"packages":{"node_modules/%s":{"version":"%s","integrity":"sha512-good"}}}' "$package" "${NPM_VERSION:-$version}" > "$prefix/package-lock.json"
"#;

const UV: &str = r#"#!/bin/sh
spec="$4"; package="${spec%%==*}"
if [ -n "$UV_FAIL" ]; then echo "No solution found for $spec" >&2; exit 1; fi
mkdir -p "$UV_TOOL_BIN_DIR"
[ -n "$UV_NO_PROGRAM" ] && exit 0
printf '#!/bin/sh\necho %s\n' "$spec" > "$UV_TOOL_BIN_DIR/$package"
chmod +x "$UV_TOOL_BIN_DIR/$package"
"#;

#[derive(Default)]
struct FakeNet {
    files: Mutex<Vec<(String, Vec<u8>)>>,
}

impl Net for FakeNet {
    fn get(&self, url: &str, _: &str) -> io::Result<Download> {
        let files = self.files.lock().unwrap();
        let bytes = files
            .iter()
            .find(|(u, _)| u == url)
            .map(|(_, b)| b.clone())
            .ok_or_else(|| io::Error::other(format!("404 {url}")))?;
        Ok(Download {
            length: None,
            body: Box::new(io::Cursor::new(bytes)),
        })
    }
}

fn script(path: &Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
}

/// A folder of stand-in helpers: npm and uv always, node printing `node` when given.
fn helpers(dir: &Path, node: Option<&str>) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    script(&bin.join("npm"), NPM);
    script(&bin.join("uv"), UV);
    if let Some(version) = node {
        script(&bin.join("node"), &format!("#!/bin/sh\necho {version}\n"));
    }
    bin
}

fn installer(dir: &Path, bin: &Path, vars: &[(&str, &str)], net: Arc<dyn Net>) -> (Installer, Installs) {
    let mut env = vec![("PATH".to_string(), format!("{}:/usr/bin:/bin", bin.display()))];
    env.extend(vars.iter().map(|(k, v)| ((*k).to_string(), (*v).to_string())));
    let installs = Installs::new(dir.join("agents"));
    (Installer::new(&Supervisor::new(), env, installs.clone(), net), installs)
}

fn npm_plan(package: &str, integrity: Option<&str>, bin: Option<&str>) -> Plan {
    Plan {
        id: "node-agent".into(),
        name: "Node Agent".into(),
        version: "1.4.2".into(),
        source: Source::Npm {
            package: package.into(),
            version: "1.4.2".into(),
            integrity: integrity.map(str::to_string),
            bin: bin.map(str::to_string),
            args: vec!["--acp".into()],
            env: vec![("NODE_AGENT".into(), "1".into())],
        },
        verified: true,
        checksum: integrity.is_some(),
        runtime: Runtime::Node(Some(20)),
    }
}

fn uv_plan() -> Plan {
    Plan {
        id: "py-agent".into(),
        name: "Py Agent".into(),
        version: "0.3.0".into(),
        source: Source::Uv {
            package: "py-agent".into(),
            version: "0.3.0".into(),
            args: vec!["acp".into()],
            env: Vec::new(),
        },
        verified: true,
        checksum: false,
        runtime: Runtime::Uv,
    }
}

fn run(program: &Path) -> String {
    let output = std::process::Command::new(program).output().unwrap();
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[test]
fn an_npm_package_installs_with_its_integrity_and_its_program_runs() {
    let dir = scratch("npm");
    let bin = helpers(&dir, Some("v22.3.0"));
    let (installer, installs) = installer(&dir, &bin, &[], Arc::new(FakeNet::default()));
    let plan = npm_plan("@acme/node-agent", Some("sha512-good"), None);
    assert!(installer.missing(&plan).is_none(), "Node 22 is new enough");
    let stages = Mutex::new(Vec::new());
    let installed = installer
        .install(&plan, false, &|p| stages.lock().unwrap().push(p))
        .unwrap();
    assert_eq!(*stages.lock().unwrap(), [Progress::Checking, Progress::Installing]);
    assert_eq!(installed.program, Path::new("node_modules/.bin/node-agent"));
    assert_eq!(installed.args, ["--acp"]);
    let (current, program) = installs.current("node-agent").unwrap();
    assert_eq!(current, installed);
    assert_eq!(run(&program), "1.4.2");
}

#[test]
fn npm_counts_only_with_the_named_version_integrity_and_program() {
    let dir = scratch("npm-refused");
    let bin = helpers(&dir, Some("v22.3.0"));
    let net: Arc<dyn Net> = Arc::new(FakeNet::default());
    let install = |vars: &[(&str, &str)], plan: &Plan| {
        let (installer, installs) = installer(&dir, &bin, vars, net.clone());
        let result = installer.install(plan, true, &|_| {});
        assert!(
            !dir.join("agents/node-agent/1.4.2").exists(),
            "a failed installation leaves no folder"
        );
        assert!(installs.current("node-agent").is_none());
        result.unwrap_err()
    };
    let plain = npm_plan("node-agent", None, None);
    match install(&[("NPM_FAIL", "1")], &plain) {
        InstallError::Tool(stderr) => assert_eq!(stderr.trim(), "npm error 404 node-agent@1.4.2"),
        other => panic!("{other:?}"),
    }
    match install(&[("NPM_VERSION", "1.4.3")], &plain) {
        InstallError::Unusable(why) => assert_eq!(why, "npm did not install node-agent@1.4.2"),
        other => panic!("{other:?}"),
    }
    match install(&[], &npm_plan("node-agent", Some("sha512-pinned"), None)) {
        InstallError::Checksum { expected, actual } => {
            assert_eq!((expected.as_str(), actual.as_str()), ("sha512-pinned", "sha512-good"))
        }
        other => panic!("{other:?}"),
    }
    match install(&[], &npm_plan("node-agent", None, Some("elsewhere"))) {
        InstallError::Unusable(why) => assert_eq!(why, "node-agent has no program elsewhere"),
        other => panic!("{other:?}"),
    }
    match install(&[("NPM_BIN", r#"{"a":"a.js","b":"b.js"}"#)], &plain) {
        InstallError::Unusable(why) => assert_eq!(why, "node-agent names no single program"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn package_scripts_run_only_after_the_integrity_is_checked() {
    let dir = scratch("npm-scripts");
    let bin = helpers(&dir, Some("v22.3.0"));
    let log = dir.join("npm.log");
    let log_text = log.to_string_lossy().into_owned();
    let net: Arc<dyn Net> = Arc::new(FakeNet::default());
    let calls = |vars: &[(&str, &str)], plan: &Plan| {
        let _ = std::fs::remove_file(&log);
        let mut vars = vars.to_vec();
        vars.push(("NPM_LOG", &log_text));
        let (installer, _) = installer(&dir, &bin, &vars, net.clone());
        let result = installer.install(plan, true, &|_| {});
        (result, std::fs::read_to_string(&log).unwrap_or_default())
    };
    let (result, log_after) = calls(&[], &npm_plan("node-agent", Some("sha512-good"), None));
    assert!(result.is_ok(), "{result:?}");
    assert_eq!(log_after, "install scripts=no\nrebuild scripts=yes\n");
    let (result, log_after) = calls(&[], &npm_plan("node-agent", Some("sha512-pinned"), None));
    assert!(matches!(result, Err(InstallError::Checksum { .. })), "{result:?}");
    assert_eq!(
        log_after, "install scripts=no\n",
        "no script ran for a package that failed its check"
    );
    let (result, _) = calls(&[("NPM_SCRIPT_FAIL", "1")], &npm_plan("node-agent", None, None));
    match result {
        Err(InstallError::Tool(stderr)) => assert_eq!(stderr.trim(), "postinstall failed"),
        other => panic!("{other:?}"),
    }
    assert!(!dir.join("agents/node-agent/1.4.2").exists());
}

#[test]
fn a_package_with_several_programs_installs_the_one_named_like_it() {
    let dir = scratch("npm-bins");
    let bin = helpers(&dir, Some("v20.0.0"));
    let (installer, _) = installer(
        &dir,
        &bin,
        &[("NPM_BIN", r#"{"node-agent":"cli.js","helper":"h.js"}"#)],
        Arc::new(FakeNet::default()),
    );
    let installed = installer
        .install(&npm_plan("node-agent", None, None), true, &|_| {})
        .unwrap();
    assert_eq!(installed.program, Path::new("node_modules/.bin/node-agent"));
}

#[test]
fn node_too_old_or_missing_and_a_missing_uv_are_named() {
    let dir = scratch("runtimes");
    let net: Arc<dyn Net> = Arc::new(FakeNet::default());
    let plan = npm_plan("node-agent", None, None);
    let old = helpers(&dir.join("old"), Some("v18.19.0"));
    let (with_old, _) = installer(&dir, &old, &[], net.clone());
    assert!(matches!(
        with_old.missing(&plan),
        Some(InstallError::Runtime("Node.js", Some(20)))
    ));
    assert!(matches!(
        with_old.install(&plan, true, &|_| {}),
        Err(InstallError::Runtime("Node.js", Some(20)))
    ));
    let none = helpers(&dir.join("none"), None);
    let (without_node, _) = installer(&dir, &none, &[], net.clone());
    assert!(matches!(
        without_node.missing(&plan),
        Some(InstallError::Runtime("Node.js", Some(20)))
    ));
    let any = Plan {
        runtime: Runtime::Node(None),
        ..plan
    };
    assert!(with_old.missing(&any).is_none(), "any Node does without a minimum");
    assert!(with_old.missing(&uv_plan()).is_none());

    let empty = dir.join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let (bare, _) = installer(&dir, &empty, &[], net);
    assert!(matches!(
        bare.missing(&uv_plan()),
        Some(InstallError::Runtime("uv", None))
    ));
    assert!(matches!(
        bare.missing(&any),
        Some(InstallError::Runtime("Node.js", None))
    ));
}

#[test]
fn a_uv_package_installs_its_program_of_the_same_name() {
    let dir = scratch("uv");
    let bin = helpers(&dir, None);
    let net: Arc<dyn Net> = Arc::new(FakeNet::default());
    let (installer_ok, installs) = installer(&dir, &bin, &[], net.clone());
    assert!(matches!(
        installer_ok.install(&uv_plan(), false, &|_| {}),
        Err(InstallError::NotConfirmed)
    ));
    let installed = installer_ok.install(&uv_plan(), true, &|_| {}).unwrap();
    assert_eq!(installed.program, Path::new("bin/py-agent"));
    let (_, program) = installs.current("py-agent").unwrap();
    assert_eq!(run(&program), "py-agent==0.3.0");

    let (failing, _) = installer(&dir, &bin, &[("UV_FAIL", "1")], net.clone());
    match failing.install(&uv_plan(), true, &|_| {}) {
        Err(InstallError::Tool(stderr)) => assert_eq!(stderr.trim(), "No solution found for py-agent==0.3.0"),
        other => panic!("{other:?}"),
    }
    let (empty, _) = installer(&dir, &bin, &[("UV_NO_PROGRAM", "1")], net);
    match empty.install(&uv_plan(), true, &|_| {}) {
        Err(InstallError::Unusable(why)) => assert_eq!(why, "py-agent installed no program of its name"),
        other => panic!("{other:?}"),
    }
}

fn archive_plan(url: &str, cmd: &str) -> Plan {
    Plan {
        id: "bin-agent".into(),
        name: "Bin Agent".into(),
        version: "2.0.0".into(),
        source: Source::Archive {
            url: url.into(),
            sha256: None,
            cmd: cmd.into(),
            args: Vec::new(),
            env: Vec::new(),
        },
        verified: false,
        checksum: false,
        runtime: Runtime::None,
    }
}

/// A gzipped tarball holding `dist/agent`, a script that prints `tarball`.
fn tarball(dir: &Path) -> Vec<u8> {
    let content = dir.join("content");
    std::fs::create_dir_all(content.join("dist")).unwrap();
    script(&content.join("dist/agent"), "#!/bin/sh\necho tarball\n");
    let file = dir.join("agent.tar.gz");
    let status = std::process::Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&file)
        .arg("-C")
        .arg(&content)
        .arg("dist")
        .status()
        .unwrap();
    assert!(status.success());
    std::fs::read(file).unwrap()
}

#[test]
fn a_tarball_unpacks_with_tar_and_its_program_must_be_inside_it() {
    let dir = scratch("tarball");
    let net = Arc::new(FakeNet::default());
    let url = "https://example.test/agent.tar.gz";
    let bytes = tarball(&dir);
    let size = bytes.len() as u64;
    net.files.lock().unwrap().push((url.into(), bytes));
    net.files
        .lock()
        .unwrap()
        .push(("https://example.test/broken.tar.gz".into(), b"not a tarball".to_vec()));
    let (installer, installs) = installer(&dir, &dir, &[], net);

    let stages = Mutex::new(Vec::new());
    let installed = installer
        .install(&archive_plan(url, "dist/agent"), true, &|p| {
            stages.lock().unwrap().push(p)
        })
        .unwrap();
    assert_eq!(
        stages.into_inner().unwrap(),
        [Progress::Checking, Progress::Downloading(size), Progress::Unpacking]
    );
    assert_eq!(installed.program, Path::new("dist/agent"));
    assert_eq!(run(&installs.current("bin-agent").unwrap().1), "tarball");

    let failure = |plan: &Plan| installer.install(plan, true, &|_| {}).unwrap_err().to_string();
    assert_eq!(
        failure(&archive_plan(url, "dist/missing")),
        "dist/missing is not in the archive"
    );
    assert_eq!(
        failure(&archive_plan(url, "../../escape")),
        "../../escape leaves its folder"
    );
    assert!(failure(&archive_plan("https://example.test/broken.tar.gz", "agent")).contains("tar"));
    assert_eq!(
        failure(&archive_plan("https://example.test/gone.zip", "agent")),
        "the download failed: 404 https://example.test/gone.zip"
    );
}

#[test]
fn a_program_that_links_out_of_its_folder_is_refused_and_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch("tarball-link");
    let outside = dir.join("outside");
    std::fs::write(&outside, "secret\n").unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o600)).unwrap();
    let content = dir.join("content");
    std::fs::create_dir_all(content.join("dist")).unwrap();
    std::os::unix::fs::symlink(&outside, content.join("dist/agent")).unwrap();
    let file = dir.join("linked.tar.gz");
    let status = std::process::Command::new("/usr/bin/tar")
        .arg("-czf")
        .arg(&file)
        .arg("-C")
        .arg(&content)
        .arg("dist")
        .status()
        .unwrap();
    assert!(status.success());
    let net = Arc::new(FakeNet::default());
    let url = "https://example.test/linked.tar.gz";
    net.files
        .lock()
        .unwrap()
        .push((url.into(), std::fs::read(&file).unwrap()));
    let (installer, installs) = installer(&dir, &dir, &[], net);
    let error = installer
        .install(&archive_plan(url, "dist/agent"), true, &|_| {})
        .unwrap_err();
    assert_eq!(error.to_string(), "dist/agent leaves its folder");
    assert!(installs.current("bin-agent").is_none());
    let mode = std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "nothing outside the folder is made executable");
}

#[test]
fn an_id_or_version_that_is_no_folder_name_installs_nothing() {
    let dir = scratch("tarball-names");
    let keep = dir.join("keep");
    std::fs::create_dir_all(keep.join("inside")).unwrap();
    let net = Arc::new(FakeNet::default());
    let url = "https://example.test/agent.tar.gz";
    net.files.lock().unwrap().push((url.into(), tarball(&dir)));
    let (installer, _) = installer(&dir, &dir, &[], net);
    for (id, version) in [
        ("bin-agent", "../../keep"),
        ("bin-agent", "1.0.0-x/../../../keep"),
        ("bin-agent", ""),
        ("..", "2.0.0"),
        ("a/b", "2.0.0"),
    ] {
        let plan = Plan {
            id: id.into(),
            version: version.into(),
            ..archive_plan(url, "dist/agent")
        };
        let error = installer.install(&plan, true, &|_| {}).unwrap_err();
        assert!(matches!(error, InstallError::Unusable(_)), "{id} {version}: {error:?}");
    }
    assert!(
        keep.join("inside").is_dir(),
        "nothing outside the agents folder was removed"
    );
}

#[test]
fn install_errors_tell_what_went_wrong() {
    let texts = [
        InstallError::NotConfirmed.to_string(),
        InstallError::Runtime("Node.js", Some(20)).to_string(),
        InstallError::Runtime("uv", None).to_string(),
        InstallError::Checksum {
            expected: "aa".into(),
            actual: "bb".into(),
        }
        .to_string(),
        InstallError::from(io::Error::other("disk full")).to_string(),
    ];
    assert_eq!(
        texts,
        [
            "the installation was not confirmed",
            "Node.js 20 or newer is needed",
            "uv is needed",
            "the download's checksum is bb, not aa",
            "disk full",
        ]
    );
}

#[test]
fn text_from_the_network_is_read_up_to_its_limit() {
    let net = FakeNet::default();
    net.files.lock().unwrap().push((
        "https://example.test/registry.json".into(),
        b"{\"agents\": []}".to_vec(),
    ));
    assert_eq!(
        net.get_text("https://example.test/registry.json", 64).unwrap(),
        "{\"agents\": []}"
    );
    let error = net.get_text("https://example.test/registry.json", 4).unwrap_err();
    assert_eq!(
        error.to_string(),
        "https://example.test/registry.json is larger than 4 bytes"
    );
    assert!(net.get_text("https://example.test/none", 64).is_err());
}

#[test]
fn the_http_client_fetches_only_https() {
    let http = Http::new("agent register");
    let error = http.get("http://example.test/registry.json", "*/*").err().unwrap();
    assert_eq!(
        error.to_string(),
        "http://example.test/registry.json is not an HTTPS address"
    );
    // Port 1 on the loopback refuses at once, so the request fails without reaching any network.
    assert!(http.get("https://127.0.0.1:1/registry.json", "*/*").is_err());
}
