//! In-place tool-result content clearing (microcompact).
//!
//! Mirrors Claude Code's `services/compact/microCompact.ts` design: when the
//! conversation has accumulated many old tool results, content-clear all but
//! the most recent N for a configurable set of "compactable" tools while
//! preserving every `tool_use_id` so the model can still reference past calls.
//!
//! Triggers (matched against Claude Code v2.1.x):
//!   1. **Time-based** — gap since last assistant > `gap_threshold_minutes`
//!      (default 60min: Anthropic prompt-cache TTL is 1h, so the cache is
//!      cold anyway).
//!   2. **Token-budget** — input_tokens approaching the model's effective
//!      context window (default 200k - 13k buffer).
//!
//! Pruning preserves: the most recent `keep_recent` (default 5) tool results
//! whose tool name is in the compactable set, plus every non-compactable tool
//! result (TaskCreate, Memory, Skill, AgentTool, MCP, …) regardless of age.
//!
//! Pruning replaces only the `output.text` content with `CLEARED_STUB` —
//! `call_id`, tool ordering, and any preceding `FunctionCall` are untouched.

use super::openai::conversation::{ConversationItem, ToolOutputPayload};
use crate::tool_names::canonical_tool_name;
use std::collections::{HashMap, HashSet};
use std::time::SystemTime;

/// Tools whose output is bulky and rarely re-read by the model after a few
/// turns, expressed as **canonical names** (see `tool_names::canonical_tool_name`).
/// Aligned with Claude Code `COMPACTABLE_TOOLS` in
/// `services/compact/microCompact.ts:43-52`. Notably absent: `agent` (Task),
/// MCP tools, and any task/skill/memory tool — their results carry
/// semantic state the model needs to keep.
///
/// Membership is checked AFTER routing the wire-side tool name through
/// `canonical_tool_name(...)`, so this list does NOT need to enumerate
/// case variants or legacy aliases (`read_file`, `replace_in_file`, …);
/// the canonicalizer collapses them all to lowercase canonical form.
pub const COMPACTABLE_TOOLS: &[&str] = &[
    "read",
    "bash",
    "grep",
    "glob",
    "websearch",
    "webfetch",
    "edit",
    "write",
    "powershell",
];

/// Replacement text for cleared `tool_result` payloads. Matches the constant
/// `TIME_BASED_MC_CLEARED_MESSAGE` in Claude Code microCompact.ts:38.
pub const CLEARED_STUB: &str = "[Old tool result content cleared]";

/// Default keep-window: how many most-recent compactable tool results stay
/// uncleared. Floor at 1 — clearing every result leaves the model blind on
/// the current task.
pub const DEFAULT_KEEP_RECENT: usize = 5;

/// Default gap (minutes since last assistant) at which time-based pruning
/// fires. Matches Claude Code `TIME_BASED_MC_CONFIG_DEFAULTS.gapThresholdMinutes`.
pub const DEFAULT_GAP_THRESHOLD_MINUTES: u64 = 60;

/// Tokens reserved before the model's hard ceiling for
/// auto-compact / summary headroom. Matches Claude Code
/// `AUTOCOMPACT_BUFFER_TOKENS = 13_000`.
pub const AUTOCOMPACT_BUFFER_TOKENS: u32 = 13_000;

#[derive(Debug, Clone, Copy)]
pub struct MicrocompactConfig {
    pub keep_recent: usize,
    pub gap_threshold_minutes: u64,
    pub token_threshold: Option<u32>,
}

impl Default for MicrocompactConfig {
    fn default() -> Self {
        Self {
            keep_recent: DEFAULT_KEEP_RECENT,
            gap_threshold_minutes: DEFAULT_GAP_THRESHOLD_MINUTES,
            token_threshold: None,
        }
    }
}

