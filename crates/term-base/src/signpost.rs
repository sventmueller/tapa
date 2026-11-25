//! Intervals that Instruments shows on its Points of Interest track while it records the app, such as a keystroke or
//! an agent's update, so a slow one in a real session shows with its name and length. macOS keeps them in memory
//! even while nothing records, at about a microsecond each.

use std::ffi::{CStr, CString, c_char, c_void};
use std::sync::OnceLock;

/// `os_signpost_type_t`'s values for the start and the end of an interval.
const BEGIN: u8 = 1;
const END: u8 = 2;

// SAFETY: The declarations match `<os/log.h>` and `<os/signpost.h>` of the macOS SDK; libSystem provides them.
unsafe extern "C" {
    fn os_log_create(subsystem: *const c_char, category: *const c_char) -> *mut c_void;
    fn os_signpost_enabled(log: *mut c_void) -> bool;
    fn os_signpost_id_generate(log: *mut c_void) -> u64;
    /// What the `os_signpost_interval_begin` and `_end` macros call.
    fn _os_signpost_emit_with_name_impl(
        dso: *const c_void,
        log: *mut c_void,
        kind: u8,
        id: u64,
        name: *const c_char,
        format: *const c_char,
        buffer: *mut u8,
        size: u32,
    );
    /// The image the names and formats are in, which the linker defines.
    static __dso_handle: u8;
}

/// The app's log for Points of Interest; its address, kept for the life of the process.
fn log() -> *mut c_void {
    static LOG: OnceLock<usize> = OnceLock::new();
    let address = *LOG.get_or_init(|| {
        let subsystem = CString::new(crate::identity().bundle_id.as_str()).unwrap_or_default();
        // SAFETY: Both strings are valid and end in a zero; the log copies them and lives as long as the process.
        unsafe { os_log_create(subsystem.as_ptr(), c"PointsOfInterest".as_ptr()) as usize }
    });
    address as *mut c_void
}

/// An interval's name, made by [`name!`] from a string literal.
#[derive(Debug, Clone, Copy)]
pub struct Name(&'static CStr);

impl Name {
    /// The name `name`.
    ///
    /// # Safety
    ///
    /// `name` must be static data of the image `__dso_handle` stands for, as the bytes of a literal in code linked
    /// into the app are; os_signpost reads the name relative to that image.
    #[doc(hidden)]
    pub const unsafe fn from_literal(name: &'static CStr) -> Self {
        Self(name)
    }
}

/// The [`Name`] of an interval, from a string literal: `signpost::name!("keystroke")`.
#[doc(hidden)]
#[macro_export]
macro_rules! __signpost_name {
    ($name:literal) => {
        const {
            match ::core::ffi::CStr::from_bytes_with_nul(::core::concat!($name, "\0").as_bytes()) {
                // SAFETY: The bytes of a literal are static data of the image the calling crate is linked into.
                Ok(name) => unsafe { $crate::signpost::Name::from_literal(name) },
                Err(_) => panic!("a signpost's name holds no zero byte"),
            }
        }
    };
}

pub use crate::__signpost_name as name;

/// An interval on the Points of Interest track, from `begin` until it is dropped.
#[must_use = "the interval ends when it is dropped"]
pub struct Interval {
    id: u64,
    name: Name,
}

/// Starts the interval `name`; `None` while signposts are off for the app's log.
pub fn begin(name: Name) -> Option<Interval> {
    let log = log();
    // SAFETY: `log` came from `os_log_create` and is never released.
    if log.is_null() || !unsafe { os_signpost_enabled(log) } {
        return None;
    }
    // SAFETY: As above.
    let id = unsafe { os_signpost_id_generate(log) };
    emit(BEGIN, id, name);
    Some(Interval { id, name })
}

impl Drop for Interval {
    fn drop(&mut self) {
        emit(END, self.id, self.name);
    }
}

fn emit(kind: u8, id: u64, name: Name) {
    // An encoded argument list without arguments: no flags, a count of zero.
    let mut arguments = [0u8, 0u8];
    // SAFETY: `name` and the empty format are static strings of this image, as the macros pass them; the argument
    // buffer holds the two bytes it says.
    unsafe {
        _os_signpost_emit_with_name_impl(
            &raw const __dso_handle as *const c_void,
            log(),
            kind,
            id,
            name.0.as_ptr(),
            c"".as_ptr(),
            arguments.as_mut_ptr(),
            2,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interval_starts_and_ends_quickly() {
        let started = std::time::Instant::now();
        for _ in 0..1000 {
            drop(begin(name!("test")));
        }
        assert!(
            started.elapsed() < std::time::Duration::from_millis(50),
            "{:?}",
            started.elapsed()
        );
    }

    /// Run under `xcrun xctrace record --template Logging --launch`, it leaves intervals named `probe` in the trace.
    #[test]
    #[ignore = "needs Instruments recording the process"]
    fn intervals_reach_a_recording() {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut started = 0;
        while std::time::Instant::now() < until {
            if let Some(interval) = begin(name!("probe")) {
                started += 1;
                std::thread::sleep(std::time::Duration::from_millis(5));
                drop(interval);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(started > 0, "no recording was on");
    }
}
