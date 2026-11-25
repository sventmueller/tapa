//! Recordings under `fixtures/` replayed through the mapper and checked against the thread they produce; the
//! `recorded_*` ones come from the real CLI, the others are written from the SDK's typings.

#![allow(clippy::unwrap_used, reason = "test helpers, which allow-unwrap-in-tests misses")]

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use term_claude::PlanAnswer;
use term_claude::replay::Replay;
use term_thread::{
    Activity, BackgroundKind, ContextGroup, ContextKind, Decision, EntryKind, ErrorKind, HookOutcome, Lane,
    LimitStatus, Notice, PermissionOption, RequestState, RuleScope, Status, TaskStatus, Thread, ThreadId, TodoStatus,
    Tokens, ToolCall, ToolOutput, ToolStatus, ToolView, TurnUsage,
};

const STEP: Duration = Duration::from_millis(100);

fn fixture(name: &str) -> String {
    let path = format!("{}/tests/fixtures/{name}.ndjson", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn start() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_800_000_000)
}

fn replay(name: &str) -> Thread {
    let mut replay = new_replay();
    replay.feed_all(&fixture(name));
    replay.into_thread()
}

fn new_replay() -> Replay {
    Replay::new(
        Thread::new(ThreadId::from("fixture"), "claude", "/tmp/vibeedit-fixture"),
        start(),
        STEP,
    )
}

/// Plays `name` up to and including the first line that contains `marker`.
fn replay_until(name: &str, marker: &str) -> Replay {
    let mut replay = new_replay();
    let text = fixture(name);
    let mut lines = text.lines();
    for line in lines.by_ref() {
        replay.feed(line);
        if line.contains(marker) {
            return replay;
        }
    }
    panic!("{marker} is not in {name}");
}

fn kinds(thread: &Thread, lane: &Lane) -> Vec<&'static str> {
    thread
        .entries(lane)
        .iter()
        .map(|e| match &e.kind {
            EntryKind::UserMessage(_) => "user",
            EntryKind::AgentMessage(_) => "agent",
            EntryKind::Thought(_) => "thought",
            EntryKind::ToolCall(_) => "tool",
            EntryKind::Todos(_) => "todos",
            EntryKind::PermissionRequest(_) => "permission",
            EntryKind::Question(_) => "question",
            EntryKind::PlanApproval(_) => "plan",
            EntryKind::Notice(_) => "notice",
        })
        .collect()
}

fn tool<'a>(thread: &'a Thread, id: &str) -> &'a ToolCall {
    match &thread.entry_by_key(id).unwrap_or_else(|| panic!("no entry {id}")).kind {
        EntryKind::ToolCall(call) => call,
        other => panic!("{id} is {other:?}"),
    }
}

fn notices(thread: &Thread) -> Vec<&Notice> {
    thread
        .entries(&Lane::Main)
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::Notice(n) => Some(n),
            _ => None,
        })
        .collect()
}

#[test]
fn a_plain_answer_streams_its_thinking_and_text_and_ends_with_cost() {
    let thread = replay("plain_thinking");
    assert_eq!(kinds(&thread, &Lane::Main), ["user", "thought", "agent"]);
    let entries = thread.entries(&Lane::Main);
    let EntryKind::Thought(thought) = &entries[1].kind else {
        panic!()
    };
    assert_eq!(thought.text, "Simple arithmetic: two plus two is four.");
    assert!(!thought.streaming);
    assert!(thought.duration.unwrap() >= STEP);
    let EntryKind::AgentMessage(answer) = &entries[2].kind else {
        panic!()
    };
    assert_eq!(answer.markdown, "2 + 2 = **4**.");
    assert!(!answer.streaming);
    assert_eq!(thread.status(), Status::Idle);
    assert_eq!(thread.session(), Some("11111111-1111-4111-8111-111111111111"));
    assert_eq!(thread.model(), Some("claude-sonnet-5-5"));
    assert_eq!(thread.mode(), Some("default"));
    assert_eq!(thread.effort(), Some("medium"));
    let usage = thread.usage();
    assert_eq!(usage.cost_usd, Some(0.0123));
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.cache_read_tokens),
        (12, 34, 20000)
    );
    assert_eq!(usage.context_used, Some(20_046));
    assert_eq!(usage.context_size, Some(200_000));
}

