//! What keeps a session's secrets in it: the password prompt that turns on secure keyboard entry (F60).

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

#[allow(
    dead_code,
    reason = "the harness is shared with the session tests, which use the rest of it"
)]
mod common;

use common::{Harness, command, temporary};

#[test]
fn a_prompt_without_echo_reads_as_a_password_and_the_echo_ends_it() {
    let dir = temporary("password");
    let script = "printf 'start\\n'; read line; stty -echo; printf 'Password:'; read secret; stty echo; \
                  printf '\\nback\\n'; read line";
    let mut harness = Harness::start(command("/bin/sh", &["-c", script], &dir), &dir);
    assert!(harness.wait_until(|h| h.screen_line("start")));
    assert!(
        !harness.session.reads_password(),
        "echo is on while the shell reads a line"
    );
    harness.session.write("\n");
    assert!(
        harness.wait_until(|h| h.session.reads_password()),
        "stty -echo before the prompt"
    );
    harness.session.write("hunter2\n");
    assert!(harness.wait_until(|h| h.screen_line("back")));
    assert!(!harness.session.reads_password());
}
