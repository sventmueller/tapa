//! Installing an agent on the user's word, as Dango does (E21): where the app keeps its agents, the ACP registry as
//! the user last fetched it, what the question before an installation says, the progress meanwhile and what a
//! refused or failed one says. Nothing here reaches the network unless the user asked for it (F179).

use std::path::Path;
use std::sync::Arc;
use std::time::SystemTime;

use dispatch2::MainThreadBound;
use objc2::MainThreadMarker;
use splat_i18n::{t, tf};
use term_acp::install::{InstallError, Progress, Refusal, Source as InstallSource};
use term_acp::launch::Origin;
use term_acp::register::{self, Register};
use term_acp::{Installed, Installer, Installs, Plan};
use term_base::explain::Explain;
use term_net::Net;
use term_process::Supervisor;
use term_ui::kit::alert::{Alert, Role, first};

use super::acp::{AcpAgent, launch_of};
use super::source::Problem;
use super::tab::AgentTab;
use crate::common::util::main_soon;

/// How long an installation waits for the login shell's environment.
#[cfg(not(feature = "selftest"))]
const ENVIRONMENT: std::time::Duration = std::time::Duration::from_secs(15);

/// The agents the app installed, in its data folder or, in the self-test, the folder it chose.
pub fn installs() -> Installs {
    #[cfg(feature = "selftest")]
    if let Some(root) = super::acp::test_installs() {
        return Installs::new(root);
    }
    Installs::new(Installs::default_root())
}

/// The network installations and the registry go through; in the self-test its stand-in, which never leaves the Mac.
fn net() -> Arc<dyn Net> {
    #[cfg(feature = "selftest")]
    {
        super::acp::test_net()
    }
    #[cfg(not(feature = "selftest"))]
    {
        Arc::new(term_net::Http::new("agent register"))
    }
}

/// The environment an installation's helpers run with: the login shell's, whose PATH finds npm, node and uv; in the
/// self-test only the system's folders, where none of them is, so that no package manager reaches the network.
fn helper_env() -> Vec<(String, String)> {
    #[cfg(feature = "selftest")]
    {
        vec![("PATH".to_string(), "/usr/bin:/bin".to_string())]
    }
    #[cfg(not(feature = "selftest"))]
    {
        term_ui::workspace_host::environment(ENVIRONMENT)
    }
}

/// The registry as last fetched on the user's request, and when.
pub fn cached_register() -> Option<(Register, SystemTime)> {
    #[cfg(feature = "selftest")]
    if let Some(register) = super::acp::test_register() {
        return Some((register, SystemTime::now()));
    }
    Register::cached()
}

/// Fetches the registry and keeps it, on the user's request; runs on a thread of its own, then `done` on the main
/// thread with the number of entries or what went wrong.
pub fn fetch_register(mtm: MainThreadMarker, done: impl FnOnce(Result<usize, String>) + 'static) {
    let done = MainThreadBound::new(done, mtm);
    let work = move || {
        let result = Register::fetch(&*net())
            .map(|r| r.entries.len())
            .map_err(|e| e.to_string());
        main_soon(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                (done.into_inner(mtm))(result);
            }
        });
    };
    if let Err(why) = crate::common::util::spawn("acp-register", work) {
        crate::common::diag::report("fetching the agent registry", &why);
    }
}

/// Whether an agent can run now, or what it takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// It starts from there.
    Ready(Origin),
    /// It can be installed as the plan says.
    Installable(Plan),
    /// It cannot be had, for this reason.
    Unavailable(String),
}

/// Whether `agent` runs with the login shell's environment `env` in `cwd`, or how it could.
pub fn availability(agent: &AcpAgent, env: &[(String, String)], cwd: &Path) -> Availability {
    if let Some(launch) = launch_of(agent, env, cwd, true) {
        return Availability::Ready(launch.origin);
    }
    if let Some(preset) = register::preset(agent.id()) {
        return Availability::Installable(Plan::of_preset(preset));
    }
    let Some((register, _)) = cached_register() else {
        return Availability::Unavailable(t("Nicht im geladenen Register").to_string());
    };
    match register.entry(agent.id()).map(Plan::of_entry) {
        Some(Ok(plan)) => Availability::Installable(plan),
        Some(Err(refusal)) => Availability::Unavailable(refusal_text(&refusal)),
        None => Availability::Unavailable(t("Nicht im geladenen Register").to_string()),
    }
}