#[test]
fn streamed_text_is_visible_before_the_message_is_complete() {
    let midway = replay_until("plain_thinking", r#""text":"2 + 2 = "}"#);
    let thread = midway.thread();
    assert_eq!(thread.status(), Status::Running);
    assert_eq!(thread.activity(), Some(Activity::Requesting));
    let EntryKind::AgentMessage(partial) = &thread.entries(&Lane::Main)[2].kind else {
        panic!()
    };
    assert_eq!(partial.markdown, "2 + 2 = ");
    assert!(partial.streaming);
}

#[test]
fn a_bash_permission_is_answered_with_an_edited_command() {
    let thread = replay("recorded_subagent_background");
    let request = thread
        .entry_by_key("perm:ee0823d8-3223-4e83-a9ff-2f8a87f509e6")
        .map(|e| &e.kind);
    let Some(EntryKind::PermissionRequest(request)) = request else {
        panic!("{request:?}")
    };
    assert_eq!(request.tool_name, "Bash");
    assert_eq!(request.tool_call.as_deref(), Some("toolu_01SLWVHmA2vx9w1ZrETd52UH"));
    assert!(request.input.is_null(), "an allowed request lets go of its input");
    assert_eq!(
        request.blocked_path.as_deref(),
        Some("/tmp/vibeedit-fixture/wb/marker.txt")
    );
    assert_eq!(
        request.state,
        RequestState::Decided(Decision::Allowed {
            always: false,
            edited: true
        })
    );
    assert_eq!(request.options.first(), Some(&PermissionOption::AllowOnce));
    assert_eq!(request.options.last(), Some(&PermissionOption::Deny));
    assert!(request.options.contains(&PermissionOption::AllowAlways {
        suggestion: 0,
        rule: "Bash(touch marker.txt && echo created)".into(),
        scope: RuleScope::ProjectLocal,
    }));
    assert!(request.options.contains(&PermissionOption::AllowAlways {
        suggestion: 2,
        rule: "mode acceptEdits".into(),
        scope: RuleScope::Session,
    }));
    let call = tool(&thread, "toolu_01SLWVHmA2vx9w1ZrETd52UH");
    assert_eq!(call.status, ToolStatus::Completed);
    assert_eq!(
        call.output,
        Some(ToolOutput::Terminal {
            stdout: "created\nedited-by-host".into(),
            stderr: String::new(),
            interrupted: false,
        })
    );
}

#[test]
fn the_thread_waits_for_the_user_while_a_permission_is_pending() {
    let midway = replay_until("recorded_subagent_background", r#""subtype":"can_use_tool""#);
    let thread = midway.thread();
    assert_eq!(thread.status(), Status::RequiresAction);
    assert_eq!(thread.pending_requests().len(), 1);
    assert_eq!(
        tool(thread, "toolu_01SLWVHmA2vx9w1ZrETd52UH").status,
        ToolStatus::Pending
    );
}

#[test]
fn edits_and_writes_show_their_diffs() {
    let thread = replay("recorded_edit_diff");
    let edit = tool(&thread, "toolu_016EgA8qyKQPYRPMvfai5osg");
    let ToolView::Edit { path, edits } = &edit.view else {
        panic!("{:?}", edit.view)
    };
    assert_eq!(path, "/tmp/vibeedit-fixture/wa/hello.txt");
    assert!(
        edits.is_empty(),
        "once the diff is there, it stands for the proposed change"
    );
    let Some(ToolOutput::Diff { hunks, .. }) = &edit.output else {
        panic!("{:?}", edit.output)
    };
    assert_eq!(hunks[0].lines, ["-hello world", "+hello there"]);
    assert_eq!((hunks[0].old_start, hunks[0].new_start), (1, 1));
    let write = tool(&thread, "toolu_01HXf2DyfBAerzfVpb3AYk2D");
    let Some(ToolOutput::Diff { hunks, .. }) = &write.output else {
        panic!("{:?}", write.output)
    };
    assert_eq!(hunks[0].lines, ["+hello world"]);
    let permissions = thread
        .entries(&Lane::Main)
        .iter()
        .filter(|e| matches!(&e.kind, EntryKind::PermissionRequest(r) if r.state == RequestState::Decided(Decision::Allowed { always: false, edited: false })))
        .count();
    assert_eq!(permissions, 2);
    let EntryKind::AgentMessage(answer) = &thread.entries(&Lane::Main).last().unwrap().kind else {
        panic!()
    };
    assert!(answer.markdown.contains("hello there"));
}

#[test]
fn a_subagent_has_its_own_lane_with_its_nested_tool_calls() {
    let thread = replay("recorded_subagent_background");
    let key = "toolu_01WS55D6AqJ1HALLYSx6F14m";
    let agent = thread.subagent(key).expect("the subagent");
    assert_eq!(agent.task_id.as_deref(), Some("a2b2a7f1224b0fe38"));
    assert_eq!(agent.agent_type.as_deref(), Some("general-purpose"));
    assert_eq!(agent.description, "list files");
    assert_eq!(agent.status, TaskStatus::Completed);
    assert_eq!(
        (agent.tokens, agent.tool_uses),
        (12_960, 1),
        "the run totals of the tool result come last"
    );
    assert_eq!(agent.last_tool.as_deref(), Some("Bash"));
    assert!(!agent.cancellable);
    let lane = Lane::Subagent(key.into());
    assert_eq!(kinds(&thread, &lane), ["user", "tool", "thought", "agent"]);
    let inner = tool(&thread, "toolu_0148n7gpLWVUaNJfBSC7TsMG");
    assert_eq!(inner.parent.as_deref(), Some(key));
    assert_eq!(inner.status, ToolStatus::Completed);
    assert!(
        !thread
            .entries(&Lane::Main)
            .iter()
            .any(|e| matches!(&e.kind, EntryKind::ToolCall(c) if c.call_id == inner.call_id))
    );
    let launcher = tool(&thread, key);
    assert_eq!(launcher.kind, term_thread::ToolKind::Think);
    assert_eq!(launcher.status, ToolStatus::Completed);
}

#[test]
fn a_running_subagent_reports_progress() {
    let midway = replay_until("recorded_subagent_background", r#""subtype":"task_progress""#);
    let agent = midway.thread().subagent("toolu_01WS55D6AqJ1HALLYSx6F14m").unwrap();
    assert_eq!(agent.status, TaskStatus::Running);
    assert!(agent.cancellable);
    assert_eq!(agent.tokens, 11_490);
    assert_eq!(agent.elapsed, Duration::from_millis(1127));
}

#[test]
fn a_background_command_lists_its_output_and_stays_as_stopped_when_stopped() {
    let midway = replay_until(
        "recorded_subagent_background",
        r#""subtype":"success","request_id":"out-1""#,
    );
    let thread = midway.thread();
    let [item] = thread.background() else {
        panic!("{:?}", thread.background())
    };
    assert_eq!(item.id, "bwwn10k3r");
    assert_eq!(item.kind, BackgroundKind::Shell);
    assert_eq!(item.status, TaskStatus::Running);
    assert_eq!(item.tool_call.as_deref(), Some("toolu_01DrrGWYR44BhueQNmb9524K"));
    assert_eq!(item.output.text.as_deref(), Some("tick 1\ntick 2\ntick 3\ntick 4\n"));
    assert_eq!(item.output.total_bytes, Some(28));
    assert!(item.started.is_some());
    let card = tool(thread, "toolu_01DrrGWYR44BhueQNmb9524K");
    assert_eq!(card.background_task.as_deref(), Some("bwwn10k3r"));
    assert!(matches!(card.view, ToolView::Command { background: true, .. }));

    let thread = replay("recorded_subagent_background");
    let [item] = thread.background() else {
        panic!("{:?}", thread.background())
    };
    assert_eq!(item.status, TaskStatus::Stopped, "the stopped task stays, as stopped");
    let ended = item.ended.and_then(|at| at.duration_since(UNIX_EPOCH).ok());
    assert_eq!(
        ended.map(|d| d.as_millis()),
        Some(1_794_247_393_969),
        "it ended when the CLI says"
    );
    assert_eq!(item.output.text.as_deref(), Some("tick 1\ntick 2\ntick 3\ntick 4\n"));
    assert_eq!(thread.status(), Status::Idle);
}

#[test]
fn compaction_reports_the_freed_tokens_and_the_smaller_context() {
    let midway = replay_until("compaction", r#""status":"compacting""#);
    assert_eq!(midway.thread().activity(), Some(Activity::Compacting));
    let thread = replay("compaction");
    let [notice] = notices(&thread)[..] else { panic!() };
    assert_eq!(notice.freed_tokens(), Some(138_000));
    assert!(matches!(notice, Notice::Compaction { manual: false, .. }));
    assert_eq!(thread.usage().context_used, Some(42_000));
    assert_eq!(thread.activity(), None);
    assert_eq!(thread.mode(), Some("acceptEdits"));
}

#[test]
fn retries_update_one_notice_with_attempt_and_delay() {
    let thread = replay("api_retry");
    assert_eq!(kinds(&thread, &Lane::Main), ["user", "notice", "agent"]);
    let [
        Notice::Retrying {
            attempt,
            max_attempts,
            delay,
            error,
            http_status,
        },
    ] = notices(&thread)[..]
    else {
        panic!()
    };
    assert_eq!((*attempt, *max_attempts), (2, 10));
    assert_eq!(*delay, Duration::from_millis(2400));
    assert_eq!(error, "server_error");
    assert_eq!(*http_status, None);
}

#[test]
fn rate_limits_are_kept_per_window_and_shown_once_they_bite() {
    let thread = replay("rate_limit");
    let five = thread.rate_limits().iter().find(|l| l.window == "five_hour").unwrap();
    assert_eq!(five.status, LimitStatus::Rejected);
    assert_eq!(five.utilization, Some(1.0));
    assert_eq!(five.resets_at, Some(UNIX_EPOCH + Duration::from_secs(1_794_276_000)));
    let shown: Vec<LimitStatus> = notices(&thread)
        .iter()
        .filter_map(|n| match n {
            Notice::RateLimit(limit) => Some(limit.status),
            _ => None,
        })
        .collect();
    assert_eq!(shown, [LimitStatus::Warning, LimitStatus::Rejected]);
}

#[test]
fn unknown_messages_subtypes_and_fields_are_ignored() {
    let thread = replay("future");
    assert_eq!(kinds(&thread, &Lane::Main), ["agent"]);
    let EntryKind::AgentMessage(answer) = &thread.entries(&Lane::Main)[0].kind else {
        panic!()
    };
    assert_eq!(answer.markdown, "Still readable.");
    assert_eq!(thread.mode(), Some("telepathic"));
    assert_eq!(thread.model(), Some("claude-future-9"));
    assert_eq!(thread.usage().cost_usd, Some(0.5));
    assert_eq!(thread.usage().context_size, Some(2_000_000));
    assert_eq!(thread.status(), Status::Idle);
}

#[test]
fn frames_of_an_older_task_run_are_ignored() {
    let thread = replay("late_runs");
    let agent = thread.subagent("toolu_agent").unwrap();
    assert_eq!(
        agent.status,
        TaskStatus::Running,
        "a stale failure does not end the current run"
    );
    assert_eq!(agent.tokens, 5000);
    assert_eq!(agent.last_tool.as_deref(), Some("Read"));
    assert_eq!(agent.summary.as_deref(), Some("Reading the parser"));
    assert!(agent.backgrounded);
    let ids: Vec<&str> = thread.background().iter().map(|b| b.id.as_str()).collect();
    assert_eq!(
        ids,
        ["agent1", "mon1"],
        "a set naming an older run does not replace the newer one"
    );
    assert_eq!(thread.background()[0].kind, BackgroundKind::Subagent);
    assert_eq!(thread.background()[1].kind, BackgroundKind::Monitor);
    assert!(thread.background()[1].ambient);
}

#[test]
fn todos_plans_denials_and_withdrawn_requests_are_tracked() {
    let thread = replay("plan_todos");
    assert_eq!(
        kinds(&thread, &Lane::Main),
        ["todos", "plan", "tool", "permission", "todos", "tool", "permission"]
    );
    assert_eq!(thread.todos().len(), 2);
    assert_eq!(thread.todos()[1].status, TodoStatus::InProgress);
    let Some(EntryKind::PlanApproval(plan)) = thread.entry_by_key("toolu_plan").map(|e| &e.kind) else {
        panic!()
    };
    assert_eq!(plan.state, RequestState::Decided(Decision::Approved));
    assert!(plan.plan.starts_with("## Plan"));
    let Some(EntryKind::PermissionRequest(denied)) = thread.entry_by_key("perm:perm-rm").map(|e| &e.kind) else {
        panic!()
    };
    assert!(denied.default_to_no);
    assert_eq!(denied.reason.as_deref(), Some("This command deletes files"));
    assert_eq!(
        denied.state,
        RequestState::Decided(Decision::Denied {
            message: Some("Do not delete files.".into())
        })
    );
    assert_eq!(tool(&thread, "toolu_rm").status, ToolStatus::Failed);
    let Some(EntryKind::PermissionRequest(withdrawn)) = thread.entry_by_key("perm:perm-test").map(|e| &e.kind) else {
        panic!()
    };
    assert_eq!(withdrawn.state, RequestState::Withdrawn);
    assert!(thread.pending_requests().is_empty());
    assert_eq!(thread.mode(), Some("acceptEdits"));
}

#[test]
fn a_question_is_answered_with_the_chosen_option() {
    let thread = replay("recorded_question");
    let question = thread
        .entries(&Lane::Main)
        .iter()
        .find_map(|e| match &e.kind {
            EntryKind::Question(q) => Some(q),
            _ => None,
        })
        .unwrap();
    assert_eq!(question.items[0].question, "Which color do you prefer?");
    assert_eq!(question.items[0].options.len(), 2);
    assert!(!question.items[0].multi_select);
    let RequestState::Decided(Decision::Answered(answers)) = &question.state else {
        panic!("{:?}", question.state)
    };
    assert_eq!(answers[0].answer, "red");
    let users = kinds(&thread, &Lane::Main).iter().filter(|k| **k == "user").count();
    assert_eq!(users, 2, "both prompts of the two turns are in the timeline");
}

#[test]
fn hooks_are_shown_with_their_outcome_until_the_conversation_is_reset() {
    let midway = replay_until("hooks", r#""subtype":"informational""#);
    let thread = midway.thread();
    let hooks: Vec<(&str, HookOutcome, &str)> = notices(thread)
        .iter()
        .filter_map(|n| match n {
            Notice::Hook {
                name, outcome, output, ..
            } => Some((name.as_str(), *outcome, output.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(
        hooks,
        [
            ("PreToolUse:Bash", HookOutcome::Success, "checked: ok"),
            ("Stop", HookOutcome::Error, "lint failed")
        ]
    );
    assert_eq!(thread.status(), Status::RequiresAction);
    let thread = replay("hooks");
    assert!(thread.entries(&Lane::Main).is_empty());
}

#[test]
fn a_missing_sign_in_is_reported_once_as_an_auth_error() {
    let thread = replay("recorded_signed_out");
    let errors: Vec<&Notice> = notices(&thread)
        .into_iter()
        .filter(|n| matches!(n, Notice::Error { .. }))
        .collect();
    let [Notice::Error { kind, message }] = errors[..] else {
        panic!("{errors:?}")
    };
    assert_eq!(*kind, ErrorKind::Auth);
    assert!(message.contains("/login"));
    assert_eq!(thread.status(), Status::Idle);
}

#[test]
fn an_interrupted_turn_closes_its_stream_without_an_error() {
    let thread = replay("interrupt");
    assert_eq!(kinds(&thread, &Lane::Main), ["user", "agent", "notice"]);
    let EntryKind::AgentMessage(partial) = &thread.entries(&Lane::Main)[1].kind else {
        panic!()
    };
    assert_eq!(partial.markdown, "Once upon a time");
    assert!(!partial.streaming);
    assert!(matches!(notices(&thread)[..], [Notice::Interrupted]));
    assert_eq!(thread.status(), Status::Idle);
}

#[test]
fn a_plan_answered_in_a_replay_is_decided_as_a_session_decides_it() {
    let plan_state = |replay: &Replay| match replay.thread().entry_by_key("toolu_plan").map(|e| &e.kind) {
        Some(EntryKind::PlanApproval(plan)) => plan.state.clone(),
        other => panic!("{other:?}"),
    };
    let feedback = "> 1. Fix the parser\n\nWrite the failing test first";
    let mut revised = replay_until("plan_todos", "\"request_id\":\"perm-plan\"");
    assert_eq!(plan_state(&revised), RequestState::Pending);
    let answer = PlanAnswer::Revise(Some(feedback.into()));
    assert!(!revised.answer_plan("perm-plan", &answer).is_empty());
    assert_eq!(plan_state(&revised), RequestState::Decided(answer.decision()));
    assert!(
        revised.answer_plan("perm-plan", &answer).is_empty(),
        "a request is answered once"
    );
    let stopped = RequestState::Decided(Decision::Rejected {
        feedback: None,
        stop: true,
    });
    let mut rejected = replay_until("plan_todos", "\"request_id\":\"perm-plan\"");
    rejected.answer_plan("perm-plan", &PlanAnswer::Reject);
    assert_eq!(plan_state(&rejected), stopped);
    rejected.feed(&plan_result("The user doesn't want to proceed with this tool use."));
    assert_eq!(
        plan_state(&rejected),
        stopped,
        "the CLI's result does not replace the user's answer"
    );
}

/// The CLI's error result of the plan in `plan_todos`, saying `text`.
fn plan_result(text: &str) -> String {
    serde_json::json!({"type": "user", "message": {"role": "user", "content": [{"tool_use_id": "toolu_plan",
        "type": "tool_result", "content": text, "is_error": true}]}, "parent_tool_use_id": null,
        "session_id": "11111111-1111-4111-8111-111111111111", "uuid": "d7000000-0000-4000-8000-0000000000ff"})
    .to_string()
}

#[test]
fn a_plan_answered_before_a_transcript_was_saved_is_read_back_from_its_result() {
    let read_back = |text: &str| {
        let mut replay = replay_until("plan_todos", "\"name\":\"ExitPlanMode\"");
        replay.feed(&plan_result(text));
        match replay.thread().entry_by_key("toolu_plan").map(|e| &e.kind) {
            Some(EntryKind::PlanApproval(plan)) => plan.state.clone(),
            other => panic!("{other:?}"),
        }
    };
    let rejected = |feedback: Option<&str>, stop| {
        RequestState::Decided(Decision::Rejected {
            feedback: feedback.map(str::to_string),
            stop,
        })
    };
    assert_eq!(
        read_back(term_claude::control::PLAN_REJECT_MESSAGE),
        rejected(None, true)
    );
    assert_eq!(
        read_back(term_claude::control::PLAN_REVISE_MESSAGE),
        rejected(None, false)
    );
    assert_eq!(
        read_back("> step 1\n\nsmaller"),
        rejected(Some("> step 1\n\nsmaller"), false)
    );
}

#[test]
fn a_session_signed_in_with_a_subscription_says_so_and_one_without_an_answer_does_not() {
    let recorded = replay("recorded_question");
    assert!(
        recorded.usage().subscription,
        "the recorded account is on a Pro plan without an API key"
    );
    assert!(recorded.usage().cost_usd.is_some() && recorded.usage().billed_cost().is_none());
    let written = replay("context_cost");
    assert!(!written.usage().subscription && written.usage().billed_cost().is_some());
}

#[test]
fn every_fixture_replays_without_panicking_line_by_line() {
    let dir = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
    for entry in std::fs::read_dir(dir).unwrap() {
        let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
        let mut replay = new_replay();
        for line in text.lines() {
            replay.feed(line);
            replay.feed(&line[..line.len() / 2]);
        }
    }
}

/// The `uuid` and `follows` of the entry with `key`.
fn chain<'a>(thread: &'a Thread, key: &str) -> (Option<&'a str>, Option<&'a str>) {
    let entry = thread.entry_by_key(key).unwrap_or_else(|| panic!("no entry {key}"));
    (entry.uuid.as_deref(), entry.follows.as_deref())
}

#[test]
fn each_entry_of_the_conversation_knows_the_frames_to_resume_at() {
    let thread = replay("recorded_question");
    let first = "a9a149e3-abe6-4449-8275-4c8b78133094";
    assert_eq!(chain(&thread, &format!("user:{first}")), (Some(first), None));
    assert_eq!(
        chain(&thread, "toolu_012i8frTirufbBVRo3n3gcWE"),
        (
            Some("827ebb10-0277-4f48-a636-08ee5213a3a2"),
            Some("90431d92-688b-4dea-bc0d-a8177a028912")
        ),
        "the question ends with its tool result and follows the thinking before its call"
    );
    let entries = thread.entries(&Lane::Main);
    let red = entries
        .iter()
        .find(|e| matches!(&e.kind, EntryKind::AgentMessage(m) if m.markdown == "red"))
        .unwrap();
    assert_eq!(red.uuid.as_deref(), Some("2103b56c-877d-4d8a-aad7-b4015128fa10"));
    assert_eq!(red.follows.as_deref(), Some("827ebb10-0277-4f48-a636-08ee5213a3a2"));
    let second = "d9a6f9ff-59a7-47f0-bde5-53b0f0ea8d1d";
    assert_eq!(
        chain(&thread, &format!("user:{second}")),
        (Some(second), Some("2103b56c-877d-4d8a-aad7-b4015128fa10")),
        "dropping the second prompt resumes at the end of the first turn"
    );
    let done = entries.last().unwrap();
    assert_eq!(done.uuid.as_deref(), Some("e85cf8d7-101b-4717-a815-c47add9fd1b4"));
    assert_eq!(done.follows.as_deref(), Some(second));
    let thoughts: Vec<Option<&str>> = entries
        .iter()
        .filter(|e| matches!(e.kind, EntryKind::Thought(_)))
        .map(|e| e.uuid.as_deref())
        .collect();
    assert_eq!(
        thoughts,
        [
            Some("a013d65d-e705-4a48-bc7d-f06ff8114593"),
            Some("f47dc001-ecd6-49e3-87eb-85c4eca8a38c"),
            Some("90431d92-688b-4dea-bc0d-a8177a028912")
        ]
    );
}

#[test]
fn tool_cards_resume_after_their_result_and_notices_at_the_entry_before() {
    let thread = replay("recorded_edit_diff");
    assert_eq!(
        chain(&thread, "toolu_01HXf2DyfBAerzfVpb3AYk2D"),
        (
            Some("35bc42ca-9bbd-4b14-8039-46e03e68f959"),
            Some("ec196b7a-5861-43b6-9a06-4daec59c63b9")
        )
    );
    assert_eq!(
        chain(&thread, "toolu_016EgA8qyKQPYRPMvfai5osg"),
        (
            Some("6bb312ca-0fb0-4931-bb2e-d628042332d9"),
            Some("35bc42ca-9bbd-4b14-8039-46e03e68f959")
        )
    );
    let entries = thread.entries(&Lane::Main);
    let notice = entries.iter().find(|e| matches!(e.kind, EntryKind::Notice(_))).unwrap();
    assert_eq!((notice.uuid.as_deref(), notice.follows.as_deref()), (None, None));
    assert_eq!(
        thread.resume_point(notice.id),
        Some("35bc42ca-9bbd-4b14-8039-46e03e68f959")
    );
    let answer = entries.last().unwrap();
    assert_eq!(answer.uuid.as_deref(), Some("df9b2582-a9f1-4cb3-ab30-c68e4654d357"));
    assert_eq!(thread.resume_point(answer.id), answer.uuid.as_deref());
}

#[test]
fn streamed_entries_and_subagent_launches_get_their_frames_while_subagent_lanes_do_not() {
    let thread = replay("plain_thinking");
    let entries = thread.entries(&Lane::Main);
    let user = "a0000000-0000-4000-8000-000000000001";
    assert_eq!(entries[1].uuid.as_deref(), Some("d0000000-0000-4000-8000-000000000001"));
    assert_eq!(entries[1].follows.as_deref(), Some(user));
    assert_eq!(entries[2].uuid.as_deref(), Some("d0000000-0000-4000-8000-000000000002"));
    assert_eq!(
        entries[2].follows.as_deref(),
        Some("d0000000-0000-4000-8000-000000000001")
    );

    let thread = replay("recorded_subagent_background");
    let key = "toolu_01WS55D6AqJ1HALLYSx6F14m";
    assert_eq!(chain(&thread, key).0, Some("3a68a225-9c26-40ad-abf9-ab32f079edce"));
    let lane = Lane::Subagent(key.into());
    assert!(
        thread
            .entries(&lane)
            .iter()
            .all(|e| e.uuid.is_none() && e.follows.is_none())
    );
    let inner = thread.entries(&lane)[0].id;
    assert_eq!(thread.resume_point(inner), None);
}

fn prompt(thread: &Thread, uuid: &str) -> term_thread::EntryId {
    thread.entry_by_key(&format!("user:{uuid}")).unwrap().id
}

#[test]
fn rewinding_before_a_prompt_forks_at_the_end_of_the_previous_turn_and_drops_its_own() {
    let thread = replay("recorded_question");
    let second = prompt(&thread, "d9a6f9ff-59a7-47f0-bde5-53b0f0ea8d1d");
    assert_eq!(
        thread.rewind_before(second),
        Some(term_thread::Rewind {
            fork_at: "2103b56c-877d-4d8a-aad7-b4015128fa10".into(),
            drops_turn: "d9a6f9ff-59a7-47f0-bde5-53b0f0ea8d1d".into(),
        })
    );
    let first = prompt(&thread, "a9a149e3-abe6-4449-8275-4c8b78133094");
    assert_eq!(
        thread.rewind_before(first),
        None,
        "before the first prompt a new session starts"
    );
    let answer = thread.entries(&Lane::Main).last().unwrap().id;
    assert_eq!(thread.rewind_before(answer), None, "only prompts are rewound to");
}

/// CLI 2.1.319 accepted this rewind with `--resume-drops-turn`: an interrupted turn ends with its tool results and the
/// interruption marker, which the timeline does not show.
#[test]
fn after_an_interrupted_turn_the_rewind_keeps_its_tool_results_and_marker() {
    let thread = replay("recorded_interrupted_tools");
    let second = prompt(&thread, "16335700-c748-429b-94e4-69ce2fcc0a70");
    assert_eq!(
        thread.rewind_before(second),
        Some(term_thread::Rewind {
            fork_at: "4ab65f99-e44b-4d74-aded-f7912cc01437".into(),
            drops_turn: "16335700-c748-429b-94e4-69ce2fcc0a70".into(),
        })
    );
    let cards: Vec<ToolStatus> = thread
        .entries(&Lane::Main)
        .iter()
        .filter_map(|e| match &e.kind {
            EntryKind::ToolCall(call) => Some(call.status),
            _ => None,
        })
        .collect();
    assert_eq!(cards, [ToolStatus::Completed, ToolStatus::Failed]);
    assert!(notices(&thread).contains(&&Notice::Interrupted));
}

#[test]
fn a_refused_rewind_is_reported_as_such_and_not_as_an_error() {
    let thread = replay("recorded_rewind_rejected");
    let [Notice::RewindRejected { detail }] = notices(&thread)[..] else {
        panic!("{:?}", notices(&thread))
    };
    assert!(detail.starts_with("Resume rejected by --resume-drops-turn:"));
    assert_eq!(thread.status(), Status::Idle);
}

fn close(a: Option<f64>, b: f64) -> bool {
    a.is_some_and(|a| (a - b).abs() < 1e-9)
}

fn turn_message(thread: &Thread, turn: &TurnUsage) -> Option<String> {
    let entry = thread.entry(turn.message?)?;
    match &entry.kind {
        EntryKind::UserMessage(m) => Some(m.text.clone()),
        _ => None,
    }
}

#[test]
fn each_turn_costs_the_difference_of_the_session_totals_before_and_after_it() {
    let thread = replay("context_cost");
    let usage = thread.usage();
    let [first, second] = usage.turns.as_slice() else {
        panic!("{:?}", usage.turns)
    };
    assert!(close(first.cost_usd, 0.0031), "{first:?}");
    assert_eq!(
        first.tokens,
        Some(Tokens {
            input: 6,
            output: 120,
            cache_read: 18_000,
            cache_write: 2_500
        })
    );
    assert_eq!(first.duration, Some(Duration::from_millis(2_400)));
    assert_eq!(turn_message(&thread, first).as_deref(), Some("First question"));
    assert!(close(second.cost_usd, 0.0016), "{second:?}");
    assert_eq!(
        second.tokens,
        Some(Tokens {
            input: 7,
            output: 110,
            cache_read: 21_000,
            cache_write: 1_300
        }),
        "a turn counts the tokens of every model it used"
    );
    assert_eq!(turn_message(&thread, second).as_deref(), Some("Second question"));
    assert!(close(usage.cost_usd, 0.0047));
    let models: Vec<&str> = usage.models.iter().map(|m| m.model.as_str()).collect();
    assert_eq!(
        models,
        ["claude-haiku-5-5", "claude-sonnet-5-5"],
        "the most expensive first"
    );
}

#[test]
fn the_context_breakdown_follows_the_latest_answer_and_says_whether_it_was_counted() {
    let estimated = replay_until("context_cost", r#""request_id":"req-2","response""#).into_thread();
    let breakdown = estimated
        .usage()
        .breakdown
        .clone()
        .expect("the summary after the first turn");
    assert!(!breakdown.exact);
    assert_eq!(breakdown.used, 21_000);
    let thread = replay("context_cost");
    let usage = thread.usage();
    let breakdown = usage.breakdown.as_ref().expect("the full answer");
    assert!(breakdown.exact);
    assert_eq!((usage.context_used, usage.context_size), (Some(21_480), Some(200_000)));
    assert_eq!((breakdown.used, breakdown.size), (21_480, 200_000));
    assert_eq!(breakdown.auto_compact_at, Some(167_000));
    let kinds: Vec<ContextKind> = breakdown.rows.iter().map(|r| r.kind).collect();
    assert_eq!(kinds.iter().filter(|k| **k == ContextKind::Used).count(), 5);
    assert!(kinds.contains(&ContextKind::Free) && kinds.contains(&ContextKind::Buffer));
    assert!(kinds.contains(&ContextKind::Deferred));
    assert_eq!(breakdown.rows[0].id.as_deref(), Some("system_prompt"));
    let part = |group: ContextGroup| -> Vec<&str> {
        breakdown
            .parts
            .iter()
            .filter(|p| p.group == group)
            .map(|p| p.name.as_str())
            .collect()
    };
    assert_eq!(part(ContextGroup::MemoryFile), ["/tmp/vibeedit-fixture/cost/CLAUDE.md"]);
    assert_eq!(part(ContextGroup::McpTool), ["context7: query-docs"]);
    assert_eq!(part(ContextGroup::SystemTool), ["Bash", "Read"]);
    assert_eq!(part(ContextGroup::Message), ["assistant_messages", "user_messages"]);
}

#[test]
fn a_resumed_session_learns_its_totals_from_get_usage_before_the_first_turn() {
    let mut replay = new_replay().resumed();
    replay.feed_all(&fixture("resumed_cost"));
    let usage = replay.thread().usage();
    let turn = usage.last_turn().expect("the turn");
    assert!(close(turn.cost_usd, 0.0021), "{turn:?}");
    assert_eq!(
        turn.tokens,
        Some(Tokens {
            input: 3,
            output: 100,
            cache_read: 21_000,
            cache_write: 400
        })
    );
}

#[test]
fn without_its_totals_a_resumed_session_leaves_the_first_turns_cost_unknown() {
    let mut replay = new_replay().resumed();
    let recording = fixture("resumed_cost");
    for line in recording.lines().filter(|l| !l.contains("req-2")) {
        replay.feed(line);
    }
    let usage = replay.thread().usage();
    let turn = usage.last_turn().expect("the turn");
    assert_eq!((turn.cost_usd, turn.tokens), (None, None), "unknown rather than wrong");
    assert!(close(usage.cost_usd, 0.0141), "the session's total is the CLI's");
}

#[test]
fn totals_answered_after_a_prompt_was_sent_are_not_taken_as_the_turns_start() {
    let recording = fixture("resumed_cost");
    let lines: Vec<&str> = recording.lines().collect();
    let prompt = lines
        .iter()
        .position(|l| l.starts_with(">> ") && l.contains(r#""type":"user""#));
    let answer = lines
        .iter()
        .position(|l| l.contains(r#""request_id":"req-2","response""#));
    let (Some(prompt), Some(answer)) = (prompt, answer) else {
        panic!("the fixture has a prompt and a get_usage answer")
    };
    let mut reordered = lines.clone();
    let moved = reordered.remove(prompt);
    reordered.insert(answer, moved);
    let mut replay = new_replay().resumed();
    for line in reordered {
        replay.feed(line);
    }
    let turn = replay.thread().usage().last_turn().cloned().expect("the turn");
    assert_eq!(turn.cost_usd, None, "the answer may count part of the turn");
}
