//! Tracks the descendants of a supervised agent across process-group and session boundaries.

use std::collections::HashMap;
use std::mem::{MaybeUninit, size_of};

use term_base::system;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Birth(u64, u64);

fn process(pid: u32) -> Option<(Birth, libc::proc_bsdinfo)> {
    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = i32::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
    let pid = i32::try_from(pid).ok()?;
    // SAFETY: The kernel writes at most `size` bytes into the buffer. Only a full result is read.
    let read = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, info.as_mut_ptr().cast(), size) };
    if read != size {
        return None;
    }
    // SAFETY: The full BSD info was written by proc_pidinfo.
    let info = unsafe { info.assume_init() };
    (info.pbi_status != libc::SZOMB).then_some((Birth(info.pbi_start_tvsec, info.pbi_start_tvusec), info))
}

pub(super) struct AgentTree {
    root: u32,
    root_birth: Birth,
    descendants: HashMap<u32, Birth>,
}

impl AgentTree {
    pub(super) fn new(root: u32) -> Option<Self> {
        Some(Self {
            root,
            root_birth: process(root)?.0,
            descendants: HashMap::new(),
        })
    }

    /// Remembers live descendants even if their parent later exits and they are reparented.
    pub(super) fn refresh(&mut self) {
        self.descendants
            .retain(|pid, birth| process(*pid).is_some_and(|(seen, _)| seen == *birth));
        let mut parents: Vec<_> = self.descendants.iter().map(|(&pid, &birth)| (pid, birth)).collect();
        parents.push((self.root, self.root_birth));
        while let Some((pid, birth)) = parents.pop() {
            if !process(pid).is_some_and(|(seen, _)| seen == birth) {
                continue;
            }
            for child in system::children(pid) {
                if let Some((born, info)) = process(child)
                    && info.pbi_ppid == pid
                    && self.descendants.insert(child, born) != Some(born)
                {
                    parents.push((child, born));
                }
            }
        }
    }

    pub(super) fn pids(&self) -> impl Iterator<Item = i32> + '_ {
        self.descendants.keys().filter_map(|&pid| i32::try_from(pid).ok())
    }

    pub(super) fn empty(&self) -> bool {
        self.descendants.is_empty() && !process(self.root).is_some_and(|(birth, _)| birth == self.root_birth)
    }

    /// A command or a separately grouped worker may still be active while the agent reports Idle.
    pub(super) fn has_work(&mut self) -> bool {
        self.refresh();
        self.descendants.iter().any(|(&pid, &birth)| {
            process(pid).is_some_and(|(seen, info)| {
                seen == birth
                    && (info.pbi_pgid != self.root
                        || i32::try_from(pid)
                            .ok()
                            .and_then(system::name)
                            .is_some_and(|name| crate::priority::runs_commands(&name)))
            })
        })
    }

    pub(super) fn signal(&mut self, signal: i32, groups: &[i32]) {
        self.refresh();
        for (&pid, &birth) in &self.descendants {
            if let Ok(pid) = i32::try_from(pid)
                && process(pid.unsigned_abs()).is_some_and(|(seen, info)| {
                    seen == birth && !i32::try_from(info.pbi_pgid).is_ok_and(|group| groups.contains(&group))
                })
            {
                // SAFETY: A positive, identity-checked PID addresses just this descendant.
                unsafe { libc::kill(pid, signal) };
            }
        }
    }
}
