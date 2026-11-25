//! What the system says about processes the app did not necessarily start: whether one or a group is still there,
//! the members of a group, the children of a process and its short name.

/// `PROC_PGRP_ONLY` from `<sys/proc_info.h>`, which the libc crate does not define.
const PROC_PGRP_ONLY: u32 = 2;

/// Whether a process numbered `pid` exists, also one of another user that the app may not signal.
pub fn exists(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: Signal 0 only asks whether the process exists; nothing is delivered.
    let status = unsafe { libc::kill(pid, 0) };
    answered(status, last_error())
}

/// Whether any process is left in the group numbered `group`; groups 0 and 1 are never the app's.
pub fn group_exists(group: i32) -> bool {
    if group <= 1 {
        return false;
    }
    // SAFETY: Signal 0 to a group only asks whether it has members; nothing is delivered, and a number above 1 can
    // never address every process.
    let status = unsafe { libc::kill(-group, 0) };
    answered(status, last_error())
}

fn last_error() -> Option<i32> {
    std::io::Error::last_os_error().raw_os_error()
}

/// What `kill(…, 0)` returning `status` with `errno` says about the target: EPERM means it is there and only refuses
/// the signal.
fn answered(status: libc::c_int, errno: Option<i32>) -> bool {
    status == 0 || errno == Some(libc::EPERM)
}

/// The processes in process group `group`.
pub fn group_members(group: i32) -> Vec<i32> {
    let Ok(typeinfo) = u32::try_from(group) else {
        return Vec::new();
    };
    listed(|pids| {
        let capacity = libc::c_int::try_from(std::mem::size_of_val(pids)).ok()?;
        // SAFETY: The buffer holds `capacity` bytes and outlives the call, which writes no more than that.
        let filled = unsafe { libc::proc_listpids(PROC_PGRP_ONLY, typeinfo, pids.as_mut_ptr().cast(), capacity) };
        usize::try_from(filled)
            .ok()
            .map(|bytes| bytes / size_of::<libc::pid_t>())
    })
}

/// The physical footprint reported by macOS for a live process, in bytes.
pub fn physical_footprint(pid: u32) -> Option<u64> {
    let pid = i32::try_from(pid).ok()?;
    let mut usage = std::mem::MaybeUninit::<libc::rusage_info_v0>::uninit();
    // SAFETY: The requested flavor writes a rusage_info_v0 into the supplied buffer.
    let status = unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V0, usage.as_mut_ptr().cast()) };
    // SAFETY: On success the buffer contains a complete rusage_info_v0.
    (status == 0).then(|| unsafe { usage.assume_init().ri_phys_footprint })
}

/// The processes `parent` started that still run.
pub fn children(parent: u32) -> Vec<u32> {
    let Ok(parent) = libc::pid_t::try_from(parent) else {
        return Vec::new();
    };
    listed(|pids| {
        let capacity = libc::c_int::try_from(std::mem::size_of_val(pids)).ok()?;
        // SAFETY: The buffer holds `capacity` bytes and outlives the call, which writes no more than that.
        let count = unsafe { libc::proc_listchildpids(parent, pids.as_mut_ptr().cast(), capacity) };
        usize::try_from(count).ok()
    })
    .into_iter()
    .filter_map(|pid| u32::try_from(pid).ok())
    .collect()
}

/// The numbers `fill` writes into a buffer and counts, in a buffer grown until the list fits; the system fills a
/// buffer that is too small to the brim without saying more were there.
fn listed(mut fill: impl FnMut(&mut [libc::pid_t]) -> Option<usize>) -> Vec<libc::pid_t> {
    let mut pids: Vec<libc::pid_t> = vec![0; 256];
    loop {
        let Some(count) = fill(&mut pids) else {
            return Vec::new();
        };
        if count < pids.len() {
            pids.truncate(count);
            pids.retain(|&pid| pid > 0);
            return pids;
        }
        let Some(grown) = pids.len().checked_mul(4) else {
            return Vec::new();
        };
        pids.resize(grown, 0);
    }
}

/// The short name the system keeps for `pid`, as `ps` shows it without arguments.
pub fn name(pid: i32) -> Option<String> {
    let mut buffer = [0u8; 256];
    let capacity = u32::try_from(buffer.len()).ok()?;
    // SAFETY: The buffer holds `capacity` bytes and outlives the call, which writes no more than that.
    let length = unsafe { libc::proc_name(pid, buffer.as_mut_ptr().cast(), capacity) };
    let length = usize::try_from(length).ok().filter(|&n| n > 0)?;
    Some(String::from_utf8_lossy(&buffer[..length.min(buffer.len())]).into_owned())
}

/// Sends `signal` to every process in group `group`; a group that is gone already is no error.
pub fn signal_group(group: i32, signal: i32) {
    if group <= 0 {
        return;
    }
    // SAFETY: killpg only sends a signal; a group that no longer exists makes it fail with ESRCH.
    unsafe { libc::killpg(group, signal) };
}

/// Whether nothing is left in process group `group` within `timeout`.
pub fn group_ends_within(group: i32, timeout: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if !group_exists(group) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    use super::*;

    #[test]
    fn a_process_that_refuses_signals_still_exists() {
        assert!(answered(-1, Some(libc::EPERM)));
        assert!(!answered(-1, Some(libc::ESRCH)));
        assert!(answered(0, None));
        assert!(exists(1), "launchd belongs to root, so asking it gives EPERM");
    }

    #[test]
    fn a_reaped_process_and_its_empty_group_are_gone() {
        let mut child = Command::new("/bin/sleep").arg("0").process_group(0).spawn().unwrap();
        let pid = child.id();
        child.wait().unwrap();
        assert!(!exists(pid));
        assert!(!group_exists(i32::try_from(pid).unwrap()));
        assert!(!group_exists(1) && !group_exists(0) && !group_exists(-5));
    }

    #[test]
    fn a_list_longer_than_the_first_buffer_comes_whole() {
        let all: Vec<libc::pid_t> = (1..=1000).collect();
        let mut calls = 0;
        let whole = listed(|pids| {
            calls += 1;
            let n = pids.len().min(all.len());
            pids[..n].copy_from_slice(&all[..n]);
            Some(n)
        });
        assert_eq!(whole, all);
        assert_eq!(
            calls, 2,
            "a full first buffer of 256, then one of 1024 with room to spare"
        );
        assert!(listed(|_| None).is_empty());
    }

    #[test]
    fn more_than_256_children_are_all_listed() {
        let mut parent = Command::new("/bin/sh")
            .args([
                "-c",
                "i=0; while [ $i -lt 260 ]; do /bin/sleep 30 & i=$((i+1)); done; echo ready; wait",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut line = String::new();
        BufReader::new(parent.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let pid = parent.id();
        let children = children(pid);
        let group = group_members(i32::try_from(pid).unwrap());
        let sleeping = children
            .iter()
            .any(|&child| i32::try_from(child).ok().and_then(name).as_deref() == Some("sleep"));
        // SAFETY: Ends the test's own process group, which it started just now.
        unsafe { libc::killpg(libc::pid_t::try_from(pid).unwrap(), libc::SIGKILL) };
        let _ = parent.wait();
        assert_eq!(children.len(), 260);
        assert_eq!(group.len(), 261, "the shell and its sleeps");
        assert!(sleeping, "the children go by their program's name");
    }
}
