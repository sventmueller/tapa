//! Installing an agent into the app's data folder as Dango does (E21): the version a preset pins, or the exact
//! version a registry entry names, never an unversioned package. Downloads are checked against their checksum; what
//! has none, or was not verified, installs only after the user confirmed it. Each agent keeps its previous version, so
//! an update can be taken back.
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use term_process::Supervisor;

use crate::register::{self, Distribution, Entry, Pinned, Preset};
use crate::tool::Runner;
use term_net::{Cancel, DownloadError, Net};

/// How long `npm install` or `uv tool install` may take.
const PACKAGE_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// How long unpacking or a version check may take.
const SHORT_TIMEOUT: Duration = Duration::from_secs(120);

/// The largest archive downloaded.
const MAX_ARCHIVE_BYTES: u64 = 1024 * 1024 * 1024;

/// Where an installation comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub enum Source {
    /// An archive to download and unpack.
    Archive {
        /// Its address.
        url: String,
        /// Its SHA-256, when known.
        sha256: Option<String>,
        /// The program inside it.
        cmd: String,
        /// The program's arguments.
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// A Node package installed with npm.
    Npm {
        /// The package.
        package: String,
        /// Its exact version.
        version: String,
        /// The integrity npm's lock file must name for it, when known.
        integrity: Option<String>,
        /// The program it installs, when known; otherwise read from its `package.json`.
        bin: Option<String>,
        /// The program's arguments.
        args: Vec<String>,
        /// Variables for its environment.
        env: Vec<(String, String)>,
    },
    /// A Python package installed with `uv tool install`.
    Uv {
        /// The package, which is also the program's name.
        package: String,
        /// Its exact version.
        version: String,
        /// The program's arguments.
        args: Vec<String>,
        /// Variables for its environment.
        env: Vec<(String, String)>,
    },
}

/// What an installation needs on the Mac besides itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// Nothing.
    None,
    /// Node and npm, at least this major version where the package names one.
    Node(Option<u32>),
    /// uv.
    Uv,
}

/// What installing an agent would do, shown to the user before it happens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The registry's id.
    pub id: String,
    /// The name shown.
    pub name: String,
    /// The version installed.
    pub version: String,
    /// Where it comes from.
    pub source: Source,
    /// Whether Dango's `make test-integration` verified exactly this combination.
    pub verified: bool,
    /// Whether what is downloaded is checked against a checksum.
    pub checksum: bool,
    /// What it needs on the Mac.
    pub runtime: Runtime,
}

/// Why a registry entry is not installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// Its package names no exact version, so what runs could change at any time.
    Unversioned(String),
    /// It offers nothing for this Mac that this build can install.
    Nothing {
        /// Ways of distribution this build does not know.
        unknown: Vec<String>,
    },
}

impl Plan {
    /// The installation a preset pins; a tested combination only once the preset is verified.
    pub fn of_preset(preset: &Preset) -> Plan {
        let source = match &preset.pinned {
            Pinned::Npm {
                package,
                version,
                integrity,
                bin,
                args,
                ..
            } => Source::Npm {
                package: (*package).into(),
                version: (*version).into(),
                integrity: Some((*integrity).into()),
                bin: Some((*bin).into()),
                args: args.iter().map(|a| (*a).to_string()).collect(),
                env: Vec::new(),
            },
            Pinned::Binary {
                arm64,
                x86_64,
                cmd,
                args,
                ..
            } => {
                let (url, sha) = if register::target() == "darwin-aarch64" {
                    arm64
                } else {
                    x86_64
                };
                Source::Archive {
                    url: (*url).into(),
                    sha256: Some((*sha).into()),
                    cmd: (*cmd).into(),
                    args: args.iter().map(|a| (*a).to_string()).collect(),
                    env: Vec::new(),
                }
            }
        };
        let runtime = match preset.pinned {
            Pinned::Npm { node_major, .. } => Runtime::Node(node_major),
            Pinned::Binary { .. } => Runtime::None,
        };
        Plan {
            id: preset.id.into(),
            name: preset.name.into(),
            version: preset.pinned.version().into(),
            source,
            verified: preset.verified,
            checksum: true,
            runtime,
        }
    }

