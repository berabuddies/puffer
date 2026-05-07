//! Tiny redaction helper.
//!
//! Tools and prompts can leak secrets (`Read /etc/passwd`, env-var
//! contents, OAuth refresh tokens accidentally pasted into a chat).
//!
//! Codex review note: the default policy is **redact everything**.
//! Callers opt in per-axis via the `include_prompts` /
//! `include_outputs` / `include_tool_io` flags on
//! [`ObservabilityConfig`]. Even when the flag is on, individual tool
//! ids on the `always_redact_tool_ids` list are still redacted.

use sha2::{Digest, Sha256};

/// Per-attribute size cap regardless of policy. Langfuse and most
/// OTLP collectors silently drop attributes larger than ~64KB; we
/// trim aggressively to keep traces snappy in the UI.
pub const MAX_ATTR_BYTES: usize = 4096;

/// What kind of content is being recorded — drives which include flag
/// gates it.
#[derive(Debug, Clone)]
pub enum ContentKind {
    Prompt,
    Output,
    ToolInput {
        tool_id: String,
    },
    ToolOutput {
        tool_id: String,
    },
    /// Prompt that embeds rendered tool I/O (e.g. the reflection
    /// judge's input prompt or a spawned-agent root prompt). Requires
    /// BOTH `include_prompts` AND `include_tool_io` to pass through;
    /// otherwise emits the `[redacted: N bytes, sha256:...]` summary.
    PromptWithEmbeddedToolIo,
    /// Output that may carry tool I/O verbatim (e.g. a compaction
    /// summary that condenses prior tool calls + outputs). Requires
    /// BOTH `include_outputs` AND `include_tool_io` to pass through.
    OutputWithEmbeddedToolIo,
}

/// Per-`ObservabilityHandle` redaction policy: three opt-in flags
/// (default off) plus a denylist of tool ids whose I/O is always
/// redacted regardless of `include_tool_io`.
#[derive(Debug, Clone)]
pub struct RedactionPolicy {
    include_prompts: bool,
    include_outputs: bool,
    include_tool_io: bool,
    always_redact_tool_ids: Vec<String>,
}

impl RedactionPolicy {
    /// Build a policy from explicit flag values + a denylist of tool
    /// ids whose I/O is always redacted regardless of `include_tool_io`.
    pub fn from_config(
        include_prompts: bool,
        include_outputs: bool,
        include_tool_io: bool,
        always_redact_tool_ids: Vec<String>,
    ) -> Self {
        Self {
            include_prompts,
            include_outputs,
            include_tool_io,
            always_redact_tool_ids,
        }
    }

    /// Whether the policy permits raw user-prompt content on spans.
    pub fn include_prompts(&self) -> bool {
        self.include_prompts
    }

    /// Whether the policy permits raw assistant-output content on spans.
    pub fn include_outputs(&self) -> bool {
        self.include_outputs
    }

    /// Whether the policy permits raw tool input/output content on spans.
    pub fn include_tool_io(&self) -> bool {
        self.include_tool_io
    }

    /// Tool ids whose I/O is always redacted regardless of the
    /// `include_tool_io` flag (e.g. credential-shaped tools).
    pub fn always_redact_tool_ids(&self) -> &[String] {
        &self.always_redact_tool_ids
    }

    /// Convenience: redact-everything policy used in tests.
    pub fn redact_all() -> Self {
        Self {
            include_prompts: false,
            include_outputs: false,
            include_tool_io: false,
            always_redact_tool_ids: Vec::new(),
        }
    }

    /// Convenience: pass-through policy used in tests.
    pub fn pass_through() -> Self {
        Self {
            include_prompts: true,
            include_outputs: true,
            include_tool_io: true,
            always_redact_tool_ids: Vec::new(),
        }
    }

