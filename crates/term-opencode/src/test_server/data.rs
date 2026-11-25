//! The fixed answers of the stand-in's management operations, shaped as the schema's response types.

use serde_json::{Value, json};

use super::{FORK, PERMISSION, SESSION};

/// The agents: two to pick, a subagent and a hidden one.
pub(super) fn agents(location: &Value) -> Value {
    let agent = |id: &str, mode: &str, hidden: bool, description: &str| {
        json!({"id": id, "name": id, "description": description, "mode": mode, "hidden": hidden,
               "request": {"settings": {}, "headers": {}, "body": {}}, "permissions": []})
    };
    json!({"location": location, "data": [
        agent("build", "primary", false, "Builds"),
        agent("plan", "primary", false, "Plans without changing files"),
        agent("general", "subagent", false, "Researches"),
        agent("title", "primary", true, "Names sessions")]})
}

fn model(id: &str, provider: &str, name: &str, variants: &[&str], input: &[&str], context: u64) -> Value {
    json!({"id": id, "modelID": id, "providerID": provider, "name": name,
           "capabilities": {"tools": true, "input": input, "output": ["text"]},
           "variants": variants.iter().map(|v| json!({"id": v})).collect::<Vec<_>>(),
           "time": {"released": 1_700_000_000_000u64}, "cost": [], "status": "active", "enabled": true,
           "limit": {"context": context, "output": 64000}})
}

/// The models, none in the scenario `nomodel`.
pub(super) fn models(location: &Value, scenario: &str) -> Value {
    if scenario == "nomodel" {
        return json!({"location": location, "data": []});
    }
    json!({"location": location, "data": [
        model("claude-sonnet-4", "anthropic", "Claude Sonnet 4", &["high", "max"], &["text", "image"], 200_000),
        model("gpt-5", "openai", "GPT-5", &["low", "high"], &["text"], 400_000)]})
}

/// The default model, none in the scenario `nomodel`.
pub(super) fn default_model(location: &Value, scenario: &str) -> Value {
    if scenario == "nomodel" {
        return json!({"location": location, "data": null});
    }
    let all = models(location, scenario);
    json!({"location": location, "data": all["data"][0]})
}

pub(super) fn providers(location: &Value) -> Value {
    json!({"location": location, "data": [
        {"id": "anthropic", "name": "Anthropic", "activation": "enabled", "package": "@ai-sdk/anthropic"},
        {"id": "openai", "name": "OpenAI", "activation": "auto", "package": "@ai-sdk/openai"}]})
}

/// A `Session.Info` of the session `id`.
pub(super) fn session(id: &str, directory: &str, title: &str, agent: &str, model: &Value) -> Value {
    json!({"id": id, "projectID": "prj_test", "agent": agent, "model": model, "cost": 0,
           "tokens": {"input": 0, "output": 0, "reasoning": 0, "cache": {"read": 0, "write": 0}},
           "time": {"created": 1_700_000_000_000u64, "updated": 1_700_000_001_000u64}, "title": title,
           "location": {"directory": directory}})
}

pub(super) fn fork(directory: &str) -> Value {
    let mut info = session(
        FORK,
        directory,
        "Fork",
        "build",
        &json!({"providerID": "anthropic", "id": "claude-sonnet-4"}),
    );
    info["fork"] = json!({"sessionID": SESSION, "boundary": {"type": "through", "messageID": "msg_a2"}});
    json!({"data": info})
}

/// The two pages of a resumed session's messages: a question and an answer with a command, then the turn's end.
pub(super) fn messages(cursor: Option<&str>) -> Value {
    if cursor == Some("page2") {
        return json!({"data": [{"id": "msg_h4", "type": "idle", "time": {"created": 1_700_000_004_000u64},
                                "outcome": "interrupted"}],
                      "cursor": {"previous": "page1", "next": null}});
    }
    json!({"data": [
        {"id": "msg_h1", "type": "user", "time": {"created": 1_700_000_001_000u64}, "text": "Earlier question"},
        {"id": "msg_h2", "type": "assistant", "time": {"created": 1_700_000_002_000u64, "completed": 1_700_000_003_000u64},
         "agent": "build", "model": {"providerID": "anthropic", "id": "claude-sonnet-4"},
         "content": [
            {"type": "reasoning", "text": "Thinking back.",
             "time": {"created": 1_700_000_002_000u64, "completed": 1_700_000_002_500u64}},
            {"type": "tool", "id": "call_h", "name": "shell", "state": {"status": "completed",
             "input": {"command": "pwd"}, "content": [{"type": "text", "text": "/work"}]},
             "time": {"created": 1_700_000_002_600u64, "completed": 1_700_000_002_700u64}},
            {"type": "text", "text": "Earlier answer"}]}],
        "cursor": {"previous": null, "next": "page2"}})
}

/// The permission a resumed session `id` still waits for.
pub(super) fn waiting(location: &Value, id: &str) -> Value {
    json!({"location": location, "data": [
        {"id": PERMISSION, "sessionID": id, "action": "edit", "resources": ["b.txt"], "save": ["*"]},
        {"id": "per_other", "sessionID": "ses_someone_else", "action": "shell", "resources": ["rm -rf /"]}]})
}

pub(super) fn mcp(location: &Value) -> Value {
    json!({"location": location, "data": [
        {"name": "docs", "status": {"status": "needs_auth", "error": "Sign in to docs"}},
        {"name": "files", "title": "Files", "status": {"status": "connected"}}]})
}

pub(super) fn credentials() -> Value {
    json!({"data": [
        {"id": "cred_1", "integrationID": "anthropic", "label": "Work", "active": true,
         "value": {"type": "key", "key": "sk-test-secret"}},
        {"id": "cred_2", "integrationID": "openai", "label": "ChatGPT", "active": false,
         "value": {"type": "oauth", "methodID": "chatgpt", "refresh": "r-secret", "access": "a-secret", "expires": 1}}]})
}

pub(super) fn limits(location: &Value) -> Value {
    let meter =
        |percent: u64, limited: bool| json!({"percent": percent, "limited": limited, "resetsAt": 2_000_000_000_000u64});
    json!({"location": location, "data": {"workspace": {"id": "wrk_1", "usageURL": "https://opencode.example.test/usage"},
        "budgets": [], "go": {"rolling": meter(85, false), "weekly": meter(10, false), "monthly": meter(30, false)}}})
}
