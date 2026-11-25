//! This Mac's host name, which shells report in OSC 7.

use std::ffi::CStr;
use std::sync::OnceLock;

/// This Mac's host name, read once.
pub(crate) fn name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        let mut buffer = [0 as libc::c_char; 256];
        // SAFETY: the buffer holds its length in bytes; gethostname writes no more and the last byte stays zero.
        let status = unsafe { libc::gethostname(buffer.as_mut_ptr(), buffer.len() - 1) };
        if status != 0 {
            return String::new();
        }
        // SAFETY: the buffer was zeroed and its last byte never written, so it ends with a zero byte.
        unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_string_lossy()
            .into_owned()
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_directory_on_this_mac_is_local() {
        let at = |host: &str| crate::WorkingDirectory {
            host: host.into(),
            path: "/tmp".into(),
        };
        assert!(at("").is_local());
        assert!(at("localhost").is_local());
        assert!(at(super::name()).is_local());
        assert!(!at("build-server.example.com").is_local());
    }
}