    /// Apply the policy to `value`, returning the string that should
    /// land on the span. Always emits the lengths so consumers can
    /// reason about size even when content is redacted.
    pub fn apply(&self, kind: ContentKind, value: &str) -> String {
        let included = match kind {
            ContentKind::Prompt => self.include_prompts,
            ContentKind::Output => self.include_outputs,
            ContentKind::PromptWithEmbeddedToolIo => self.include_prompts && self.include_tool_io,
            ContentKind::OutputWithEmbeddedToolIo => self.include_outputs && self.include_tool_io,
            ContentKind::ToolInput { tool_id } | ContentKind::ToolOutput { tool_id } => {
                if self
                    .always_redact_tool_ids
                    .iter()
                    .any(|id| id.eq_ignore_ascii_case(&tool_id))
                {
                    false
                } else {
                    self.include_tool_io
                }
            }
        };
        if included {
            truncate(value, MAX_ATTR_BYTES)
        } else {
            redact_summary(value)
        }
    }
}

/// Free-standing convenience: same effect as [`RedactionPolicy::apply`]
/// for callers that want a one-off redaction without holding a policy.
pub fn redact(policy: &RedactionPolicy, kind: ContentKind, value: &str) -> String {
    policy.apply(kind, value)
}

fn truncate(value: &str, cap: usize) -> String {
    if value.len() <= cap {
        return value.to_string();
    }
    // Find the largest UTF-8-safe boundary ≤ cap.
    let mut idx = cap.min(value.len());
    while idx > 0 && !value.is_char_boundary(idx) {
        idx -= 1;
    }
    let head = &value[..idx];
    format!(
        "{head}\n[truncated: kept {idx} of {total} bytes]",
        head = head,
        idx = idx,
        total = value.len()
    )
}

fn redact_summary(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let prefix = digest
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("[redacted: {} bytes, sha256:{}]", value.len(), prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pass_through_keeps_short_values_verbatim() {
        let policy = RedactionPolicy::pass_through();
        assert_eq!(policy.apply(ContentKind::Prompt, "hi there"), "hi there");
    }

    #[test]
    fn pass_through_truncates_at_cap() {
        let policy = RedactionPolicy::pass_through();
        let big = "a".repeat(MAX_ATTR_BYTES * 2);
        let out = policy.apply(ContentKind::Output, &big);
        assert!(out.len() < big.len());
        assert!(out.contains("[truncated:"));
    }

    #[test]
    fn redact_returns_only_metadata() {
        let policy = RedactionPolicy::redact_all();
        let out = policy.apply(ContentKind::Prompt, "secret-token-deadbeef");
        assert!(out.contains("[redacted:"));
        assert!(out.contains("sha256:"));
        assert!(!out.contains("secret-token"));
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        let policy = RedactionPolicy::pass_through();
        let big = "💖".repeat(MAX_ATTR_BYTES);
        let out = policy.apply(ContentKind::Output, &big);
        let _ = out.chars().last();
        assert!(out.contains("[truncated:"));
    }

    #[test]
    fn split_flags_let_prompts_in_but_redact_tool_output() {
        let policy = RedactionPolicy::from_config(
            /* prompts */ true,
            /* outputs */ false,
            /* tool_io */ false,
            Vec::new(),
        );
        assert_eq!(
            policy.apply(ContentKind::Prompt, "user said hi"),
            "user said hi"
        );
        assert!(policy
            .apply(
                ContentKind::ToolOutput {
                    tool_id: "Read".to_string(),
                },
                "/etc/passwd contents",
            )
            .contains("[redacted:"));
        assert!(policy
            .apply(ContentKind::Output, "model said hi back")
            .contains("[redacted:"));
    }

    #[test]
    fn always_redact_list_overrides_include_tool_io() {
        let policy = RedactionPolicy::from_config(
            /* prompts */ true,
            /* outputs */ true,
            /* tool_io */ true,
            vec!["AuthSetApiKey".to_string()],
        );
        let out = policy.apply(
            ContentKind::ToolInput {
                tool_id: "AuthSetApiKey".to_string(),
            },
            "sk-deadbeef-12345",
        );
        assert!(out.contains("[redacted:"));
        assert!(!out.contains("sk-deadbeef"));
        // Other tools still pass through.
        assert_eq!(
            policy.apply(
                ContentKind::ToolInput {
                    tool_id: "Read".to_string()
                },
                "fixture.txt",
            ),
            "fixture.txt"
        );
    }
}
