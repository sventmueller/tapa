//! What `get_context_usage` answers: how full the context window is and what fills it.

use serde::Deserialize;

/// How `get_context_usage` counts the categories of the context window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextCount {
    /// Estimated from the last response, which costs nothing.
    Estimated,
    /// Each with the token-count API, which costs a call per category.
    Exact,
}

/// What `get_context_usage` answers.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ContextUsage {
    /// The rows of the breakdown.
    pub(crate) categories: Vec<ContextCategory>,
    /// Tokens in use.
    pub total_tokens: u64,
    /// The window measured against.
    pub(crate) raw_max_tokens: u64,
    /// The share in use, in percent.
    pub(crate) percentage: f64,
    pub(crate) model: String,
    /// Where compacting by itself starts, in tokens.
    pub(crate) auto_compact_threshold: Option<f64>,
    /// Whether the conversation is compacted by itself.
    pub(crate) is_auto_compact_enabled: bool,
    /// The memory files, such as `CLAUDE.md`.
    pub(crate) memory_files: Vec<ContextFile>,
    /// The tools of MCP servers.
    pub(crate) mcp_tools: Vec<ContextMcpTool>,
    /// The custom agents.
    pub(crate) agents: Vec<ContextAgent>,
    pub(crate) skills: Option<ContextSkills>,
    /// The built-in tools.
    pub(crate) system_tools: Option<Vec<ContextNamed>>,
    /// The conversation by kind of message.
    pub(crate) message_breakdown: Option<ContextMessages>,
}

/// A memory file in the context.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ContextFile {
    pub(crate) path: String,
    pub(crate) tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ContextMcpTool {
    pub(crate) name: String,
    pub(crate) server_name: String,
    pub(crate) tokens: u64,
}

/// A custom agent in the context.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ContextAgent {
    pub(crate) agent_type: String,
    pub(crate) tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ContextSkills {
    /// Their tokens together.
    pub(crate) tokens: u64,
    /// Each skill.
    pub(crate) skill_frontmatter: Vec<ContextNamed>,
}

/// Something in the context with a name.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ContextNamed {
    pub(crate) name: String,
    pub(crate) tokens: u64,
}

/// The conversation in the context by kind of message.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct ContextMessages {
    pub(crate) tool_call_tokens: u64,
    pub(crate) tool_result_tokens: u64,
    pub(crate) attachment_tokens: u64,
    pub(crate) assistant_message_tokens: u64,
    pub(crate) user_message_tokens: u64,
}

/// One row of the context breakdown.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ContextCategory {
    /// The stable id of the row, when the CLI sends one.
    pub(crate) id: Option<String>,
    /// The row's name as the CLI shows it.
    pub(crate) name: String,
    pub(crate) tokens: u64,
    pub(crate) kind: CategoryKind,
}

/// What a row of the context breakdown is, by the CLI's `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CategoryKind {
    /// The room left.
    Free,
    /// The reserve kept for compaction.
    Buffer,
    /// Tool definitions loaded only when needed, outside the window.
    Deferred,
    /// Content in the window, and any kind the CLI adds later.
    #[default]
    #[serde(other)]
    Used,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rows_kind_reads_from_the_clis_word_and_a_new_word_counts_as_used() {
        for (word, kind) in [
            ("used", CategoryKind::Used),
            ("free", CategoryKind::Free),
            ("buffer", CategoryKind::Buffer),
            ("deferred", CategoryKind::Deferred),
            ("reserved_later", CategoryKind::Used),
        ] {
            let row: ContextCategory =
                serde_json::from_value(serde_json::json!({"name": "n", "tokens": 1, "kind": word})).unwrap();
            assert_eq!(row.kind, kind, "{word}");
        }
        let without: ContextCategory = serde_json::from_value(serde_json::json!({"name": "n"})).unwrap();
        assert_eq!(without.kind, CategoryKind::Used);
    }
}
