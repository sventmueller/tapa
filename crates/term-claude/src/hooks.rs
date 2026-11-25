//! The hooks the app registers with the CLI (E27): a `PreToolUse` hook on Bash that makes destructive git commands
//! ask, whatever rules are stored or the permission mode allows.

use serde_json::{Value, json};

/// The id of the app's own `PreToolUse` hook, which makes destructive git commands ask (E27).
pub(crate) const DESTRUCTIVE_GIT_HOOK: &str = "term-destructive-git";

/// The reason that goes with the `ask`; the permission card shows the app's own words instead.
const DESTRUCTIVE_GIT_REASON: &str = "Destructive git command: it needs the user's approval every time.";

/// The hooks the app registers with `initialize`; the CLI runs `PreToolUse` before stored allow rules and the
/// permission mode, and sends its `ask` to `can_use_tool` even under `bypassPermissions` (checked in CLI 2.1.322).
pub(crate) fn registered() -> Value {
    json!({
        "PreToolUse": [{
            "matcher": "Bash",
            "hookCallbackIds": [DESTRUCTIVE_GIT_HOOK],
            "onFailure": "block",
        }]
    })
}

/// The answer to a `hook_callback` for the app's hook `callback_id` with the hook's `input`: `ask` for a Bash call
/// that runs a destructive git command, else nothing to add. `None` for a hook the app did not register.
pub(crate) fn hook_answer(callback_id: &str, input: &Value) -> Option<Value> {
    if callback_id != DESTRUCTIVE_GIT_HOOK {
        return None;
    }
    let bash = input.get("tool_name").and_then(Value::as_str) == Some("Bash");
    let destructive = bash
        && input
            .get("tool_input")
            .and_then(term_thread::shell::destructive_input)
            .is_some();
    if !destructive {
        return Some(json!({}));
    }
    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "ask",
            "permissionDecisionReason": DESTRUCTIVE_GIT_REASON,
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{Incoming, Request};

    #[test]
    fn initialize_registers_the_hook_that_makes_destructive_git_ask() {
        let body = Request::Initialize {
            per_task_stop: true,
            progress_summaries: false,
            forward_subagent_text: false,
        }
        .body();
        assert_eq!(
            body["hooks"]["PreToolUse"],
            json!([{"matcher": "Bash", "hookCallbackIds": [DESTRUCTIVE_GIT_HOOK], "onFailure": "block"}])
        );
    }

    #[test]
    fn the_hook_asks_for_destructive_git_and_adds_nothing_otherwise() {
        let call = |tool: &str, command: &str| json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": {"command": command}, "tool_use_id": "t"});
        for command in [
            "git push -f",
            "git push --force-with-lease",
            "git -C x reset --hard",
            "git clean -fdx",
            "cargo test && git reset --hard",
            "sh -c 'git push --force'",
        ] {
            let answer = hook_answer(DESTRUCTIVE_GIT_HOOK, &call("Bash", command)).unwrap();
            assert_eq!(answer["hookSpecificOutput"]["permissionDecision"], "ask", "{command}");
            assert_eq!(answer["hookSpecificOutput"]["hookEventName"], "PreToolUse");
        }
        assert_eq!(
            hook_answer(DESTRUCTIVE_GIT_HOOK, &call("Bash", "git push")),
            Some(json!({}))
        );
        assert_eq!(
            hook_answer(DESTRUCTIVE_GIT_HOOK, &call("Write", "git push -f")),
            Some(json!({}))
        );
        assert_eq!(hook_answer("someone-else", &call("Bash", "git push -f")), None);
        assert_eq!(
            Incoming::read(
                &json!({"subtype": "hook_callback", "callback_id": "h", "input": {"a": 1}, "tool_use_id": "t"})
            ),
            Incoming::HookCallback {
                callback_id: "h".into(),
                input: json!({"a": 1})
            }
        );
    }
}
