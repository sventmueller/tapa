use super::*;
use term_thread::{HookOutcome, Notice, ThreadId, Update, UserMessage};

fn hook(thread: &mut Thread, id: &str, outcome: HookOutcome) {
    thread.apply(Update::Put {
        lane: Lane::Main,
        key: Some(format!("hook:{id}")),
        at: std::time::UNIX_EPOCH,
        entry: EntryKind::Notice(Notice::Hook {
            id: id.into(),
            name: "PostToolUse:Bash".into(),
            event: "PostToolUse".into(),
            output: String::new(),
            exit_code: None,
            outcome,
        }),
    });
}

fn prompt(thread: &mut Thread) {
    thread.apply(Update::Put {
        lane: Lane::Main,
        key: None,
        at: std::time::UNIX_EPOCH,
        entry: EntryKind::UserMessage(UserMessage::default()),
    });
}

#[test]
fn hooks_that_ran_fine_before_the_first_prompt_get_no_row() {
    let mut thread = Thread::new(ThreadId::from("t"), "test", "/tmp");
    hook(&mut thread, "start", HookOutcome::Success);
    hook(&mut thread, "failed", HookOutcome::Error);
    assert_eq!(shown(&thread, false).len(), 1, "only the failed hook shows");
    prompt(&mut thread);
    hook(&mut thread, "a", HookOutcome::Success);
    assert_eq!(shown(&thread, false).len(), 3);
}

#[test]
fn running_hooks_get_no_row_until_they_fail() {
    let mut thread = Thread::new(ThreadId::from("t"), "test", "/tmp");
    prompt(&mut thread);
    hook(&mut thread, "a", HookOutcome::Success);
    hook(&mut thread, "b", HookOutcome::Running);
    assert_eq!(shown(&thread, false).len(), 2);
    hook(&mut thread, "b", HookOutcome::Success);
    assert_eq!(shown(&thread, false).len(), 2);
    hook(&mut thread, "c", HookOutcome::Running);
    assert_eq!(shown(&thread, false).len(), 2);
    hook(&mut thread, "c", HookOutcome::Error);
    assert_eq!(shown(&thread, false).len(), 3);
}
