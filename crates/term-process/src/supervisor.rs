//! The register of running processes: who started them, how to end them, and who wants to hear when that changes.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::os::unix::process::CommandExt;
use std::process::{ChildStderr, ChildStdin, ChildStdout, Command};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

use term_base::{IgnoringPoison, LockIgnoringPoison};

pub use term_base::{Ending, ProcessId};

use crate::agent_tree::AgentTree;

/// What a process is there for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A shell in a terminal tab.
    Terminal,
    /// A language server.
    LanguageServer,
    /// A coding agent or its adapter.
    Agent,
    /// A web server a project runs while it is developed.
    DevServer,
    /// git.
    Git,
    /// A debug adapter, such as lldb-dap.
    DebugAdapter,
    /// A test run of the Tests panel (E110).
    Test,
    /// Anything else, such as an installer.
    Tool,
    /// A command an agent runs through the app, such as the own agent's shell tool.
    AgentCommand,
    /// A check a work order must pass, such as its `make check`, run by the orchestrator in the gate sandbox (E187).
    Gate,
}

/// How the supervisor knows a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    /// Started by the supervisor, which waits for its end.
    Started,
    /// Started elsewhere, such as a terminal's shell, and reported by its owner.
    Registered,
}

/// A running process as the activity view shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(missing_docs, reason = "the names say it")]
pub struct ProcessInfo {
    pub id: ProcessId,
    /// The system's process number, which is also the number of its process group.
    pub pid: u32,
    /// What it is there for.
    pub kind: Kind,
    /// What the user reads, such as `gopls` or `claude`.
    pub label: String,
    /// When it started.
    pub started: SystemTime,
    /// How the supervisor knows it.
    pub(crate) origin: Origin,
}

/// A change to the register.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A process started or was registered.
    Started(ProcessInfo),
    /// A process ended or was forgotten.
    Ended {
        /// The process.
        id: ProcessId,
        /// How it ended.
        ending: Ending,
    },
}

/// The pipes of a started process; what the command did not ask for is `None`.
#[expect(missing_docs, reason = "the names say it")]
pub struct Spawned {
    pub id: ProcessId,
    /// The system's process number, which is also the number of its process group.
    pub pid: u32,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    pub stderr: Option<ChildStderr>,
}

type Listener = Arc<dyn Fn(&Event) + Send + Sync>;
type GracefulStop = Box<dyn FnOnce() + Send>;

struct Entry {
    info: ProcessInfo,
    graceful: Option<GracefulStop>,
}

/// How far a started process was announced, so that its end is never announced before its start.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Announced {
    Not,
    /// It ended before its start was announced, and its end waits for that.
    EndedBefore(Ending),
    Started,
}

/// How many endings `wait` can still report after the process is gone.
const ENDINGS_KEPT: usize = 256;

#[derive(Default)]
struct Register {
    next: u64,
    entries: BTreeMap<ProcessId, Entry>,
    agent_trees: BTreeMap<ProcessId, Arc<Mutex<AgentTree>>>,
    next_listener: u64,
    listeners: Vec<(u64, Listener)>,
    ended: VecDeque<(ProcessId, Ending)>,
    /// Processes whose caller waits for their end, which is kept beyond `ENDINGS_KEPT` until `wait` reports it.
    awaited: HashSet<ProcessId>,
    kept: HashMap<ProcessId, Ending>,
    /// Whether a thread lowers the commands agents run.
    governed: bool,
}

type Shared = (Mutex<Register>, Condvar);

/// A listener on a supervisor, which hears no more once this is dropped.
#[must_use = "dropping the subscription ends it at once"]
pub struct Subscription {
    register: Weak<Shared>,
    id: u64,
}

impl Subscription {
    /// Lets the listener hear for as long as the supervisor lives, for one that the app sets up once.
    pub fn keep(self) {
        std::mem::forget(self);
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(register) = self.register.upgrade() {
            register
                .0
                .lock_ignoring_poison()
                .listeners
                .retain(|(id, _)| *id != self.id);
        }
    }
}

impl std::fmt::Debug for Subscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Subscription")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// How often the commands agents started are looked for.
const GOVERN_EVERY: Duration = Duration::from_secs(1);

