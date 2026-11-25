//! How hard the terminal's threads compete for the processor: reading the PTY and drawing first, compression,
//! suggestions and git last.

/// How much a thread's work matters to the user right now; a thread nobody sets one for runs at macOS's `default`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qos {
    /// Answers the user waits for: the terminal's output and its frames.
    UserInitiated,
    /// Work whose result shows later: suggestions, git state, waiting for processes.
    Utility,
    /// Upkeep nobody waits for, such as compressing cold history.
    Background,
}

/// Gives the calling thread `qos`.
pub fn set_qos(qos: Qos) {
    let class = match qos {
        Qos::UserInitiated => libc::qos_class_t::QOS_CLASS_USER_INITIATED,
        Qos::Utility => libc::qos_class_t::QOS_CLASS_UTILITY,
        Qos::Background => libc::qos_class_t::QOS_CLASS_BACKGROUND,
    };
    // SAFETY: Changes only the calling thread's class; a refusal leaves it as it was.
    unsafe { libc::pthread_set_qos_class_self_np(class, 0) };
}

/// `set_qos(Qos::UserInitiated)`, as a lane's start.
pub fn user_initiated() {
    set_qos(Qos::UserInitiated);
}

/// `set_qos(Qos::Utility)`, as a lane's start.
pub fn utility() {
    set_qos(Qos::Utility);
}

/// `set_qos(Qos::Background)`, as a lane's start.
pub fn background() {
    set_qos(Qos::Background);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_thread_gets_the_class_it_asks_for() {
        let class = std::thread::spawn(|| {
            set_qos(Qos::Utility);
            let mut class = libc::qos_class_t::QOS_CLASS_UNSPECIFIED;
            // SAFETY: Reads the calling thread's class into a local.
            unsafe { libc::pthread_get_qos_class_np(libc::pthread_self(), &raw mut class, std::ptr::null_mut()) };
            class as u32
        })
        .join()
        .unwrap();
        assert_eq!(class, libc::qos_class_t::QOS_CLASS_UTILITY as u32);
    }
}