/// Why the register's entry is not installed.
pub fn refusal_text(refusal: &Refusal) -> String {
    match refusal {
        Refusal::Unversioned(package) => tf(
            "{} nennt keine feste Version und wird deshalb nicht installiert",
            &[package],
        ),
        Refusal::Nothing { unknown } if unknown.is_empty() => t("Für diesen Mac gibt es keine Ausgabe").to_string(),
        Refusal::Nothing { unknown } => tf(
            "Nur als {} erhältlich, was hier nicht unterstützt wird",
            &[&unknown.join(", ")],
        ),
    }
}

/// What installing `plan` would download, for the question before it.
fn plan_text(plan: &Plan) -> String {
    let source = match &plan.source {
        InstallSource::Archive { url, .. } => tf("Archiv von {}", &[url]),
        InstallSource::Npm { package, version, .. } => tf("npm-Paket {}", &[&format!("{package}@{version}")]),
        InstallSource::Uv { package, version, .. } => {
            tf("Python-Paket {} über uv", &[&format!("{package}=={version}")])
        }
    };
    let mut lines = vec![
        tf("Version {}", &[&plan.version]),
        source,
        tf(
            "Ziel: {}",
            &[&Installs::default_root().join(&plan.id).display().to_string()],
        ),
    ];
    if !plan.verified {
        lines.push(t("Diese Version wurde mit dieser App nicht erprobt.").to_string());
    }
    if !plan.checksum {
        lines.push(t("Das Register nennt keine Prüfsumme; der Download wird nicht geprüft.").to_string());
    }
    lines.join("\n")
}

/// What a failed installation says to the user.
pub(crate) fn install_error_text(error: &InstallError) -> String {
    error.explain()
}

fn bytes(count: u64) -> String {
    objc2_foundation::NSByteCountFormatter::new()
        .stringFromByteCount(i64::try_from(count).unwrap_or(i64::MAX))
        .to_string()
}

/// What an installation of `name` has got to, in words.
pub fn progress_text(name: &str, progress: Progress) -> String {
    match progress {
        Progress::Checking => tf("{} wird vorbereitet …", &[name]),
        Progress::Downloading(count) => tf("{} wird geladen: {}", &[name, &bytes(count)]),
        Progress::Unpacking => tf("{} wird entpackt …", &[name]),
        Progress::Installing => tf("{} wird installiert …", &[name]),
    }
}

/// Asks before installing `plan` of `agent`; whether the user agreed.
pub fn confirm_install(mtm: MainThreadMarker, agent: &str, plan: &Plan) -> bool {
    first(
        Alert::new(mtm, &tf("{} installieren?", &[agent]))
            .informative(&plan_text(plan))
            .button(t("Installieren"), Role::Plain)
            .button(t("Abbrechen"), Role::Plain)
            .run_modal(),
    )
}

/// What hears an installation's progress, on the main thread.
type Heard = Box<dyn Fn(String)>;

/// Installs `plan` of `name` on a thread of its own, the user having agreed; `progress` hears each stage in words and
/// `done` the result, both on the main thread.
pub fn install_plan(
    mtm: MainThreadMarker,
    plan: Plan,
    name: &str,
    progress: impl Fn(String) + 'static,
    done: impl FnOnce(Result<Installed, String>) + 'static,
) {
    let done = MainThreadBound::new(done, mtm);
    let progress: Arc<MainThreadBound<Heard>> = Arc::new(MainThreadBound::new(Box::new(progress), mtm));
    let name = name.to_string();
    let work = move || {
        let installer = Installer::new(Supervisor::global(), helper_env(), installs(), net());
        let step = move |step: Progress| {
            let text = progress_text(&name, step);
            let heard = Arc::clone(&progress);
            main_soon(move || {
                if let Some(mtm) = MainThreadMarker::new() {
                    (heard.get(mtm))(text);
                }
            });
        };
        let result = installer
            .install(&plan, true, &step)
            .map_err(|e| install_error_text(&e));
        main_soon(move || {
            if let Some(mtm) = MainThreadMarker::new() {
                (done.into_inner(mtm))(result);
            }
        });
    };
    if let Err(why) = crate::common::util::spawn("acp-install", work) {
        crate::common::diag::report("installing an agent", &why);
    }
}