/// Keeps every process the app starts, each in a process group of its own so that ending it also ends what it started.
///
/// ```no_run
/// use std::process::{Command, Stdio};
/// use std::time::Duration;
///
/// use term_process::{Ending, Kind, Supervisor};
///
/// let supervisor = Supervisor::new();
/// let mut command = Command::new("/bin/sleep");
/// command.arg("60").stdin(Stdio::null());
/// let spawned = supervisor.spawn(command, Kind::Tool, "sleep")?;
/// assert_eq!(supervisor.running()[0].label, "sleep");
/// supervisor.stop(spawned.id, Duration::from_millis(200));
/// assert_eq!(supervisor.wait(spawned.id, Duration::from_secs(5)), Some(Ending::Signal(libc::SIGTERM)));
/// # Ok::<(), std::io::Error>(())
/// ```
#[derive(Clone, Default)]
pub struct Supervisor {
    register: Arc<Shared>,
}

impl Supervisor {
    /// A supervisor of its own, for tests; the app uses `global`.
    pub fn new() -> Supervisor {
        Supervisor::default()
    }

    /// The app's supervisor.
    pub fn global() -> &'static Supervisor {
        static GLOBAL: OnceLock<Supervisor> = OnceLock::new();
        GLOBAL.get_or_init(Supervisor::new)
    }

    /// Starts `command` in a new process group and keeps it until it ends.
    pub fn spawn(&self, command: Command, kind: Kind, label: impl Into<String>) -> io::Result<Spawned> {
        self.start(command, kind, label.into(), false)
    }

    /// Starts `command` as `spawn` does, for a caller that will `wait` for its end: that end is kept until `wait`
    /// reports it or `release` gives it up, however many other processes end first.
    pub fn spawn_awaited(&self, command: Command, kind: Kind, label: impl Into<String>) -> io::Result<Spawned> {
        self.start(command, kind, label.into(), true)
    }

    /// Stops keeping the end of `id` from `spawn_awaited`, for a caller that no longer waits for it.
    pub fn release(&self, id: ProcessId) {
        let (lock, _) = &*self.register;
        let mut register = lock.lock_ignoring_poison();
        register.awaited.remove(&id);
        register.kept.remove(&id);
    }

    fn start(&self, mut command: Command, kind: Kind, label: String, awaited: bool) -> io::Result<Spawned> {
        command.process_group(0);
        let mut child = command.spawn()?;
        let (stdin, stdout, stderr) = (child.stdin.take(), child.stdout.take(), child.stderr.take());
        let pid = child.id();
        if let Some(nice) = crate::priority::nice(kind) {
            crate::priority::lower(pid, nice);
        }
        let info = self.add(pid, kind, label, Origin::Started);
        let id = info.id;
        if awaited {
            self.register.0.lock_ignoring_poison().awaited.insert(id);
        }
        let supervisor = self.clone();
        let announced = Arc::new(Mutex::new(Announced::Not));
        let gate = announced.clone();
        let reaped = crate::reaper::reap(child, move |ending| {
            let mut gate = gate.lock_ignoring_poison();
            if *gate == Announced::Not {
                *gate = Announced::EndedBefore(ending);
                return;
            }
            drop(gate);
            supervisor.remove(id, ending);
        });
        match reaped {
            Err(error) => {
                self.abandon(id, pid);
                return Err(error);
            }
            Ok(ended) => {
                self.announce(&Event::Started(info));
                let early = std::mem::replace(&mut *announced.lock_ignoring_poison(), Announced::Started);
                let early = match early {
                    Announced::EndedBefore(ending) => Some(ending),
                    Announced::Not | Announced::Started => None,
                };
                if let Some(ending) = ended.or(early) {
                    self.remove(id, ending);
                }
            }
        }
        Ok(Spawned {
            id,
            pid,
            stdin,
            stdout,
            stderr,
        })
    }

    /// Adds a process started elsewhere, which leads its own process group; its owner calls `forget` when it ends.
    pub fn register(&self, pid: u32, kind: Kind, label: impl Into<String>) -> ProcessId {
        let info = self.add(pid, kind, label.into(), Origin::Registered);
        let id = info.id;
        self.announce(&Event::Started(info));
        id
    }

    /// Removes a registered process that has ended.
    pub fn forget(&self, id: ProcessId) {
        self.remove(id, Ending::Unknown);
    }

    /// What `stop` does first for `id`, such as closing its input or asking a language server to shut down.
    pub fn on_stop(&self, id: ProcessId, graceful: impl FnOnce() + Send + 'static) {
        let (lock, _) = &*self.register;
        if let Some(entry) = lock.lock_ignoring_poison().entries.get_mut(&id) {
            entry.graceful = Some(Box::new(graceful));
        }
    }

    /// Calls `listener` with every change from now on, on whichever thread makes it, until the subscription is
    /// dropped.
    pub fn subscribe(&self, listener: impl Fn(&Event) + Send + Sync + 'static) -> Subscription {
        let (lock, _) = &*self.register;
        let mut register = lock.lock_ignoring_poison();
        register.next_listener += 1;
        let id = register.next_listener;
        register.listeners.push((id, Arc::new(listener)));
        Subscription {
            register: Arc::downgrade(&self.register),
            id,
        }
    }

    /// How `id` ended, waiting up to `timeout` for it; `None` while it still runs.
    pub fn wait(&self, id: ProcessId, timeout: Duration) -> Option<Ending> {
        let deadline = Instant::now() + timeout;
        let (lock, changed) = &*self.register;
        let mut register = lock.lock_ignoring_poison();
        loop {
            if let Some(ending) = register.kept.remove(&id) {
                return Some(ending);
            }
            if let Some((_, ending)) = register.ended.iter().find(|(ended, _)| *ended == id) {
                return Some(*ending);
            }
            if !register.entries.contains_key(&id) {
                return Some(Ending::Unknown);
            }
            let now = Instant::now();
            if now >= deadline {
                return None;
            }
            register = changed.wait_timeout(register, deadline - now).ignoring_poison().0;
        }
    }

    /// The processes that run now, oldest first.
    pub fn running(&self) -> Vec<ProcessInfo> {
        let (lock, _) = &*self.register;
        lock.lock_ignoring_poison()
            .entries
            .values()
            .map(|e| e.info.clone())
            .collect()
    }

    /// Ends `id` in the background: its graceful step, then after `grace` SIGTERM to its group, then after another
    /// `grace` SIGKILL.
    pub fn stop(&self, id: ProcessId, grace: Duration) {
        let supervisor = self.clone();
        term_base::thread::detach_or_run("stop", move || supervisor.end(&[id], grace));
    }

    /// Whether an agent still owns a shell, a command or a separately grouped worker.
    /// A missing process observation is treated as busy.
    pub fn agent_has_work(&self, id: ProcessId) -> bool {
        let tree = self.register.0.lock_ignoring_poison().agent_trees.get(&id).cloned();
        tree.is_none_or(|tree| tree.lock_ignoring_poison().has_work())
    }

    /// Whether shutdown has processes to clean up, including descendants whose leader exited.
    pub fn has_managed_processes(&self) -> bool {
        let register = self.register.0.lock_ignoring_poison();
        !register.entries.is_empty() || !register.agent_trees.is_empty()
    }

    /// Ends every process as `stop` does and returns once all are gone or `grace` has passed three times.
    pub fn shutdown(&self, grace: Duration) {
        let ids: HashSet<_> = {
            let register = self.register.0.lock_ignoring_poison();
            register
                .entries
                .keys()
                .chain(register.agent_trees.keys())
                .copied()
                .collect()
        };
        let ids: Vec<_> = ids.into_iter().collect();
        self.end(&ids, grace);
    }

    fn end(&self, ids: &[ProcessId], grace: Duration) {
        let mut graceful = vec![];
        let mut groups = vec![];
        let trees: Vec<_> = {
            let (lock, _) = &*self.register;
            let mut register = lock.lock_ignoring_poison();
            for id in ids {
                if let Some(entry) = register.entries.get_mut(id) {
                    groups.extend(libc::pid_t::try_from(entry.info.pid).ok());
                    graceful.extend(entry.graceful.take());
                }
            }
            ids.iter()
                .filter_map(|id| register.agent_trees.get(id).cloned())
                .collect()
        };
        for tree in &trees {
            tree.lock_ignoring_poison().refresh();
        }
        let had_graceful = !graceful.is_empty();
        for step in graceful {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(step));
        }
        if had_graceful && self.wait_until_gone(ids, &groups, &trees, grace) {
            return;
        }
        signal_owned(&groups, &trees, libc::SIGTERM);
        if self.wait_until_gone(ids, &groups, &trees, grace) {
            return;
        }
        signal_owned(&groups, &trees, libc::SIGKILL);
        if !self.wait_until_gone(ids, &groups, &trees, grace) {
            // A registered process whose owner never reports its end would otherwise stay listed for good.
            for id in ids {
                if self.origin(*id) == Some(Origin::Registered) {
                    self.forget(*id);
                }
            }
        }
    }

    fn origin(&self, id: ProcessId) -> Option<Origin> {
        let (lock, _) = &*self.register;
        lock.lock_ignoring_poison().entries.get(&id).map(|e| e.info.origin)
    }

    /// Whether all of `ids` ended and nothing is left in `groups` within `timeout`.
    fn wait_until_gone(
        &self,
        ids: &[ProcessId],
        groups: &[libc::pid_t],
        trees: &[Arc<Mutex<AgentTree>>],
        timeout: Duration,
    ) -> bool {
        let (lock, _) = &*self.register;
        let watched = || {
            let registered: Vec<i32> = {
                let register = lock.lock_ignoring_poison();
                ids.iter()
                    .filter_map(|id| register.entries.get(id))
                    .filter(|e| e.info.origin == Origin::Registered)
                    .filter_map(|e| i32::try_from(e.info.pid).ok())
                    .collect()
            };
            let mut pids: Vec<i32> = groups
                .iter()
                .flat_map(|&g| term_base::system::group_members(g))
                .collect();
            for tree in trees {
                let mut tree = tree.lock_ignoring_poison();
                tree.refresh();
                pids.extend(tree.pids());
            }
            pids.extend(registered);
            pids.sort_unstable();
            pids.dedup();
            pids
        };
        let gone = || {
            self.prune_registered();
            let removed = {
                let register = lock.lock_ignoring_poison();
                ids.iter().all(|id| !register.entries.contains_key(id))
            };
            removed
                && !groups.iter().any(|&g| term_base::system::group_exists(g))
                && trees.iter().all(|tree| {
                    let mut tree = tree.lock_ignoring_poison();
                    tree.refresh();
                    tree.empty()
                })
        };
        crate::reaper::wait_until(timeout, watched, gone)
    }

    /// Drops registered processes that no longer exist.
    fn prune_registered(&self) {
        let (lock, changed) = &*self.register;
        let gone: Vec<ProcessId> = {
            let mut register = lock.lock_ignoring_poison();
            let gone: Vec<ProcessId> = register
                .entries
                .values()
                .filter(|e| e.info.origin == Origin::Registered && !term_base::system::exists(e.info.pid))
                .map(|e| e.info.id)
                .collect();
            for id in &gone {
                register.entries.remove(id);
            }
            gone
        };
        if !gone.is_empty() {
            changed.notify_all();
        }
        for id in gone {
            self.announce(&Event::Ended {
                id,
                ending: Ending::Unknown,
            });
        }
    }

    fn add(&self, pid: u32, kind: Kind, label: String, origin: Origin) -> ProcessInfo {
        let tree = if matches!(kind, Kind::Agent | Kind::AgentCommand) && origin == Origin::Started {
            AgentTree::new(pid)
        } else {
            None
        };
        let (lock, _) = &*self.register;
        let mut register = lock.lock_ignoring_poison();
        register.next += 1;
        let info = ProcessInfo {
            id: ProcessId(register.next),
            pid,
            kind,
            label,
            started: SystemTime::now(),
            origin,
        };
        register.entries.insert(
            info.id,
            Entry {
                info: info.clone(),
                graceful: None,
            },
        );
        if let Some(tree) = tree {
            register.agent_trees.insert(info.id, Arc::new(Mutex::new(tree)));
        }
        let govern = matches!(kind, Kind::Agent | Kind::AgentCommand) && !register.governed;
        register.governed |= govern;
        drop(register);
        if govern {
            self.govern();
        }
        info
    }

    /// Lowers, from now on, everything every agent starts (see `priority::lower_agent_commands`), until the
    /// supervisor goes.
    fn govern(&self) {
        let register = Arc::downgrade(&self.register);
        let started = term_base::thread::detach("agent-commands", move || {
            term_base::priority::set_qos(term_base::priority::Qos::Utility);
            let mut lowered = std::collections::HashSet::new();
            while let Some(register) = register.upgrade() {
                let agents: Vec<u32> = register
                    .0
                    .lock_ignoring_poison()
                    .entries
                    .values()
                    .filter(|e| e.info.kind == Kind::Agent)
                    .map(|e| e.info.pid)
                    .collect();
                let trees: Vec<_> = register
                    .0
                    .lock_ignoring_poison()
                    .agent_trees
                    .iter()
                    .map(|(&id, tree)| (id, tree.clone()))
                    .collect();
                let gone: Vec<_> = trees
                    .into_iter()
                    .filter_map(|(id, tree)| {
                        let mut tree = tree.lock_ignoring_poison();
                        tree.refresh();
                        tree.empty().then_some(id)
                    })
                    .collect();
                if !gone.is_empty() {
                    let mut state = register.0.lock_ignoring_poison();
                    for id in gone {
                        state.agent_trees.remove(&id);
                    }
                }
                drop(register);
                if !agents.is_empty() || !lowered.is_empty() {
                    crate::priority::lower_agent_commands(&agents, &mut lowered);
                }
                std::thread::sleep(GOVERN_EVERY);
            }
        });
        if let Err(unstarted) = started {
            term_base::diag::report("lowering agent commands", unstarted.error);
        }
    }

    /// Kills and reaps a started process nobody can wait for, and takes it off the list unannounced.
    fn abandon(&self, id: ProcessId, pid: u32) {
        if let Ok(group) = libc::pid_t::try_from(pid) {
            // SAFETY: kill and waitpid act on the child just started and not yet reaped, so its number is still its
            // own; the status pointer may be null.
            unsafe {
                libc::kill(-group, libc::SIGKILL);
                libc::waitpid(group, std::ptr::null_mut(), 0);
            }
        }
        let (lock, changed) = &*self.register;
        let mut register = lock.lock_ignoring_poison();
        register.entries.remove(&id);
        register.awaited.remove(&id);
        drop(register);
        changed.notify_all();
    }

    fn remove(&self, id: ProcessId, ending: Ending) {
        let (lock, changed) = &*self.register;
        let removed = {
            let mut register = lock.lock_ignoring_poison();
            let removed = register.entries.remove(&id).is_some();
            if removed {
                if register.ended.len() == ENDINGS_KEPT {
                    register.ended.pop_front();
                }
                register.ended.push_back((id, ending));
                if register.awaited.remove(&id) {
                    register.kept.insert(id, ending);
                }
            }
            removed
        };
        changed.notify_all();
        if removed {
            self.announce(&Event::Ended { id, ending });
        }
    }

    fn announce(&self, event: &Event) {
        let (lock, _) = &*self.register;
        let listeners: Vec<Listener> = lock
            .lock_ignoring_poison()
            .listeners
            .iter()
            .map(|(_, listener)| listener.clone())
            .collect();
        for listener in listeners {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| listener(event)));
        }
    }
}