    /// The installation a registry entry offers: its archive for this Mac, else its npm or uv package.
    pub fn of_entry(entry: &Entry) -> Result<Plan, Refusal> {
        let plan = |source: Source, checksum: bool, runtime: Runtime, version: &str| Plan {
            id: entry.id.clone(),
            name: entry.name.clone(),
            version: version.to_string(),
            source,
            verified: false,
            checksum,
            runtime,
        };
        let mut unversioned = None;
        for distribution in &entry.distributions {
            match distribution {
                Distribution::Binary {
                    archive,
                    cmd,
                    args,
                    env,
                    sha256,
                } => {
                    let source = Source::Archive {
                        url: archive.clone(),
                        sha256: sha256.clone(),
                        cmd: cmd.clone(),
                        args: args.clone(),
                        env: env.clone(),
                    };
                    return Ok(plan(source, sha256.is_some(), Runtime::None, &entry.version));
                }
                Distribution::Npm(p) | Distribution::Uvx(p) => {
                    let Some(version) = p.version.clone().filter(|v| register::is_exact(v)) else {
                        unversioned = Some(p.name.clone());
                        continue;
                    };
                    let (source, runtime) = if matches!(distribution, Distribution::Npm(_)) {
                        let source = Source::Npm {
                            package: p.name.clone(),
                            version: version.clone(),
                            integrity: None,
                            bin: None,
                            args: p.args.clone(),
                            env: p.env.clone(),
                        };
                        (source, Runtime::Node(None))
                    } else {
                        let source = Source::Uv {
                            package: p.name.clone(),
                            version: version.clone(),
                            args: p.args.clone(),
                            env: p.env.clone(),
                        };
                        (source, Runtime::Uv)
                    };
                    // npm and uv check the package against their registry's own hashes.
                    return Ok(plan(source, true, runtime, &version));
                }
            }
        }
        match unversioned {
            Some(name) => Err(Refusal::Unversioned(name)),
            None => Err(Refusal::Nothing {
                unknown: entry.unknown.clone(),
            }),
        }
    }

    /// Whether the user has to confirm it first: it was not verified with this build, or nothing checks the download.
    pub fn needs_confirmation(&self) -> bool {
        !self.verified || !self.checksum
    }
}

/// An installed version of an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(missing_docs, reason = "the names say it")]
pub struct Installed {
    pub version: String,
    /// The program, relative to the version's folder.
    pub program: PathBuf,
    pub args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    /// Whether it was installed as a combination verified with this build.
    pub verified: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    current: Option<Installed>,
    previous: Option<Installed>,
}

/// The installed agents: `<root>/<id>/<version>/` and a `state.json` per agent naming the current and the previous
/// version.
#[derive(Debug, Clone)]
pub struct Installs {
    root: PathBuf,
}

impl Installs {
    /// The agents installed under `root`.
    pub fn new(root: impl Into<PathBuf>) -> Installs {
        Installs { root: root.into() }
    }

    /// The app's agents folder.
    pub fn default_root() -> PathBuf {
        term_base::data_dir().join("agents")
    }