impl AgentTab {
    /// Installs the agent after the user agreed, showing the progress, then starts it.
    pub(crate) fn install_agent(&self) {
        let (Some(agent), Some(Problem::Missing(Some(plan)))) =
            (self.agent.acp_agent(), self.state.borrow().problem().cloned())
        else {
            return;
        };
        if !confirm_install(self.mtm, &agent.name, &plan) {
            return;
        }
        let key = self.key;
        self.set_problem(Some(Problem::Installing(progress_text(
            &agent.name,
            Progress::Checking,
        ))));
        let progress = move |text: String| {
            if let Some(tab) = super::get(key)
                && matches!(tab.state.borrow().problem(), Some(Problem::Installing(_)))
            {
                tab.set_problem(Some(Problem::Installing(text)));
            }
        };
        install_plan(self.mtm, *plan, &agent.name, progress, move |result| {
            let Some(tab) = super::get(key) else { return };
            match result {
                Ok(_) => tab.start(),
                Err(why) => tab.set_problem(Some(Problem::Start(why))),
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(source: InstallSource, verified: bool, checksum: bool) -> Plan {
        Plan {
            id: "test-agent".into(),
            name: "Test".into(),
            version: "1.2.3".into(),
            source,
            verified,
            checksum,
            runtime: term_acp::install::Runtime::None,
        }
    }

    #[test]
    fn the_question_before_installing_says_what_is_downloaded_and_what_is_not_checked() {
        let npm = InstallSource::Npm {
            package: "@acme/agent".into(),
            version: "1.2.3".into(),
            integrity: None,
            bin: None,
            args: Vec::new(),
            env: Vec::new(),
        };
        let text = plan_text(&plan(npm, false, false));
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Version 1.2.3");
        assert_eq!(lines[1], "npm-Paket @acme/agent@1.2.3");
        assert!(
            lines[2].starts_with("Ziel: ") && lines[2].ends_with("test-agent"),
            "{}",
            lines[2]
        );
        assert_eq!(lines[3], "Diese Version wurde mit dieser App nicht erprobt.");
        assert_eq!(
            lines[4],
            "Das Register nennt keine Prüfsumme; der Download wird nicht geprüft."
        );
        let uv = InstallSource::Uv {
            package: "acme".into(),
            version: "0.9".into(),
            args: Vec::new(),
            env: Vec::new(),
        };
        let text = plan_text(&plan(uv, true, true));
        assert_eq!(
            text.lines().count(),
            3,
            "a verified, checked plan has no warnings: {text}"
        );
        assert!(text.contains("Python-Paket acme==0.9 über uv"));
    }

    #[test]
    fn refusals_failures_and_progress_of_an_installation_are_worded() {
        assert_eq!(
            refusal_text(&Refusal::Unversioned("acme".into())),
            "acme nennt keine feste Version und wird deshalb nicht installiert"
        );
        assert_eq!(
            refusal_text(&Refusal::Nothing { unknown: Vec::new() }),
            "Für diesen Mac gibt es keine Ausgabe"
        );
        assert_eq!(
            refusal_text(&Refusal::Nothing {
                unknown: vec!["docker".into(), "brew".into()]
            }),
            "Nur als docker, brew erhältlich, was hier nicht unterstützt wird"
        );
        assert_eq!(
            install_error_text(&InstallError::Runtime("Node", Some(20))),
            "Node 20 oder neuer wird gebraucht"
        );
        assert_eq!(
            install_error_text(&InstallError::Checksum {
                expected: "a".into(),
                actual: "b".into()
            }),
            "Der Download stimmt nicht mit seiner Prüfsumme überein und wurde verworfen"
        );
        assert_eq!(progress_text("Codex", Progress::Checking), "Codex wird vorbereitet …");
        assert_eq!(progress_text("Codex", Progress::Installing), "Codex wird installiert …");
        assert!(progress_text("Codex", Progress::Downloading(2_000_000)).starts_with("Codex wird geladen: "));
    }
}