fn signal_owned(groups: &[libc::pid_t], trees: &[Arc<Mutex<AgentTree>>], signal: i32) {
    for tree in trees {
        tree.lock_ignoring_poison().signal(signal, groups);
    }
    signal_groups(groups, signal);
}

/// Sends `signal` to each of `groups` that still has members, whether or not its leader is still there.
fn signal_groups(groups: &[libc::pid_t], signal: i32) {
    for &group in groups {
        if term_base::system::group_exists(group) {
            // SAFETY: kill only sends a signal. The system reuses no number while a group of that number has members,
            // so a group found just now is the one the process led, unless it emptied and its number was taken
            // again in the moment since, which needs the whole range of process numbers to wrap around.
            unsafe { libc::kill(-group, signal) };
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::process::Stdio;
    use std::sync::mpsc::channel;

    use super::*;
    use term_base::system::{exists, group_exists};

    const GRACE: Duration = Duration::from_secs(2);

    fn sleeper(seconds: u32) -> Command {
        let mut command = Command::new("/bin/sleep");
        command.arg(seconds.to_string());
        command
    }

    #[test]
    fn a_process_is_listed_until_it_exits_and_its_code_is_reported() {
        let supervisor = Supervisor::new();
        let (tx, rx) = channel();
        let _listening = supervisor.subscribe(move |event| tx.send(event.clone()).unwrap());
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 3"]);
        let spawned = supervisor.spawn(command, Kind::Tool, "sh").unwrap();
        let started = rx.recv_timeout(GRACE).unwrap();
        assert!(matches!(started, Event::Started(ref info) if info.id == spawned.id && info.label == "sh"));
        let ended = rx.recv_timeout(GRACE).unwrap();
        assert_eq!(
            ended,
            Event::Ended {
                id: spawned.id,
                ending: Ending::Code(3)
            }
        );
        assert!(supervisor.running().is_empty());
    }

    #[test]
    fn a_process_that_ends_at_once_is_announced_as_started_before_it_is_announced_as_ended() {
        let supervisor = Supervisor::new();
        let heard = Arc::new(Mutex::new(Vec::new()));
        let hearing = heard.clone();
        let _listening = supervisor.subscribe(move |event| {
            if matches!(event, Event::Started(_)) {
                std::thread::sleep(Duration::from_millis(200));
            }
            hearing.lock_ignoring_poison().push(event.clone());
        });
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 5"]);
        let spawned = supervisor.spawn(command, Kind::Tool, "sh").unwrap();
        let deadline = Instant::now() + GRACE;
        while heard.lock_ignoring_poison().len() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let heard = heard.lock_ignoring_poison();
        assert!(
            matches!(
                heard.as_slice(),
                [Event::Started(info), Event::Ended { id, ending: Ending::Code(5) }]
                    if info.id == spawned.id && *id == spawned.id
            ),
            "{heard:?}"
        );
        assert_eq!(supervisor.wait(spawned.id, GRACE), Some(Ending::Code(5)));
    }

    #[test]
    fn a_dropped_subscription_hears_no_more() {
        let supervisor = Supervisor::new();
        let (tx, rx) = channel();
        let kept = tx.clone();
        let listening = supervisor.subscribe(move |event| tx.send(event.clone()).unwrap());
        let _kept = supervisor.subscribe(move |event| kept.send(event.clone()).unwrap());
        drop(listening);
        let spawned = supervisor.spawn(sleeper(0), Kind::Tool, "sleep").unwrap();
        assert_eq!(supervisor.wait(spawned.id, GRACE), Some(Ending::Code(0)));
        let heard: Vec<Event> = std::iter::from_fn(|| rx.recv_timeout(Duration::from_millis(200)).ok()).collect();
        assert_eq!(heard.len(), 2, "only the kept listener heard the start and the end");
        assert_eq!(supervisor.register.0.lock_ignoring_poison().listeners.len(), 1);
    }

    #[test]
    fn an_awaited_end_outlasts_many_others_until_it_is_reported() {
        let supervisor = Supervisor::new();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exit 5"]);
        let awaited = supervisor.spawn_awaited(command, Kind::Git, "git").unwrap();
        let deadline = Instant::now() + GRACE;
        while supervisor.running().iter().any(|p| p.id == awaited.id) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let mut last = None;
        for _ in 0..ENDINGS_KEPT + 10 {
            let id = supervisor.spawn(sleeper(0), Kind::Tool, "sleep").unwrap().id;
            supervisor.wait(id, GRACE).unwrap();
            last = Some(id);
        }
        assert!(supervisor.wait(last.unwrap(), Duration::ZERO).is_some());
        assert_eq!(supervisor.wait(awaited.id, GRACE), Some(Ending::Code(5)));
        assert!(supervisor.register.0.lock_ignoring_poison().kept.is_empty());
        assert_eq!(
            supervisor.wait(awaited.id, Duration::ZERO),
            Some(Ending::Unknown),
            "once reported it is kept no longer than any other"
        );
    }

    #[test]
    fn a_released_end_is_not_kept() {
        let supervisor = Supervisor::new();
        let awaited = supervisor.spawn_awaited(sleeper(0), Kind::Git, "git").unwrap();
        supervisor.release(awaited.id);
        let deadline = Instant::now() + GRACE;
        while !supervisor.running().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let register = supervisor.register.0.lock_ignoring_poison();
        assert!(register.kept.is_empty() && register.awaited.is_empty());
    }

    #[test]
    fn waiting_reports_how_a_process_ended_also_after_it_is_gone() {
        let supervisor = Supervisor::new();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 0.2; exit 4"]);
        let spawned = supervisor.spawn(command, Kind::Tool, "sh").unwrap();
        assert_eq!(supervisor.wait(spawned.id, Duration::from_millis(10)), None);
        assert_eq!(supervisor.wait(spawned.id, GRACE), Some(Ending::Code(4)));
        assert_eq!(supervisor.wait(spawned.id, Duration::ZERO), Some(Ending::Code(4)));
    }

    #[test]
    fn stopping_ends_the_whole_group_including_what_the_process_started() {
        let supervisor = Supervisor::new();
        let mut command = Command::new("/bin/sh");
        // The grandchild prints its number, then outlives the shell unless its group is ended.
        command.args(["-c", "sleep 30 & echo $!; wait"]).stdout(Stdio::piped());
        let spawned = supervisor.spawn(command, Kind::Agent, "parent").unwrap();
        let mut line = String::new();
        BufReader::new(spawned.stdout.unwrap()).read_line(&mut line).unwrap();
        let grandchild: u32 = line.trim().parse().unwrap();
        assert!(exists(grandchild));
        supervisor.stop(spawned.id, GRACE);
        let deadline = Instant::now() + GRACE * 3;
        while exists(grandchild) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!exists(grandchild), "the grandchild survived");
    }

    #[test]
    fn the_graceful_step_comes_first_and_can_be_enough() {
        let supervisor = Supervisor::new();
        let mut command = Command::new("/bin/cat");
        command.stdin(Stdio::piped());
        let spawned = supervisor.spawn(command, Kind::LanguageServer, "cat").unwrap();
        let stdin = Mutex::new(spawned.stdin);
        let (tx, rx) = channel();
        let _listening = supervisor.subscribe(move |event| {
            if let Event::Ended { ending, .. } = event {
                tx.send(*ending).unwrap();
            }
        });
        supervisor.on_stop(spawned.id, move || drop(stdin.lock().unwrap().take()));
        supervisor.shutdown(GRACE);
        assert_eq!(
            rx.recv_timeout(GRACE).unwrap(),
            Ending::Code(0),
            "cat ended by itself at the end of its input"
        );
    }

    /// Starts a shell that prints the number of a background `sleep` in its group, then waits for its input to end.
    fn leader_with_a_member(supervisor: &Supervisor, member: &str) -> (Spawned, u32) {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", &format!("{member} & echo $!; read line")])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut spawned = supervisor.spawn(command, Kind::Agent, "leader").unwrap();
        let mut line = String::new();
        BufReader::new(spawned.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        (spawned, line.trim().parse().unwrap())
    }

    fn gone_within(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while exists(pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        !exists(pid)
    }

    #[test]
    fn a_member_outliving_a_leader_that_ended_gracefully_is_ended_too() {
        let supervisor = Supervisor::new();
        let (spawned, member) = leader_with_a_member(&supervisor, "sleep 30");
        let stdin = Mutex::new(spawned.stdin);
        supervisor.on_stop(spawned.id, move || drop(stdin.lock_ignoring_poison().take()));
        let grace = Duration::from_millis(300);
        supervisor.shutdown(grace);
        assert!(
            matches!(supervisor.wait(spawned.id, Duration::ZERO), Some(Ending::Code(_))),
            "the leader ended by itself"
        );
        assert!(gone_within(member, GRACE), "the member outlived its leader");
    }

    #[test]
    fn a_member_that_ignores_sigterm_is_killed_after_its_leader_is_gone() {
        let supervisor = Supervisor::new();
        let (spawned, member) = leader_with_a_member(&supervisor, "trap '' TERM; while true; do sleep 1; done");
        let started = Instant::now();
        supervisor.shutdown(Duration::from_millis(300));
        assert!(supervisor.running().is_empty());
        assert!(gone_within(member, GRACE), "the member survived SIGKILL");
        assert!(started.elapsed() < Duration::from_secs(3));
        drop(spawned);
    }

    #[test]
    fn an_agent_child_in_its_own_session_is_still_cleaned_up() {
        let supervisor = Supervisor::new();
        let mut command = Command::new("/usr/bin/perl");
        command
            .args([
                "-MPOSIX=setsid",
                "-e",
                r#"
                $| = 1;
                my $child = fork();
                die "fork" unless defined $child;
                if (!$child) {
                    setsid() >= 0 or die "setsid";
                    print "$$\n";
                    exec "/bin/sleep", "8";
                }
                scalar <STDIN>;
            "#,
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped());
        let mut spawned = supervisor.spawn(command, Kind::Agent, "test agent").unwrap();
        let mut line = String::new();
        BufReader::new(spawned.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let child = line.trim().parse::<u32>().unwrap();
        let work = supervisor.agent_has_work(spawned.id);
        let stdin = Mutex::new(spawned.stdin);
        supervisor.on_stop(spawned.id, move || drop(stdin.lock_ignoring_poison().take()));
        supervisor.shutdown(Duration::from_millis(200));
        let gone = gone_within(child, GRACE);
        assert!(work, "a worker in an independent session prevents parking");
        assert!(gone, "the child survived shutdown");
    }

    #[test]
    fn a_registered_leader_already_reaped_leaves_its_members_to_be_ended() {
        let supervisor = Supervisor::new();
        let mut leader = Command::new("/bin/sh")
            .args(["-c", "sleep 30 & echo $!"])
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(leader.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let member: u32 = line.trim().parse().unwrap();
        let id = supervisor.register(leader.id(), Kind::Terminal, "sh");
        leader.wait().unwrap();
        supervisor.shutdown(Duration::from_millis(200));
        assert_eq!(supervisor.wait(id, Duration::ZERO), Some(Ending::Unknown));
        assert!(gone_within(member, GRACE), "the member of a reaped leader survived");
    }

    #[test]
    fn signalling_a_group_that_is_gone_does_nothing() {
        let mut child = sleeper(0).process_group(0).spawn().unwrap();
        let group = libc::pid_t::try_from(child.id()).unwrap();
        child.wait().unwrap();
        assert!(!group_exists(group));
        signal_groups(&[group, 0, -1], libc::SIGKILL);
    }

    #[test]
    fn shutdown_ends_everything_even_what_ignores_sigterm() {
        let supervisor = Supervisor::new();
        let mut stubborn = Command::new("/bin/sh");
        stubborn.args(["-c", "trap '' TERM; while true; do sleep 1; done"]);
        supervisor.spawn(stubborn, Kind::DevServer, "stubborn").unwrap();
        supervisor.spawn(sleeper(30), Kind::Tool, "sleep").unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        supervisor.shutdown(Duration::from_millis(300));
        assert!(supervisor.running().is_empty());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn a_registered_process_leaves_the_list_when_it_is_gone() {
        let supervisor = Supervisor::new();
        let mut child = sleeper(30).process_group(0).spawn().unwrap();
        let id = supervisor.register(child.id(), Kind::Terminal, "zsh");
        assert_eq!(supervisor.running().len(), 1);
        assert_eq!(supervisor.running()[0].origin, Origin::Registered);
        let waiter = std::thread::spawn(move || child.wait());
        supervisor.stop(id, Duration::from_millis(200));
        waiter.join().unwrap().unwrap();
        let deadline = Instant::now() + GRACE;
        while !supervisor.running().is_empty() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(supervisor.running().is_empty());
    }
}