impl MicrocompactConfig {
    /// Loads config from environment variables. Returns `None` when
    /// microcompact is disabled (default — opt-in to match Claude Code's
    /// `tengu_slate_heron` GrowthBook default of `enabled: false`).
    ///
    /// Variables:
    /// - `PUFFER_MICROCOMPACT=1` — enables the pass.
    /// - `PUFFER_MICROCOMPACT_KEEP_RECENT=N` — overrides default 5.
    /// - `PUFFER_MICROCOMPACT_GAP_MINUTES=N` — overrides default 60.
    /// - `PUFFER_MICROCOMPACT_TOKEN_THRESHOLD=N` — adds a token-budget
    ///   trigger; without this only the time-gap trigger fires.
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("PUFFER_MICROCOMPACT")
            .ok()
            .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
        if !enabled {
            return None;
        }
        let mut cfg = Self::default();
        if let Ok(s) = std::env::var("PUFFER_MICROCOMPACT_KEEP_RECENT") {
            if let Ok(n) = s.parse::<usize>() {
                cfg.keep_recent = n;
            }
        }
        if let Ok(s) = std::env::var("PUFFER_MICROCOMPACT_GAP_MINUTES") {
            if let Ok(n) = s.parse::<u64>() {
                cfg.gap_threshold_minutes = n;
            }
        }
        if let Ok(s) = std::env::var("PUFFER_MICROCOMPACT_TOKEN_THRESHOLD") {
            if let Ok(n) = s.parse::<u32>() {
                cfg.token_threshold = Some(n);
            }
        }
        Some(cfg)
    }
}

#[derive(Debug, Clone)]
pub struct MicrocompactOutcome {
    pub trigger: MicrocompactTrigger,
    pub cleared_count: usize,
    pub tokens_saved: usize,
    /// Tool call_ids whose output was cleared, in encounter order.
    pub cleared_call_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrocompactTrigger {
    TimeGap,
    TokenBudget,
}

/// Inputs the caller already has on hand when assembling a request.
pub struct MicrocompactInputs<'a> {
    pub last_assistant_at: Option<SystemTime>,
    pub last_input_tokens: Option<u32>,
    pub config: &'a MicrocompactConfig,
}

/// Returns true when a wire-side tool name (any case / alias) maps to one
/// of the compactable canonical names. Centralizes the
/// `canonical_tool_name` indirection so both passes stay in sync.
fn is_compactable_tool(wire_name: &str) -> bool {
    let canonical = canonical_tool_name(wire_name);
    COMPACTABLE_TOOLS.contains(&canonical.as_str())
}

/// Walk `items` in encounter order and collect tool_use ids whose
/// originating `FunctionCall.name`, after canonical normalization,
/// matches `COMPACTABLE_TOOLS`.
fn collect_compactable_call_ids(items: &[ConversationItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            ConversationItem::FunctionCall { call_id, name, .. } if is_compactable_tool(name) => {
                Some(call_id.clone())
            }
            _ => None,
        })
        .collect()
}

/// Decide whether either trigger fires. Returns `None` to skip pruning.
fn evaluate_trigger(inputs: &MicrocompactInputs<'_>) -> Option<MicrocompactTrigger> {
    if let Some(at) = inputs.last_assistant_at {
        if let Ok(elapsed) = SystemTime::now().duration_since(at) {
            if elapsed.as_secs() / 60 >= inputs.config.gap_threshold_minutes {
                return Some(MicrocompactTrigger::TimeGap);
            }
        }
    }
    if let (Some(tokens), Some(threshold)) =
        (inputs.last_input_tokens, inputs.config.token_threshold)
    {
        if tokens >= threshold {
            return Some(MicrocompactTrigger::TokenBudget);
        }
    }
    None
}

