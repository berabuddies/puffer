use super::{
    BatchAssessment, JudgeSignal, LlmJudgeContextScope, LlmJudgeMode, ReflectionLanguage,
    RECENT_ACTION_PREVIEW,
};
use crate::runtime::openai::conversation::{ContentPart, ConversationItem, ReasoningSummary};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fmt::Write as _;

#[derive(Debug, Clone, Deserialize)]
pub(super) struct LlmJudgeResponse {
    pub(super) decision: String,
    pub(super) reason: String,
    pub(super) next_action: String,
    pub(super) confidence: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LlmJudgeDecision {
    Continue,
    Reflect,
    Escalate,
}

pub(super) fn build_llm_judge_prompt(
    language: ReflectionLanguage,
    goal: &str,
    assessment: &BatchAssessment,
    code_signal: Option<&JudgeSignal>,
    context: &str,
    relevant_paths: &str,
) -> String {
    let signal_lines = assessment
        .signal_notes
        .iter()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let recent_actions = assessment
        .recent_actions
        .iter()
        .rev()
        .take(RECENT_ACTION_PREVIEW)
        .cloned()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|line| format!("- {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let code_line = code_signal
        .map(render_judge_lines)
        .unwrap_or_else(|| "- code_judge: not triggered".to_string());
    // When the agent has emitted a text-only terminal turn, the code-judge
    // path synthesises a claim-only observation with no real progress
    // markers and the `completion_claim` field carries the agent's final
    // answer. Surfacing it explicitly (instead of relying on the judge to
    // fish it out of the context preview) lets us ask the skeptical
    // question — "did they actually verify, or is this a confident
    // summary?" — in a way that doesn't get drowned out by the prior
    // tool trace.
    let completion_claim_section = match assessment.completion_claim.as_deref() {
        Some(text) if !text.trim().is_empty() => {
            let truncated = if text.chars().count() > 1200 {
                let cut: String = text.chars().take(1200).collect();
                format!("{cut}…(truncated)")
            } else {
                text.to_string()
            };
            match language {
                ReflectionLanguage::Chinese => format!(
                    "\nAgent 声明已完成（终局 turn，这是最后一次干预机会）：\n\"\"\"\n{truncated}\n\"\"\"\n\
质疑该声明时重点看：\n\
- 任务 prompt 里如果明确指出要运行某个命令/脚本/检查，agent 是否照做并得到通过？还是只跑了 agent 自己造的 smoke test？\n\
- 产物目录里有没有 agent 测试留下的临时产物（编译中间文件、备份文件等）— 严格的 file-listing 检查会挂。\n\
- agent 自己的验证断言是否覆盖了任务 requirement 里**全部**的项目（包括数值边界、格式保留、跨输入一致性）？\n\
- 自信的 \"Done!\" 不是证据；\"我跑了任务要求的那个检查并通过\" 才是证据。\n",
                ),
                ReflectionLanguage::English => format!(
                    "\nAgent claims completion (terminal turn — last chance to intervene):\n\"\"\"\n{truncated}\n\"\"\"\n\
When questioning the claim, focus on:\n\
- If the task prompt names a specific test/check/command to run, did the agent run it verbatim and confirm it passes? Or only a surrogate smoke test they wrote themselves?\n\
- Are there leftover build/test artifacts in the output directory — strict file-listing checks in the verifier will fail.\n\
- Does the agent's self-verification cover **every** requirement item (numeric bounds, format preservation, cross-input consistency)?\n\
- A confident \"Done!\" is not evidence; \"I ran the check the task prompt asked for and it passed\" is.\n",
                ),
            }
        }
        _ => String::new(),
    };
    match language {
        ReflectionLanguage::Chinese => format!(
            "你是一个严格的运行时判断器，不要解决任务本身，只判断主 agent 现在是否应该进入反思。\n\
**极简推理。这是一个 gate 决策，不是分析题**。reasoning 最多 2-3 句（如果你的 reasoning 超过 500 token 说明你在过度思考——停下直接出 JSON）。\n\
只输出一个 JSON 对象，不要输出 Markdown 代码块，不要输出额外解释。\n\n\
目标：\n- {goal}\n\n\
启发式 judge 状态：\n{code_line}\n\n\
最近信号：\n{signal_lines}\n\n\
最近动作：\n{recent_actions}\n\n\
相关文件：\n{relevant_paths}\n\n\
当前上下文窗口：\n{context}\n{completion_claim_section}\n\
请判断主 agent 现在应该：continue / reflect / escalate。\n\
判断标准：\n\
- 如果明显在重复、停滞、偏航，输出 reflect。\n\
- 如果 agent 声明完成但验证证据不充分（没跑真 verifier 测试、留了测试产物、自测没覆盖 requirement），输出 reflect。\n\
- 如果只是正常推进中，输出 continue。\n\
- 如果核心信息缺失、上下文明显不足或需要用户输入才能继续，输出 escalate。\n\n\
输出 JSON schema：\n\
{{\"decision\":\"continue|reflect|escalate\",\"reason\":\"...\",\"next_action\":\"...\",\"confidence\":0.0}}"
        ),
        ReflectionLanguage::English => format!(
            "You are a strict runtime judge. Do not solve the task itself. Only decide whether the main agent should enter reflection now.\n\
**Minimize reasoning. This is a gate decision, not an analysis.** 2-3 short sentences of reasoning max (if yours runs past ~500 tokens you are overthinking — stop and emit the JSON).\n\
Return exactly one JSON object and nothing else.\n\n\
Goal:\n- {goal}\n\n\
Heuristic judge status:\n{code_line}\n\n\
Recent signals:\n{signal_lines}\n\n\
Recent actions:\n{recent_actions}\n\n\
Relevant files:\n{relevant_paths}\n\n\
Current context window:\n{context}\n{completion_claim_section}\n\
Decide whether the main agent should: continue / reflect / escalate.\n\
Criteria:\n\
- If it is clearly looping, stalled, or wandering, output reflect.\n\
- If the agent claims completion but the evidence is weak (no run of the real verifier tests, leftover test artifacts, self-tests that don't cover the full requirement list), output reflect.\n\
- If it is progressing normally, output continue.\n\
- If key information is missing or user input is required, output escalate.\n\n\
Output JSON schema:\n\
{{\"decision\":\"continue|reflect|escalate\",\"reason\":\"...\",\"next_action\":\"...\",\"confidence\":0.0}}"
        ),
    }
}

pub(super) fn render_judge_lines(signal: &JudgeSignal) -> String {
    let mut lines = vec![
        format!("- source: {}", signal.source),
        format!("- summary: {}", signal.summary),
        format!("- reason: {}", signal.reason),
    ];
    if let Some(next_action) = &signal.next_action {
        lines.push(format!("- next_action: {next_action}"));
    }
    lines.join("\n")
}

pub(super) fn render_relevant_paths(paths: &BTreeSet<String>) -> String {
    let rendered = paths
        .iter()
        .filter(|path| !super::support::is_runtime_path(path))
        .take(6)
        .map(|path| format!("- {path}"))
        .collect::<Vec<_>>()
        .join("\n");
    if rendered.is_empty() {
        "- <none>".to_string()
    } else {
        rendered
    }
}

pub(super) fn select_final_signal(
    llm_mode: Option<LlmJudgeMode>,
    code_signal: Option<JudgeSignal>,
    llm_signal: Option<Option<JudgeSignal>>,
) -> Option<JudgeSignal> {
    match llm_mode {
        Some(LlmJudgeMode::ConfirmCodeJudge) => match (code_signal, llm_signal) {
            (Some(_), Some(Some(llm))) => Some(llm),
            (Some(code), None) => Some(code),
            (Some(_), Some(None)) => None,
            (None, _) => None,
        },
        // Independent: the LLM decides authoritatively when it was reached.
        // - Some(Some(sig)) — LLM said REFLECT/ESCALATE, use it.
        // - Some(None)      — LLM said CONTINUE, suppress code signal.
        // - None            — LLM was unavailable / errored, fall back to code signal.
        _ => match llm_signal {
            Some(value) => value,
            None => code_signal,
        },
    }
}

pub(super) fn render_llm_judge_context(
    items: &[ConversationItem],
    scope: LlmJudgeContextScope,
    recent_item_count: usize,
    max_context_chars: usize,
    max_tool_output_chars: usize,
) -> String {
    let rendered = match scope {
        LlmJudgeContextScope::CurrentWindow => render_items(items, max_tool_output_chars),
        LlmJudgeContextScope::RecentWindow => {
            let start = items.len().saturating_sub(recent_item_count.max(1));
            let mut output = String::new();
            if start > 0 {
                let _ = writeln!(&mut output, "[{} older items omitted]", start);
            }
            output.push_str(&render_items(&items[start..], max_tool_output_chars));
            output
        }
        LlmJudgeContextScope::SummaryAndRecent => {
            let start = items.len().saturating_sub(recent_item_count.max(1));
            let older = &items[..start];
            let mut output = String::new();
            if !older.is_empty() {
                let _ = writeln!(
                    &mut output,
                    "[older context summary: {}]",
                    summarize_older_items(older)
                );
            }
            output.push_str(&render_items(&items[start..], max_tool_output_chars));
            output
        }
    };
    truncate_middle(&rendered, max_context_chars)
}

pub(super) fn parse_llm_judge_response(text: &str) -> Option<LlmJudgeResponse> {
    serde_json::from_str::<LlmJudgeResponse>(text.trim())
        .ok()
        .or_else(|| {
            let json = extract_json_object(text)?;
            serde_json::from_str::<LlmJudgeResponse>(&json).ok()
        })
}

pub(super) fn parse_llm_judge_decision(text: &str) -> Option<LlmJudgeDecision> {
    match text.trim().to_ascii_lowercase().as_str() {
        "continue" => Some(LlmJudgeDecision::Continue),
        "reflect" => Some(LlmJudgeDecision::Reflect),
        "escalate" => Some(LlmJudgeDecision::Escalate),
        _ => None,
    }
}

fn render_items(items: &[ConversationItem], max_tool_output_chars: usize) -> String {
    let mut rendered = String::new();
    for item in items {
        match item {
            ConversationItem::Message { role, content } => {
                let text = content
                    .iter()
                    .map(|part| match part {
                        ContentPart::Text { text } => text.as_str(),
                        ContentPart::Image { url } => url.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                let _ = writeln!(&mut rendered, "{}: {}", role.to_ascii_uppercase(), text);
            }
            ConversationItem::FunctionCall {
                call_id,
                name,
                arguments,
            } => {
                let _ = writeln!(
                    &mut rendered,
                    "TOOL_CALL {} [{}]: {}",
                    name, call_id, arguments
                );
            }
            ConversationItem::FunctionCallOutput { call_id, output } => {
                let _ = writeln!(
                    &mut rendered,
                    "TOOL_RESULT [{}] {}: {}",
                    call_id,
                    if output.is_error { "error" } else { "ok" },
                    truncate_middle(&output.text, max_tool_output_chars)
                );
            }
            ConversationItem::Reasoning {
                summary,
                encrypted_content: _,
            } => {
                let text = summary
                    .iter()
                    .map(|item| match item {
                        ReasoningSummary::SummaryText { text } => text.as_str(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                if !text.is_empty() {
                    let _ = writeln!(&mut rendered, "REASONING: {text}");
                }
            }
            ConversationItem::Compaction { summary } => {
                let _ = writeln!(&mut rendered, "COMPACTION: {summary}");
            }
        }
    }
    if rendered.trim().is_empty() {
        "<empty>".to_string()
    } else {
        rendered
    }
}

fn summarize_older_items(items: &[ConversationItem]) -> String {
    let mut user_messages = 0usize;
    let mut assistant_messages = 0usize;
    let mut tool_calls = 0usize;
    let mut tool_results = 0usize;
    let mut reasoning_items = 0usize;
    let mut compactions = 0usize;
    for item in items {
        match item {
            ConversationItem::Message { role, .. } if role == "user" => user_messages += 1,
            ConversationItem::Message { role, .. } if role == "assistant" => {
                assistant_messages += 1;
            }
            ConversationItem::Message { .. } => {}
            ConversationItem::FunctionCall { .. } => tool_calls += 1,
            ConversationItem::FunctionCallOutput { .. } => tool_results += 1,
            ConversationItem::Reasoning { .. } => reasoning_items += 1,
            ConversationItem::Compaction { .. } => compactions += 1,
        }
    }
    format!(
        "{} user, {} assistant, {} tool calls, {} tool results, {} reasoning, {} compactions",
        user_messages, assistant_messages, tool_calls, tool_results, reasoning_items, compactions
    )
}

fn extract_json_object(text: &str) -> Option<String> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].to_string())
}

fn truncate_middle(text: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    if max_chars <= 16 {
        return chars[..max_chars].iter().collect();
    }
    let head = max_chars / 2;
    let tail = max_chars.saturating_sub(head + 13);
    let prefix = chars[..head].iter().collect::<String>();
    let suffix = chars[chars.len().saturating_sub(tail)..]
        .iter()
        .collect::<String>();
    format!("{prefix}\n[...snip...]\n{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llm_response_parser_accepts_fenced_json() {
        let parsed = parse_llm_judge_response(
            "```json\n{\"decision\":\"reflect\",\"reason\":\"looping\",\"next_action\":\"inspect file\",\"confidence\":0.8}\n```",
        )
        .expect("json should parse");
        assert_eq!(parsed.decision, "reflect");
        assert_eq!(parsed.reason, "looping");
    }

    #[test]
    fn summary_and_recent_context_adds_older_summary() {
        let items = vec![
            ConversationItem::user_message("one"),
            ConversationItem::assistant_message("two"),
            ConversationItem::user_message("three"),
            ConversationItem::assistant_message("four"),
        ];
        let rendered = render_llm_judge_context(
            &items,
            LlmJudgeContextScope::SummaryAndRecent,
            2,
            2_000,
            200,
        );
        assert!(rendered.contains("older context summary"));
        assert!(rendered.contains("USER: three"));
        assert!(rendered.contains("ASSISTANT: four"));
    }

    #[test]
    fn confirm_mode_respects_llm_continue() {
        let code_signal = JudgeSignal {
            source: "code_judge",
            summary: "stalled".to_string(),
            reason: "looping".to_string(),
            next_action: None,
        };
        assert!(select_final_signal(
            Some(LlmJudgeMode::ConfirmCodeJudge),
            Some(code_signal),
            Some(None),
        )
        .is_none());
    }

    #[test]
    fn confirm_mode_falls_back_when_llm_is_unavailable() {
        let code_signal = JudgeSignal {
            source: "code_judge",
            summary: "stalled".to_string(),
            reason: "looping".to_string(),
            next_action: None,
        };
        let selected = select_final_signal(
            Some(LlmJudgeMode::ConfirmCodeJudge),
            Some(code_signal.clone()),
            None,
        )
        .expect("code judge should survive transport failure");
        assert_eq!(selected.source, code_signal.source);
    }

    #[test]
    fn independent_mode_honours_llm_continue_over_code_signal() {
        let code_signal = JudgeSignal {
            source: "code_judge",
            summary: "stalled".to_string(),
            reason: "looping".to_string(),
            next_action: None,
        };
        let selected = select_final_signal(
            Some(LlmJudgeMode::Independent),
            Some(code_signal),
            Some(None),
        );
        assert!(
            selected.is_none(),
            "independent mode should respect an explicit LLM CONTINUE"
        );
    }

    #[test]
    fn independent_mode_falls_back_to_code_when_llm_unreached() {
        let code_signal = JudgeSignal {
            source: "code_judge",
            summary: "stalled".to_string(),
            reason: "looping".to_string(),
            next_action: None,
        };
        let selected =
            select_final_signal(Some(LlmJudgeMode::Independent), Some(code_signal.clone()), None)
                .expect("independent mode should fall back to code judge on llm failure");
        assert_eq!(selected.source, code_signal.source);
    }
}
