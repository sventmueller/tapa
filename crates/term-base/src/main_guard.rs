//! Reports work that waits on the main thread, where it holds up every keystroke: debug builds and builds with the
//! feature `testing` say so through [`crate::diag::report`], release builds of the product do nothing.

/// Reports that the calling thread is about to wait for `what`, if it is the main thread and the build checks.
pub fn blocking(what: &str) {
    if cfg!(any(debug_assertions, feature = "testing")) && on_main_thread() {
        crate::diag::report("main thread blocks", format!("on {what}"));
    }
}

/// Whether this is the process's first thread: the standard library names it `main`, and this crate forbids the
/// `unsafe` that `pthread_main_np` needs.
fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawned_thread_is_not_the_main_thread() {
        let named = std::thread::Builder::new().name("worker".into()).spawn(on_main_thread);
        assert!(!named.unwrap().join().unwrap());
        assert!(!std::thread::spawn(on_main_thread).join().unwrap());
        blocking("a test that may run on any thread");
    }
}