/// In-place clear of stale tool_result content. Returns `None` when no
/// trigger fires or there's nothing to clear.
///
/// Caller is responsible for inserting a boundary message after a successful
/// pass (mirrors Claude Code `createMicrocompactBoundaryMessage`). We don't
/// do it here so callers can format it however the surrounding pipeline
/// already formats system reminders.
pub fn microcompact_in_place(
    items: &mut [ConversationItem],
    inputs: &MicrocompactInputs<'_>,
) -> Option<MicrocompactOutcome> {
    let trigger = evaluate_trigger(inputs)?;
    let compactable_ids = collect_compactable_call_ids(items);
    if compactable_ids.is_empty() {
        return None;
    }
    let keep_recent = inputs.config.keep_recent.max(1);
    let keep_set: HashSet<String> = compactable_ids
        .iter()
        .rev()
        .take(keep_recent)
        .cloned()
        .collect();

    // Map call_id → tool name (so the boundary message can summarize what we
    // dropped, and so we never accidentally clear a non-compactable output
    // that happens to share an id pattern). Owned strings so we can mutate
    // `items` afterward.
    let name_by_call: HashMap<String, String> = items
        .iter()
        .filter_map(|item| match item {
            ConversationItem::FunctionCall { call_id, name, .. } => {
                Some((call_id.clone(), name.clone()))
            }
            _ => None,
        })
        .collect();

    let mut tokens_saved = 0usize;
    let mut cleared_call_ids = Vec::new();

    for item in items.iter_mut() {
        let ConversationItem::FunctionCallOutput { call_id, output } = item else {
            continue;
        };
        if keep_set.contains(call_id) {
            continue;
        }
        let Some(tool_name) = name_by_call.get(call_id) else {
            continue;
        };
        if !is_compactable_tool(tool_name) {
            continue;
        }
        // CLEARED_STUB is a sentinel, not a marker prefix — only an exact
        // match means we already cleared this slot. `starts_with`/`contains`
        // would falsely skip outputs that legitimately quote the stub
        // (e.g. a Read of this file's source code).
        if output.text == CLEARED_STUB {
            continue;
        }
        // Reconstruct estimated tokens for the cleared content. Conservative:
        // use the same heuristic as ConversationItem::estimated_tokens.
        tokens_saved += rough_token_estimate(&output.text);
        cleared_call_ids.push(call_id.clone());
        *output = ToolOutputPayload {
            text: CLEARED_STUB.to_string(),
            is_error: output.is_error,
        };
    }

    if cleared_call_ids.is_empty() {
        return None;
    }

    Some(MicrocompactOutcome {
        trigger,
        cleared_count: cleared_call_ids.len(),
        tokens_saved,
        cleared_call_ids,
    })
}

/// Format a boundary system message announcing the prune. Mirrors
/// `createMicrocompactBoundaryMessage` from Claude Code utils/messages.ts:4557.
pub fn boundary_message(outcome: &MicrocompactOutcome) -> String {
    format!(
        "[microcompact] cleared {} stale tool result{} (~{} tokens reclaimed). \
         Their call ids remain in context; ask the model to re-run the tool if you need the content.",
        outcome.cleared_count,
        if outcome.cleared_count == 1 { "" } else { "s" },
        outcome.tokens_saved,
    )
}

