//! The context window: how full it is and what fills it.

use std::time::SystemTime;

use term_thread::{ContextBreakdown, ContextGroup, ContextKind, ContextPart, ContextRow, Update, UsagePatch};

use super::Mapper;
use crate::control::{CategoryKind, ContextCount, ContextUsage};

impl Mapper {
    /// The updates for the context usage that `get_context_usage` returned at `now`, counted as `count` says.
    pub(crate) fn context(&self, usage: &ContextUsage, count: ContextCount, now: SystemTime) -> Vec<Update> {
        vec![Update::Usage(UsagePatch {
            context_used: Some(usage.total_tokens),
            context_size: (usage.raw_max_tokens > 0).then_some(usage.raw_max_tokens),
            breakdown: Some(breakdown(usage, count == ContextCount::Exact, now)),
            ..UsagePatch::default()
        })]
    }
}

/// The thread's view of a `get_context_usage` answer.
fn breakdown(usage: &ContextUsage, exact: bool, now: SystemTime) -> ContextBreakdown {
    let rows = usage
        .categories
        .iter()
        .map(|c| ContextRow {
            id: c.id.clone(),
            name: c.name.clone(),
            tokens: c.tokens,
            kind: match c.kind {
                CategoryKind::Free => ContextKind::Free,
                CategoryKind::Buffer => ContextKind::Buffer,
                CategoryKind::Deferred => ContextKind::Deferred,
                CategoryKind::Used => ContextKind::Used,
            },
        })
        .collect();
    let threshold = usage
        .auto_compact_threshold
        .filter(|t| usage.is_auto_compact_enabled && t.is_finite() && *t > 0.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "a positive token count far below 2^53"
    )]
    let auto_compact_at = threshold.map(|t| {
        if t <= 1.0 {
            (t * usage.raw_max_tokens as f64).round() as u64
        } else {
            t.round() as u64
        }
    });
    ContextBreakdown {
        rows,
        used: usage.total_tokens,
        size: usage.raw_max_tokens,
        auto_compact_at,
        model: usage.model.clone(),
        exact,
        at: now,
        parts: context_parts(usage),
    }
}

/// What the rows of a `get_context_usage` answer consist of, by sort.
fn context_parts(usage: &ContextUsage) -> Vec<ContextPart> {
    let part = |group, name: &str, tokens| ContextPart {
        group,
        name: name.to_string(),
        tokens,
    };
    let mut parts: Vec<ContextPart> = Vec::new();
    parts.extend(
        usage
            .memory_files
            .iter()
            .map(|f| part(ContextGroup::MemoryFile, &f.path, f.tokens)),
    );
    parts.extend(usage.mcp_tools.iter().map(|t| {
        part(
            ContextGroup::McpTool,
            &format!("{}: {}", t.server_name, t.name),
            t.tokens,
        )
    }));
    parts.extend(
        usage
            .agents
            .iter()
            .map(|a| part(ContextGroup::Agent, &a.agent_type, a.tokens)),
    );
    if let Some(skills) = &usage.skills {
        parts.extend(
            skills
                .skill_frontmatter
                .iter()
                .map(|s| part(ContextGroup::Skill, &s.name, s.tokens)),
        );
    }
    parts.extend(
        usage
            .system_tools
            .iter()
            .flatten()
            .map(|t| part(ContextGroup::SystemTool, &t.name, t.tokens)),
    );
    if let Some(m) = &usage.message_breakdown {
        for (name, tokens) in [
            ("tool_calls", m.tool_call_tokens),
            ("tool_results", m.tool_result_tokens),
            ("attachments", m.attachment_tokens),
            ("assistant_messages", m.assistant_message_tokens),
            ("user_messages", m.user_message_tokens),
        ] {
            if tokens > 0 {
                parts.push(part(ContextGroup::Message, name, tokens));
            }
        }
    }
    parts
}

pub(super) fn context_used(tokens: u64) -> Update {
    Update::Usage(UsagePatch {
        context_used: Some(tokens),
        ..UsagePatch::default()
    })
}