    fn folder(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    fn version_folder(&self, id: &str, version: &str) -> PathBuf {
        self.folder(id).join(version)
    }

    fn state(&self, id: &str) -> State {
        fs::read(self.folder(id).join("state.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn save(&self, id: &str, state: &State) -> io::Result<()> {
        term_base::atomic_write::write_json(&self.folder(id).join("state.json"), state)
    }

    /// The version in use and its program's full path.
    pub fn current(&self, id: &str) -> Option<(Installed, PathBuf)> {
        let installed = self.state(id).current?;
        let program = self.version_folder(id, &installed.version).join(&installed.program);
        program.is_file().then_some((installed, program))
    }

    /// The version used before the last update, which `rollback` returns to.
    pub fn previous(&self, id: &str) -> Option<Installed> {
        self.state(id)
            .previous
            .filter(|p| self.version_folder(id, &p.version).is_dir())
    }

    /// Goes back to the previous version; the one left becomes the previous, so the step can be undone.
    pub fn rollback(&self, id: &str) -> io::Result<Installed> {
        let state = self.state(id);
        let previous = self
            .previous(id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no previous version"))?;
        self.save(
            id,
            &State {
                current: Some(previous.clone()),
                previous: state.current,
            },
        )?;
        Ok(previous)
    }

    /// Makes `installed` the current version, keeps the one before as the previous and removes any older one.
    fn record(&self, id: &str, installed: Installed) -> io::Result<()> {
        let state = self.state(id);
        let previous = match state.current {
            Some(current) if current.version != installed.version => Some(current),
            _ => state.previous.filter(|p| p.version != installed.version),
        };
        let keep: Vec<String> = [Some(&installed), previous.as_ref()]
            .into_iter()
            .flatten()
            .map(|i| i.version.clone())
            .collect();
        self.save(
            id,
            &State {
                current: Some(installed),
                previous,
            },
        )?;
        for old in fs::read_dir(self.folder(id))?.flatten() {
            let name = old.file_name().to_string_lossy().into_owned();
            if old.path().is_dir() && !keep.contains(&name) {
                let _ = fs::remove_dir_all(old.path());
            }
        }
        Ok(())
    }
}

/// A stage of an installation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    /// Checking what the installation needs.
    Checking,
    /// Downloading; the bytes so far.
    Downloading(u64),
    /// Unpacking the archive.
    Unpacking,
    /// npm or uv installs the package.
    Installing,
}

/// Why an installation failed.
#[derive(Debug)]
pub enum InstallError {
    /// The plan needs the user's confirmation, which was not given.
    NotConfirmed,
    /// A program the installation needs is missing or too old: its name and the version needed, if any.
    Runtime(&'static str, Option<u32>),
    /// The download failed.
    Net(io::Error),
    /// The download is not what its checksum says.
    Checksum {
        /// The checksum expected.
        expected: String,
        /// The checksum of what arrived.
        actual: String,
    },
    /// A helper such as npm failed; its error output.
    Tool(String),
    /// What was installed cannot be used.
    Unusable(String),
    /// Reading or writing the folder failed.
    Io(io::Error),
}

impl fmt::Display for InstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InstallError::NotConfirmed => write!(f, "the installation was not confirmed"),
            InstallError::Runtime(name, Some(major)) => write!(f, "{name} {major} or newer is needed"),
            InstallError::Runtime(name, None) => write!(f, "{name} is needed"),
            InstallError::Net(e) => write!(f, "the download failed: {e}"),
            InstallError::Checksum { expected, actual } => {
                write!(f, "the download's checksum is {actual}, not {expected}")
            }
            InstallError::Tool(stderr) => write!(f, "{stderr}"),
            InstallError::Unusable(why) => write!(f, "{why}"),
            InstallError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl term_base::explain::Explain for InstallError {
    fn explain(&self) -> String {
        use splat_i18n::{t, tf};
        match self {
            InstallError::NotConfirmed => t("Die Installation wurde nicht bestätigt.").to_string(),
            InstallError::Runtime(name, Some(major)) => {
                tf("{} {} oder neuer wird gebraucht", &[*name, &major.to_string()])
            }
            InstallError::Runtime(name, None) => tf("{} wird gebraucht", &[name]),
            InstallError::Net(error) => tf("Der Download ist fehlgeschlagen: {}", &[&error.explain()]),
            InstallError::Checksum { .. } => {
                t("Der Download stimmt nicht mit seiner Prüfsumme überein und wurde verworfen").to_string()
            }
            InstallError::Tool(said) | InstallError::Unusable(said) => said.clone(),
            InstallError::Io(error) => error.explain(),
        }
    }
}

impl std::error::Error for InstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            InstallError::Net(error) | InstallError::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> InstallError {
        InstallError::Io(e)
    }
}

/// Whether `name` is one plain folder name, as an agent's id and version must be to name a folder inside the agents
/// folder.
fn plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\0'])
}

/// `path` inside `folder`, refusing anything that climbs out of it.
fn inside(folder: &Path, path: &str) -> Option<PathBuf> {
    let relative = Path::new(path);
    let clean = relative
        .components()
        .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    clean.then(|| {
        folder.join(
            relative
                .components()
                .filter(|c| *c != Component::CurDir)
                .collect::<PathBuf>(),
        )
    })
}

/// Installs agents; every helper runs through the supervisor with the login shell's environment.
pub struct Installer {
    runner: Runner,
    installs: Installs,
    net: Arc<dyn Net>,
}

impl Installer {
    /// An installer into `installs` that reaches the network through `net`.
    pub fn new(
        supervisor: &Supervisor,
        env: Vec<(String, String)>,
        installs: Installs,
        net: Arc<dyn Net>,
    ) -> Installer {
        Installer {
            runner: Runner::new(supervisor, env),
            installs,
            net,
        }
    }

    /// What keeps `plan` from installing on this Mac, checked before the user is asked.
    pub fn missing(&self, plan: &Plan) -> Option<InstallError> {
        match plan.runtime {
            Runtime::None => None,
            Runtime::Uv => self
                .runner
                .which("uv")
                .is_none()
                .then_some(InstallError::Runtime("uv", None)),
            Runtime::Node(major) => {
                if self.runner.which("npm").is_none() {
                    return Some(InstallError::Runtime("Node.js", major));
                }
                let found = self.node_major();
                match (found, major) {
                    (None, _) => Some(InstallError::Runtime("Node.js", major)),
                    (Some(found), Some(needed)) if found < needed => Some(InstallError::Runtime("Node.js", major)),
                    _ => None,
                }
            }
        }
    }

    fn node_major(&self) -> Option<u32> {
        let node = self.runner.which("node")?;
        let output = self
            .runner
            .run(&node, &["--version"], Path::new("/"), &[], SHORT_TIMEOUT)
            .ok()?;
        let version = output.stdout.trim().trim_start_matches('v');
        version.split('.').next()?.parse().ok()
    }

    /// Installs `plan` and makes it the agent's current version; `confirmed` says the user agreed where the plan
    /// asks for it.
    pub fn install(
        &self,
        plan: &Plan,
        confirmed: bool,
        progress: &dyn Fn(Progress),
    ) -> Result<Installed, InstallError> {
        if plan.needs_confirmation() && !confirmed {
            return Err(InstallError::NotConfirmed);
        }
        // A registry entry names both, and the version's folder is emptied before it is filled.
        if let Some(name) = [&plan.id, &plan.version].into_iter().find(|name| !plain_name(name)) {
            return Err(InstallError::Unusable(format!("{name} leaves its folder")));
        }
        progress(Progress::Checking);
        if let Some(problem) = self.missing(plan) {
            return Err(problem);
        }
        let folder = self.installs.version_folder(&plan.id, &plan.version);
        if folder.exists() {
            fs::remove_dir_all(&folder)?;
        }
        fs::create_dir_all(&folder)?;
        let result = match &plan.source {
            Source::Archive {
                url,
                sha256,
                cmd,
                args,
                env,
            } => self
                .archive(&folder, url, sha256.as_deref(), cmd, progress)
                .map(|p| (p, args, env)),
            Source::Npm {
                package,
                version,
                integrity,
                bin,
                args,
                env,
            } => {
                progress(Progress::Installing);
                self.npm(&folder, package, version, integrity.as_deref(), bin.as_deref())
                    .map(|p| (p, args, env))
            }
            Source::Uv {
                package,
                version,
                args,
                env,
            } => {
                progress(Progress::Installing);
                self.uv(&folder, package, version).map(|p| (p, args, env))
            }
        };
        let (program, args, env) = match result {
            Ok(found) => found,
            Err(error) => {
                let _ = fs::remove_dir_all(&folder);
                return Err(error);
            }
        };
        let installed = Installed {
            version: plan.version.clone(),
            program,
            args: args.clone(),
            env: env.clone(),
            verified: plan.verified,
        };
        self.installs.record(&plan.id, installed.clone())?;
        Ok(installed)
    }

    /// Downloads `url` into `folder`, checks it against `sha256` and unpacks it; the program's path in the folder.
    fn archive(
        &self,
        folder: &Path,
        url: &str,
        sha256: Option<&str>,
        cmd: &str,
        progress: &dyn Fn(Progress),
    ) -> Result<PathBuf, InstallError> {
        let name = url.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("download");
        let file = folder.join(format!(".download-{name}"));
        let actual = self.download(url, &file, progress)?;
        if let Some(expected) = sha256
            && !expected.eq_ignore_ascii_case(&actual)
        {
            let _ = fs::remove_file(&file);
            return Err(InstallError::Checksum {
                expected: expected.to_ascii_lowercase(),
                actual,
            });
        }
        progress(Progress::Unpacking);
        let file_text = file.to_string_lossy().into_owned();
        let folder_text = folder.to_string_lossy().into_owned();
        let (program, args): (&str, Vec<&str>) = if name.ends_with(".zip") {
            ("/usr/bin/ditto", vec!["-x", "-k", &file_text, &folder_text])
        } else {
            ("/usr/bin/tar", vec!["-xf", &file_text, "-C", &folder_text])
        };
        let output = self.runner.run(Path::new(program), &args, folder, &[], SHORT_TIMEOUT)?;
        let _ = fs::remove_file(&file);
        if !output.succeeded() {
            return Err(InstallError::Tool(output.stderr));
        }
        let path = inside(folder, cmd).ok_or_else(|| InstallError::Unusable(format!("{cmd} leaves its folder")))?;
        if !path.is_file() {
            return Err(InstallError::Unusable(format!("{cmd} is not in the archive")));
        }
        // tar and ditto unpack symbolic links as they are, pointing anywhere.
        if !path.canonicalize()?.starts_with(folder.canonicalize()?) {
            return Err(InstallError::Unusable(format!("{cmd} leaves its folder")));
        }
        make_executable(&path)?;
        Ok(path.strip_prefix(folder).map(Path::to_path_buf).unwrap_or(path))
    }

    /// Streams `url` into `file`; the SHA-256 of what arrived.
    fn download(&self, url: &str, file: &Path, progress: &dyn Fn(Progress)) -> Result<String, InstallError> {
        let progress = &mut |done, _| progress(Progress::Downloading(done));
        term_net::download(&*self.net, url, file, &Cancel::new(), Some(MAX_ARCHIVE_BYTES), progress).map_err(|error| {
            match error {
                DownloadError::Network(error) => InstallError::Net(error),
                DownloadError::Io(error) => InstallError::Io(error),
                DownloadError::TooLarge(_) => InstallError::Unusable(format!("{url} is larger than expected")),
                DownloadError::Cancelled => InstallError::Unusable(format!("{url} was cancelled")),
            }
        })
    }

    /// Installs `package` at `version` with npm into `folder`; the program's path in the folder.
    fn npm(
        &self,
        folder: &Path,
        package: &str,
        version: &str,
        integrity: Option<&str>,
        bin: Option<&str>,
    ) -> Result<PathBuf, InstallError> {
        let npm = self.runner.which("npm").ok_or(InstallError::Runtime("Node.js", None))?;
        let spec = format!("{package}@{version}");
        let prefix = folder.to_string_lossy().into_owned();
        // Install scripts run only in the rebuild below, once the package is known to be the one the plan names.
        let args = [
            "install",
            "--prefix",
            &prefix,
            "--ignore-scripts",
            "--no-audit",
            "--no-fund",
            "--omit=dev",
            &spec,
        ];
        let output = self.runner.run(&npm, &args, folder, &[], PACKAGE_TIMEOUT)?;
        if !output.succeeded() {
            return Err(InstallError::Tool(output.stderr));
        }
        let lock: Value = fs::read(folder.join("package-lock.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let entry = &lock["packages"][format!("node_modules/{package}")];
        if entry["version"].as_str() != Some(version) {
            return Err(InstallError::Unusable(format!("npm did not install {spec}")));
        }
        if let Some(expected) = integrity {
            let actual = entry["integrity"].as_str().unwrap_or_default();
            if actual != expected {
                return Err(InstallError::Checksum {
                    expected: expected.into(),
                    actual: actual.into(),
                });
            }
        }
        let rebuild = ["rebuild", "--prefix", &prefix, "--no-audit", "--no-fund"];
        let output = self.runner.run(&npm, &rebuild, folder, &[], PACKAGE_TIMEOUT)?;
        if !output.succeeded() {
            return Err(InstallError::Tool(output.stderr));
        }
        let bin = match bin {
            Some(bin) => bin.to_string(),
            None => package_bin(&folder.join("node_modules").join(package), package)?,
        };
        let program = PathBuf::from("node_modules/.bin").join(&bin);
        if !folder.join(&program).exists() {
            return Err(InstallError::Unusable(format!("{package} has no program {bin}")));
        }
        Ok(program)
    }

    /// Installs `package` at `version` with `uv tool install` into `folder`; the program's path in the folder.
    fn uv(&self, folder: &Path, package: &str, version: &str) -> Result<PathBuf, InstallError> {
        let uv = self.runner.which("uv").ok_or(InstallError::Runtime("uv", None))?;
        let spec = format!("{package}=={version}");
        let extra = [
            (
                "UV_TOOL_DIR".to_string(),
                folder.join("tools").to_string_lossy().into_owned(),
            ),
            (
                "UV_TOOL_BIN_DIR".to_string(),
                folder.join("bin").to_string_lossy().into_owned(),
            ),
        ];
        let output = self.runner.run(
            &uv,
            &["tool", "install", "--force", &spec],
            folder,
            &extra,
            PACKAGE_TIMEOUT,
        )?;
        if !output.succeeded() {
            return Err(InstallError::Tool(output.stderr));
        }
        let program = PathBuf::from("bin").join(package);
        if !folder.join(&program).exists() {
            return Err(InstallError::Unusable(format!(
                "{package} installed no program of its name"
            )));
        }
        Ok(program)
    }
}

/// The program a Node package installs: its only one, or the one named like the package.
fn package_bin(folder: &Path, package: &str) -> Result<String, InstallError> {
    let manifest: Value = fs::read(folder.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let short = package.rsplit('/').next().unwrap_or(package);
    match &manifest["bin"] {
        Value::String(_) => Ok(short.to_string()),
        Value::Object(bins) if bins.len() == 1 => Ok(bins.keys().next().cloned().unwrap_or_default()),
        Value::Object(bins) if bins.contains_key(short) => Ok(short.to_string()),
        _ => Err(InstallError::Unusable(format!("{package} names no single program"))),
    }
}

fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_path_may_not_leave_its_folder() {
        let folder = Path::new("/agents/x/1.0.0");
        assert_eq!(inside(folder, "./bin/agent"), Some(folder.join("bin/agent")));
        assert_eq!(inside(folder, "../../evil"), None);
        assert_eq!(inside(folder, "/usr/bin/evil"), None);
    }

    #[test]
    fn only_a_plain_name_names_an_agent_s_folder() {
        assert!(plain_name("codex-acp") && plain_name("1.0.0-beta.2"));
        for name in ["", ".", "..", "../../x", "1.0.0-a/../..", "/abs", "a\0b"] {
            assert!(!plain_name(name), "{name:?}");
        }
    }

    #[test]
    fn a_package_names_its_program_once_or_by_its_name() {
        let dir = term_base::testing::scratch("acp-bin");
        fs::write(dir.join("package.json"), r#"{"bin":{"tool":"cli.js"}}"#).unwrap();
        assert_eq!(package_bin(&dir, "@scope/other").unwrap(), "tool");
        fs::write(dir.join("package.json"), r#"{"bin":{"a":"a.js","agent":"b.js"}}"#).unwrap();
        assert_eq!(package_bin(&dir, "@scope/agent").unwrap(), "agent");
        fs::write(dir.join("package.json"), r#"{"bin":"cli.js"}"#).unwrap();
        assert_eq!(package_bin(&dir, "@scope/agent").unwrap(), "agent");
        let _ = fs::remove_dir_all(&dir);
    }
}