/// Reuses ConversationItem's CJK-aware estimator without introducing a
/// dependency on its private helper.
fn rough_token_estimate(text: &str) -> usize {
    let mut units = 0usize;
    for ch in text.chars() {
        if ch.is_ascii() {
            units += 1;
        } else {
            units += 6;
        }
    }
    (units + 3) / 4
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::openai::conversation::{ContentPart, ConversationItem, ToolOutputPayload};
    use std::time::Duration;

    fn fc(call_id: &str, name: &str) -> ConversationItem {
        ConversationItem::FunctionCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: "{}".to_string(),
        }
    }
    fn fco(call_id: &str, body: &str) -> ConversationItem {
        ConversationItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: ToolOutputPayload::success(body.to_string()),
        }
    }
    fn user_msg(text: &str) -> ConversationItem {
        ConversationItem::Message {
            role: "user".to_string(),
            content: vec![ContentPart::Text {
                text: text.to_string(),
            }],
        }
    }

    fn an_hour_ago() -> SystemTime {
        SystemTime::now() - Duration::from_secs(3600 + 60)
    }

    #[test]
    fn time_gap_clears_old_compactable_outputs_keeps_last_n() {
        let mut items = vec![
            user_msg("first ask"),
            fc("c1", "Read"),
            fco("c1", "huge file dump 1"),
            fc("c2", "Read"),
            fco("c2", "huge file dump 2"),
            fc("c3", "Bash"),
            fco("c3", "ls -la output"),
            fc("c4", "Grep"),
            fco("c4", "grep results"),
            fc("c5", "Glob"),
            fco("c5", "glob results"),
            fc("c6", "Read"),
            fco("c6", "RECENT — must keep"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 2,
            gap_threshold_minutes: 60,
            token_threshold: None,
        };
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: Some(an_hour_ago()),
                last_input_tokens: None,
                config: &cfg,
            },
        )
        .expect("trigger should fire");

        assert_eq!(outcome.trigger, MicrocompactTrigger::TimeGap);
        assert_eq!(outcome.cleared_count, 4); // c1..c4 cleared, c5/c6 kept
                                              // last two compactable outputs stay verbatim
        let kept_texts: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                ConversationItem::FunctionCallOutput { call_id, output } => {
                    Some((call_id.as_str(), output.text.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(kept_texts[0], ("c1", CLEARED_STUB));
        assert_eq!(kept_texts[1], ("c2", CLEARED_STUB));
        assert_eq!(kept_texts[2], ("c3", CLEARED_STUB));
        assert_eq!(kept_texts[3], ("c4", CLEARED_STUB));
        assert_eq!(kept_texts[4], ("c5", "glob results"));
        assert_eq!(kept_texts[5], ("c6", "RECENT — must keep"));
    }

    #[test]
    fn non_compactable_tools_never_cleared() {
        let mut items = vec![
            fc("m1", "Memory"),
            fco("m1", "memory write payload"),
            fc("t1", "TaskCreate"),
            fco("t1", "task created details"),
            fc("a1", "Agent"),
            fco("a1", "agent delegation result"),
            fc("r1", "Read"),
            fco("r1", "first read"),
            fc("r2", "Read"),
            fco("r2", "RECENT — must keep"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 1,
            ..Default::default()
        };
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: Some(an_hour_ago()),
                last_input_tokens: None,
                config: &cfg,
            },
        )
        .expect("Read still compactable");

        assert_eq!(outcome.cleared_count, 1, "only the older Read clears");
        let texts: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                ConversationItem::FunctionCallOutput { call_id, output } => {
                    Some((call_id.as_str(), output.text.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts[0], ("m1", "memory write payload"));
        assert_eq!(texts[1], ("t1", "task created details"));
        assert_eq!(texts[2], ("a1", "agent delegation result"));
        assert_eq!(texts[3], ("r1", CLEARED_STUB));
        assert_eq!(texts[4], ("r2", "RECENT — must keep"));
    }

    #[test]
    fn recent_assistant_skips_pruning() {
        let mut items = vec![fc("c1", "Read"), fco("c1", "stale but still recent")];
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: Some(SystemTime::now()),
                last_input_tokens: None,
                config: &MicrocompactConfig::default(),
            },
        );
        assert!(outcome.is_none());
    }

    #[test]
    fn token_threshold_triggers_when_above_budget() {
        let mut items = vec![
            fc("c1", "Read"),
            fco("c1", "old"),
            fc("c2", "Read"),
            fco("c2", "newer"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 1,
            token_threshold: Some(100),
            ..Default::default()
        };
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: None,
                last_input_tokens: Some(150),
                config: &cfg,
            },
        )
        .expect("over budget");
        assert_eq!(outcome.trigger, MicrocompactTrigger::TokenBudget);
        assert_eq!(outcome.cleared_count, 1);
    }

    #[test]
    fn idempotent_does_not_double_count() {
        let mut items = vec![
            fc("c1", "Read"),
            fco("c1", "huge"),
            fc("c2", "Read"),
            fco("c2", "huge"),
            fc("c3", "Read"),
            fco("c3", "RECENT"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 1,
            ..Default::default()
        };
        let inputs = MicrocompactInputs {
            last_assistant_at: Some(an_hour_ago()),
            last_input_tokens: None,
            config: &cfg,
        };
        let first = microcompact_in_place(&mut items, &inputs).unwrap();
        assert_eq!(first.cleared_count, 2);
        let second = microcompact_in_place(&mut items, &inputs);
        assert!(
            second.is_none(),
            "second pass with all stale entries already CLEARED_STUB should be a no-op"
        );
    }

    #[test]
    fn canonical_normalization_handles_alias_and_lowercase_tool_names() {
        // The four equivalent ways a tool name can show up on the wire:
        //   - canonical lowercase ("read")
        //   - TitleCase ("Read")
        //   - legacy snake_case alias ("read_file")
        //   - mixed-case alias ("Read_File")
        // All four MUST be treated as compactable. Before the refactor the
        // first and the last would silently no-op because the whitelist
        // only contained TitleCase + a fixed alias list.
        let mut items = vec![
            fc("c1", "read"),
            fco("c1", "lowercase"),
            fc("c2", "Read"),
            fco("c2", "titlecase"),
            fc("c3", "read_file"),
            fco("c3", "snake_alias"),
            fc("c4", "Read_File"),
            fco("c4", "mixed_alias_RECENT"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 1,
            ..Default::default()
        };
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: Some(an_hour_ago()),
                last_input_tokens: None,
                config: &cfg,
            },
        )
        .expect("trigger");
        assert_eq!(outcome.cleared_count, 3, "first three Read variants clear");
        let texts: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                ConversationItem::FunctionCallOutput { call_id, output } => {
                    Some((call_id.as_str(), output.text.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(texts[0].1, CLEARED_STUB);
        assert_eq!(texts[1].1, CLEARED_STUB);
        assert_eq!(texts[2].1, CLEARED_STUB);
        assert_eq!(texts[3].1, "mixed_alias_RECENT");
    }

    #[test]
    fn cleared_stub_substring_in_payload_does_not_skip_first_clear() {
        // The CLEARED_STUB sentinel is a Rust constant in this very source
        // file. If the model `Read`s `microcompact.rs`, the resulting
        // tool_result will contain the stub as a SUBSTRING. Strict equality
        // (`==`) is what makes the first-pass clear still fire — switching
        // to `starts_with`/`contains` would break this. Lock the invariant.
        let payload_with_substring = format!(
            "fn foo() {{\n  pub const CLEARED_STUB: &str = \"{}\";\n}}\n",
            CLEARED_STUB
        );
        let mut items = vec![
            fc("c1", "Read"),
            fco("c1", &payload_with_substring),
            fc("c2", "Read"),
            fco("c2", "RECENT"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 1,
            ..Default::default()
        };
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: Some(an_hour_ago()),
                last_input_tokens: None,
                config: &cfg,
            },
        )
        .expect("trigger");
        assert_eq!(outcome.cleared_count, 1);
        // After clearing, the FCO is exactly the stub. A second pass must
        // detect that and skip (the existing idempotence test covers the
        // mechanic; this one covers the substring-collision path).
        let cleared_text = items
            .iter()
            .find_map(|i| match i {
                ConversationItem::FunctionCallOutput { call_id, output } if call_id == "c1" => {
                    Some(output.text.clone())
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(cleared_text, CLEARED_STUB, "exact stub, not substring");
    }

    #[test]
    fn keep_recent_floors_at_one() {
        let mut items = vec![
            fc("c1", "Read"),
            fco("c1", "first"),
            fc("c2", "Read"),
            fco("c2", "MOST_RECENT"),
        ];
        let cfg = MicrocompactConfig {
            keep_recent: 0,
            ..Default::default()
        };
        let outcome = microcompact_in_place(
            &mut items,
            &MicrocompactInputs {
                last_assistant_at: Some(an_hour_ago()),
                last_input_tokens: None,
                config: &cfg,
            },
        )
        .expect("trigger");
        assert_eq!(outcome.cleared_count, 1);
        let last = items
            .iter()
            .rev()
            .find_map(|i| match i {
                ConversationItem::FunctionCallOutput { output, .. } => Some(output.text.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(last, "MOST_RECENT", "must always keep the most recent");
    }
}
