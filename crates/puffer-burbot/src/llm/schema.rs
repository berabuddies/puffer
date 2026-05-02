use super::{is_codex_backend, supports_responses_api};
use crate::contract::{CapabilityContract, SideEffectClass};
use crate::model_policy::model_proposal_args_schema;
use crate::semantics::read_only_side_effect;
use anyhow::{anyhow, Result};
use puffer_provider_openai::{
    build_json_post_request, build_responses_request, BuiltOpenAIRequest, OpenAIRequestConfig,
    OpenAIResponsesRequest, OpenAIResponsesTextConfig, OpenAIResponsesTextFormat,
};
use serde_json::{json, Value};

pub(super) const CHAT_JSON_MAX_TOKENS: u64 = 8192;
pub(super) const CHAT_OBSERVE_ACT_TOOL_NAME: &str = "burbot_propose_candidates";
pub(super) const CHAT_GOAL_VERIFICATION_TOOL_NAME: &str = "burbot_verify_goal";
pub(super) const CHAT_ARTIFACT_REVIEW_TOOL_NAME: &str = "burbot_review_artifact";

pub(super) fn build_tool_call_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        tool_call_text_config(contract)?,
        "burbot-tool-call-proposal",
        "You are Burbot's Puffer-tool proposal operator. Return only the requested structured JSON object.",
    )
}

pub(super) fn build_tool_call_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-tool-call-proposal",
        "You are Burbot's Puffer-tool proposal operator. Return only the requested structured JSON object.",
    )
}

pub(super) fn build_observe_act_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        observe_act_text_config(contract)?,
        "burbot-observe-act-proposal",
        "You are Burbot's observe-act proposal operator. Return only validated structural JSON.",
    )
}

pub(super) fn build_observe_act_chat_tool_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_chat_structured_tool_request(
        config,
        model,
        prompt,
        image_data_urls,
        "You are Burbot's observe-act proposal operator. Call only `burbot_propose_candidates`. Do not call workspace tools such as Read, Bash, Write, Glob, or Grep directly; encode desired workspace tool calls inside the `candidates` array argument.",
        CHAT_OBSERVE_ACT_TOOL_NAME,
        "Validated Burbot candidate actions for the next PESA step.",
        candidate_list_response_schema(contract)?,
    )
}

pub(super) fn build_observe_act_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-observe-act-proposal",
        "You are Burbot's observe-act proposal operator. Return only validated structural JSON.",
    )
}

pub(super) fn build_goal_verification_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        goal_verification_text_config(contract)?,
        "burbot-goal-verification",
        "You are Burbot's goal verifier. Return only validated structural JSON.",
    )
}

pub(super) fn build_goal_verification_chat_tool_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_chat_structured_tool_request(
        config,
        model,
        prompt,
        image_data_urls,
        "You are Burbot's goal verifier. Call only `burbot_verify_goal`. Do not call workspace tools such as Read, Bash, Write, Glob, or Grep directly; encode follow-up workspace tool calls inside the `suggested_candidates` array argument.",
        CHAT_GOAL_VERIFICATION_TOOL_NAME,
        "Goal satisfaction decision plus validated follow-up candidates.",
        goal_verification_response_schema(contract)?,
    )
}

pub(super) fn build_goal_verification_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-goal-verification",
        "You are Burbot's goal verifier. Return only validated structural JSON.",
    )
}

pub(super) fn build_artifact_review_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_structured_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        artifact_review_text_config(contract)?,
        "burbot-artifact-review",
        "You are Burbot's generated-artifact reviewer. Return only validated structural JSON.",
    )
}

pub(super) fn build_artifact_review_chat_tool_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    contract: &CapabilityContract,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_chat_structured_tool_request(
        config,
        model,
        prompt,
        image_data_urls,
        "You are Burbot's generated-artifact reviewer. Call only `burbot_review_artifact`. Do not call workspace tools such as Read, Bash, Write, Glob, or Grep directly; encode follow-up workspace tool calls inside the `suggested_candidates` array argument.",
        CHAT_ARTIFACT_REVIEW_TOOL_NAME,
        "Generated artifact acceptance decision plus validated follow-up candidates.",
        artifact_review_response_schema(contract)?,
    )
}

pub(super) fn build_artifact_review_plain_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
) -> Result<BuiltOpenAIRequest> {
    build_plain_json_request(
        config,
        model,
        prompt,
        image_data_urls,
        "burbot-artifact-review",
        "You are Burbot's generated-artifact reviewer. Return only validated structural JSON.",
    )
}

pub(super) fn observe_act_prompt(
    goal: &str,
    contract: &CapabilityContract,
    observation_context: &Value,
) -> Result<String> {
    Ok(format!(
         "Return exactly this JSON shape: {{\"candidates\":[{{\"id\":\"stable-candidate-id\",\"tool_id\":\"<available tool id>\",\"args\":{{}},\"completion_role\":\"terminal|support|verification|repair\",\"depends_on\":[],\"rationale\":\"why this next action is safe and useful\"}}]}}.\n\
         The `candidates` array must be non-empty. Each candidate must choose one available tool, object args, completion_role, depends_on, and rationale.\n\
         Use stable candidate ids in `id`. Use `depends_on` only for other candidate ids that must execute first; otherwise leave it empty. Do not rely on array order for sequencing.\n\
         Propose only actions that materially advance the goal from the given structural context. \
         Use read-only tools for support/observation work. Unknown-side-effect tools such as shell execution \
         must be repair, verification, or terminal candidates with concrete bounded work, not support candidates. \
         Large or long-running compute must be decomposed into multiple structural candidates with dependencies. \
         Do not put a full generated program, solver, benchmark, migration, or other large payload into one shell command; \
         use file/artifact actions plus bounded execution and verification candidates. If creating or replacing a file, \
         use a file-writing tool with object args instead of shell heredocs, redirects, or inline generated file payloads. \
         If a step needs substantial analysis, search, compilation, symbolic work, or computation, propose a concrete \
         workspace tool action that performs that work against local artifacts; do not spend the response solving the \
         computation in prose. For generated executable artifacts, propose structural candidates that create the artifact, \
         execute it against authoritative local examples or source-derived checks, and only then run the final artifact-producing step. \
         Use `depends_on` to connect those steps when they must execute in order. Mark an action terminal only when it creates \
         or verifies the final required artifact. \
         When the goal expresses an external success condition (e.g. an HTTP response, a file present at a path, a command \
         exits zero, a service answers on a port), the candidate that *demonstrates that condition end-to-end* against the \
         live system MUST be marked `terminal` — orchestration tasks have no single creating action, so the demonstration \
         step is the terminal one. Do not leave a multi-step orchestration without any terminal candidate; if the goal \
         appears satisfied, propose the end-to-end demonstration as terminal. \
         A structurally valid candidate is still invalid if it only prints, echoes, comments on, or restates future work instead of actually \
         inspecting evidence, creating or changing the required artifact, starting a required service, running a concrete check, or repairing a concrete failure. \
         Do not submit placeholder commands, TODO scaffolds, fake success markers, or commands whose only effect is describing what should be implemented. \
         Prefer cheap observations before risky changes, and include verification candidates when useful.\n\n\
         When `context.generated_artifacts.items` is non-empty, use that structural ledger of model-created artifact actions, typed args, outputs, \
         failures, and completion roles to propose concrete artifact repairs or stronger checks; do not ignore the current generated artifact state.\n\n\
         When the context reports missing evidence, a goal-verifier failure, invalid prior candidates, or non-advancing prior candidates, propose concrete \
         valid tool calls that directly collect or repair that evidence. Do not repeat checks already shown successful \
         unless the context shows relevant state changed after those checks. If prior candidates were skipped as duplicates, \
         choose a different concrete action absent from frontier/history, or propose verification/terminal work using existing evidence when appropriate.\n\n\
         When the context includes a classified failure, use its structural `failure_mode`. A `no_progress` failure means the prior action did not create \
         a state, artifact, structural, or verification witness; do not propose another output-only repair for the same missing progress. \
         After a failed generated artifact execution, propose a concrete artifact repair and a bounded verification action that reruns the repaired artifact \
         against local evidence before proposing final completion.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}\n\n\
         Structural context JSON:\n{}",
        serde_json::to_string_pretty(&available_tool_summaries(contract))?,
        serde_json::to_string_pretty(observation_context)?
    ))
}

pub(super) fn goal_verification_prompt(
    goal: &str,
    contract: &CapabilityContract,
    verification_context: &Value,
) -> Result<String> {
    Ok(format!(
         "Return exactly this JSON shape: {{\"satisfied\":false,\"confidence\":0.0,\"missing_evidence\":[],\"suggested_candidates\":[{{\"id\":\"stable-candidate-id\",\"tool_id\":\"<available tool id>\",\"args\":{{}},\"completion_role\":\"verification|support|repair\",\"depends_on\":[],\"rationale\":\"why this exact follow-up resolves missing evidence\"}}]}}. \
         Decide whether the goal is satisfied by the structural evidence. \
         If evidence is missing, set satisfied=false and include concrete follow-up candidates with tool_id, object args, completion_role, and rationale. \
         Large or long-running compute must be decomposed into multiple structural candidates with dependencies instead of one oversized shell command. \
         If creating or replacing a file, use a file-writing tool with object args instead of shell heredocs, redirects, or inline generated file payloads. \
         Use stable candidate ids in `id`. Use `depends_on` only for other suggested candidate ids that must execute first; otherwise leave it empty. Do not rely on array order for sequencing. \
         Suggested candidates must execute concrete inspection, repair, artifact creation, service startup, or verification work; \
         Use read-only tools for support/observation work. Unknown-side-effect tools such as shell execution \
         must be repair or verification candidates with concrete bounded work, not support candidates. \
         if the missing evidence requires substantial analysis, search, compilation, symbolic work, or computation, suggest a \
         concrete workspace tool action that performs it against local artifacts instead of reasoning it out in prose. \
         When `context.generated_artifacts.items` is non-empty, verify or repair those generated artifacts using their structured action refs, \
         typed args, outputs, failures, and completion roles. \
         If a generated executable artifact is part of the solution, require a structural follow-up that exercises the artifact against \
         authoritative local examples, source-derived checks, or existing fixtures before accepting its final output. \
         never suggest placeholder commands, TODO scaffolds, fake success markers, or commands that only describe future work. \
         If the context reports non-advancing prior candidates or duplicate skips, suggest different concrete work absent \
         from history, or use the existing evidence to propose the required verification/repair. \
         Judge satisfaction from what has actually been observed in `history`, `verified_target`, and `generated_artifacts` — \
         not from speculative future work. There is no queued-work list in this context; if the executed evidence already \
         demonstrates the goal's external success condition, return satisfied=true with empty missing_evidence. \
         Do not infer success from a tool exit code alone when the goal requires an artifact or semantic result. \
         For generated code, migrations, ports, reimplementations, protocol services, or other behavioral artifacts, \
         a narrow self-check is not enough: require independent project tests when available, or broad representative checks \
         that exercise stateful/repeated operations, failure paths, edge cases, and final artifacts required by the goal. \
         For interface, protocol, schema, API, CLI, file-format, or wire-format goals, exact declared names are semantic \
         requirements: service names, method names, message names, field names, route names, ports, file names, command names, \
         enum variants, and parameter names must match the goal or source contract. Tests that only use generated clients, \
         wrappers, or helpers from the current artifact do not prove exact contract compatibility unless they also compare \
         the generated definitions or external contract text against the requested names. \
         If the evidence only compares one current input, one happy path, or a single synthetic example, mark the goal unsatisfied \
         and propose stronger verification or repair candidates. If required artifact contents, diffs, test results, or contract \
         comparisons are absent from the evidence, propose concrete read/search/test candidates instead of returning no candidates.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}\n\n\
         Evidence JSON:\n{}",
        serde_json::to_string_pretty(&available_tool_summaries(contract))?,
        serde_json::to_string_pretty(verification_context)?
    ))
}

pub(super) fn generated_artifact_review_prompt(
    goal: &str,
    contract: &CapabilityContract,
    review_context: &Value,
) -> Result<String> {
    Ok(format!(
         "Return exactly this JSON shape: {{\"satisfied\":false,\"confidence\":0.0,\"missing_evidence\":[],\"suggested_candidates\":[{{\"id\":\"stable-candidate-id\",\"tool_id\":\"<available tool id>\",\"args\":{{}},\"completion_role\":\"verification|support|repair\",\"depends_on\":[],\"rationale\":\"why this exact follow-up resolves the artifact risk\"}}]}}. \
         In this artifact-review context, `satisfied=true` means the generated artifact is safe, structurally plausible, and useful enough to proceed to planned execution or completion checks. \
         It does not mean the overall user goal is complete and it does not mean the artifact has already passed runtime verification. \
         `satisfied=false` means the artifact has concrete structural, safety, interface, boundedness, or consistency defects that should be repaired before downstream actions run. \
         Do not reject an artifact merely because it still needs compile, runtime, fixture, or goal-verification evidence. Those checks belong to downstream verification actions. \
         Decide only from the structural evidence JSON, including the action args, structured tool output, generated artifact ledger, frontier, and history. \
         If the artifact is not accepted, include concrete follow-up candidates with tool_id, object args, completion_role, depends_on, and rationale. \
         Do not call workspace tools directly from this response; encode follow-up workspace calls in suggested_candidates. \
         If creating or replacing a file, use a file-writing tool with object args instead of shell heredocs, redirects, or inline generated file payloads. \
         Suggested candidates must execute concrete repair, artifact creation, source-derived checking, local fixture checking, or verification work. \
         Unknown-side-effect tools such as shell execution must be repair or verification candidates with concrete bounded work, not support candidates. \
         Reject generated executable artifacts that are merely scaffolds, TODOs, fake success markers, placeholders, narrative analysis, guessed interfaces, unbounded/infeasible computations, or partial algorithms. \
         Reject artifacts whose own evidence shows they were not derived from authoritative local source, existing fixtures, declared contracts, or prior observations when such evidence is needed. \
         Reject artifacts that omit their own required output behavior or test only helpers generated by the same artifact. \
         Accept artifacts that make a concrete source-derived attempt with no obvious safety, interface, boundedness, or placeholder defect, even if only execution can determine correctness. \
         When rejecting, propose a concrete artifact repair and a bounded verification action that exercises the repaired artifact against authoritative local examples, source-derived checks, or existing fixtures. \
         Suggested file-writing candidates must contain complete replacement content for the repaired artifact. If complete content is not available from the evidence, suggest structural read, search, compile, run, or verification actions first instead of a placeholder write. \
         When accepting, use confidence >= 0.5 if the artifact has a clear structural path to verification and no obvious missing contract, file-format, interface, final-output, boundedness, or safety obligation remains.\n\n\
         Available tools:\n{}\n\n\
         Goal:\n{goal}\n\n\
         Artifact review evidence JSON:\n{}",
        serde_json::to_string_pretty(&available_tool_summaries(contract))?,
        serde_json::to_string_pretty(review_context)?
    ))
}

fn build_structured_json_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    text: OpenAIResponsesTextConfig,
    prompt_cache_key: &str,
    instructions: &str,
) -> Result<BuiltOpenAIRequest> {
    if is_codex_backend(&config.base_url) {
        let body = json!({
            "model": model,
            "instructions": instructions,
            "input": [{
                "type": "message",
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": false,
            "include": [],
            "text": text,
            "prompt_cache_key": prompt_cache_key
        });
        build_json_post_request(config, "/responses", &body)
    } else if supports_responses_api(&config.base_url) && image_data_urls.is_empty() {
        build_responses_request(
            config,
            &OpenAIResponsesRequest {
                model: model.to_string(),
                input: prompt.to_string(),
                text: Some(text),
            },
        )
    } else if supports_responses_api(&config.base_url) {
        let body = json!({
            "model": model,
            "input": [{
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
            "text": text,
        });
        build_json_post_request(config, "/v1/responses", &body)
    } else {
        build_chat_json_object_request(config, model, prompt, image_data_urls, Some(instructions))
    }
}

fn build_chat_structured_tool_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    instructions: &str,
    tool_name: &str,
    tool_description: &str,
    parameters: Value,
) -> Result<BuiltOpenAIRequest> {
    let mut body = json!({
        "model": model,
        "messages": [
            {
                "role": "system",
                "content": instructions
            },
            {
                "role": "user",
                "content": chat_content(prompt, image_data_urls)
            }
        ],
        "tools": [{
            "type": "function",
            "function": {
                "name": tool_name,
                "description": tool_description,
                "parameters": parameters,
                "strict": false
            }
        }],
        "tool_choice": {
            "type": "function",
            "function": {
                "name": tool_name
            }
        },
        "max_tokens": chat_json_max_tokens(config),
        "stream": false
    });
    apply_chat_compatibility_overrides(config, &mut body);
    build_json_post_request(config, "/v1/chat/completions", &body)
}

fn build_plain_json_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    prompt_cache_key: &str,
    instructions: &str,
) -> Result<BuiltOpenAIRequest> {
    if is_codex_backend(&config.base_url) {
        let body = json!({
            "model": model,
            "instructions": instructions,
            "input": [{
                "type": "message",
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
            "tools": [],
            "tool_choice": "auto",
            "parallel_tool_calls": false,
            "store": false,
            "stream": false,
            "include": [],
            "prompt_cache_key": prompt_cache_key
        });
        build_json_post_request(config, "/responses", &body)
    } else if supports_responses_api(&config.base_url) && image_data_urls.is_empty() {
        build_responses_request(
            config,
            &OpenAIResponsesRequest {
                model: model.to_string(),
                input: prompt.to_string(),
                text: None,
            },
        )
    } else if supports_responses_api(&config.base_url) {
        let body = json!({
            "model": model,
            "input": [{
                "role": "user",
                "content": multimodal_content(prompt, image_data_urls)
            }],
        });
        build_json_post_request(config, "/v1/responses", &body)
    } else {
        build_chat_plain_json_request(config, model, prompt, image_data_urls, Some(instructions))
    }
}

fn multimodal_content(prompt: &str, image_data_urls: &[String]) -> Vec<Value> {
    let mut content = vec![json!({"type": "input_text", "text": prompt})];
    content.extend(image_data_urls.iter().map(|image_url| {
        json!({
            "type": "input_image",
            "image_url": image_url,
            "detail": "auto",
        })
    }));
    content
}

fn build_chat_json_object_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    instructions: Option<&str>,
) -> Result<BuiltOpenAIRequest> {
    let mut messages = Vec::new();
    if let Some(instructions) = instructions {
        messages.push(json!({
            "role": "system",
            "content": chat_json_object_instructions(config, instructions)
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": chat_content(prompt, image_data_urls)
    }));
    let mut body = json!({
        "model": model,
        "messages": messages,
        "response_format": {"type": "json_object"},
        "max_tokens": chat_json_max_tokens(config),
        "stream": false
    });
    apply_chat_compatibility_overrides(config, &mut body);
    build_json_post_request(config, "/v1/chat/completions", &body)
}

fn build_chat_plain_json_request(
    config: &OpenAIRequestConfig,
    model: &str,
    prompt: &str,
    image_data_urls: &[String],
    instructions: Option<&str>,
) -> Result<BuiltOpenAIRequest> {
    let mut messages = Vec::new();
    if let Some(instructions) = instructions {
        messages.push(json!({
            "role": "system",
            "content": plain_json_instructions(instructions)
        }));
    } else {
        messages.push(json!({
            "role": "system",
            "content": plain_json_instructions("Return only the requested structural JSON object.")
        }));
    }
    messages.push(json!({
        "role": "user",
        "content": chat_content(prompt, image_data_urls)
    }));
    let mut body = json!({
        "model": model,
        "messages": messages,
        "max_tokens": chat_json_max_tokens(config),
        "stream": false
    });
    apply_chat_compatibility_overrides(config, &mut body);
    build_json_post_request(config, "/v1/chat/completions", &body)
}

fn chat_json_object_instructions(config: &OpenAIRequestConfig, instructions: &str) -> String {
    if deepseek_chat_endpoint(&config.base_url) {
        plain_json_instructions(instructions)
    } else {
        instructions.to_string()
    }
}

fn plain_json_instructions(instructions: &str) -> String {
    format!(
        "{instructions}\n\n\
         Output contract: reply with exactly one raw JSON object and nothing else. \
         The first byte of the assistant message must be `{{` and the last byte must be `}}`. \
         Do not wrap the JSON in markdown fences. Do not prefix it with prose. Do not suffix it with commentary. \
         If you cannot satisfy the requested task, return a JSON object with the requested schema and structural follow-up candidates."
    )
}

pub(super) fn apply_chat_compatibility_overrides(config: &OpenAIRequestConfig, body: &mut Value) {
    let mode = chat_thinking_mode(config);
    if let Some(object) = body.as_object_mut() {
        if let Some(mode) = mode.as_deref() {
            object.insert("thinking".to_string(), json!({"type": mode}));
        }
        if deepseek_chat_endpoint(&config.base_url) && mode.as_deref() != Some("disabled") {
            object.insert("reasoning_effort".to_string(), chat_reasoning_effort());
        }
    }
}

fn chat_thinking_mode(config: &OpenAIRequestConfig) -> Option<String> {
    if let Ok(mode) = std::env::var("BURBOT_OPENAI_THINKING_MODE") {
        let mode = mode.trim().to_ascii_lowercase();
        return match mode.as_str() {
            "enabled" | "disabled" => Some(mode),
            "omit" | "none" => None,
            _ => None,
        };
    }
    deepseek_chat_endpoint(&config.base_url).then(|| "disabled".to_string())
}

fn chat_reasoning_effort() -> Value {
    let effort = std::env::var("BURBOT_OPENAI_REASONING_EFFORT")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| matches!(value.as_str(), "high" | "max"))
        .unwrap_or_else(|| "max".to_string());
    Value::String(effort)
}

fn chat_json_max_tokens(_config: &OpenAIRequestConfig) -> u64 {
    if let Ok(value) = std::env::var("BURBOT_OPENAI_MAX_TOKENS") {
        if let Ok(parsed) = value.trim().parse::<u64>() {
            return parsed;
        }
    }
    CHAT_JSON_MAX_TOKENS
}

pub(super) fn deepseek_chat_endpoint(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .is_some_and(|host| host == "api.deepseek.com" || host.ends_with(".api.deepseek.com"))
}

fn chat_content(prompt: &str, image_data_urls: &[String]) -> Value {
    if image_data_urls.is_empty() {
        return Value::String(prompt.to_string());
    }
    let mut content = vec![json!({"type": "text", "text": prompt})];
    content.extend(image_data_urls.iter().map(|image_url| {
        json!({
            "type": "image_url",
            "image_url": {"url": image_url}
        })
    }));
    Value::Array(content)
}

fn tool_call_text_config(contract: &CapabilityContract) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_puffer_tool_call".to_string(),
            description: Some(
                "One validated Puffer tool call proposal for the current Burbot goal.".to_string(),
            ),
            schema: tool_call_response_schema(contract)?,
            strict: true,
        },
    })
}

fn tool_call_response_schema(contract: &CapabilityContract) -> Result<Value> {
    let variants = contract
        .actions
        .iter()
        .map(tool_call_variant_schema)
        .collect::<Vec<_>>();
    if variants.is_empty() {
        return Err(anyhow!("Puffer tool contract has no available actions"));
    }
    Ok(json!({"oneOf": variants}))
}

fn observe_act_text_config(contract: &CapabilityContract) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_observe_act_candidates".to_string(),
            description: Some(
                "Zero or more validated Puffer tool-call candidates for the next Burbot step."
                    .to_string(),
            ),
            schema: candidate_list_response_schema(contract)?,
            strict: true,
        },
    })
}

fn goal_verification_text_config(
    contract: &CapabilityContract,
) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_goal_verification".to_string(),
            description: Some(
                "Goal satisfaction decision with optional validated follow-up candidates."
                    .to_string(),
            ),
            schema: goal_verification_response_schema(contract)?,
            strict: true,
        },
    })
}

fn artifact_review_text_config(contract: &CapabilityContract) -> Result<OpenAIResponsesTextConfig> {
    Ok(OpenAIResponsesTextConfig {
        format: OpenAIResponsesTextFormat {
            kind: "json_schema".to_string(),
            name: "burbot_artifact_review".to_string(),
            description: Some(
                "Generated artifact acceptance decision with optional validated follow-up candidates."
                    .to_string(),
            ),
            schema: artifact_review_response_schema(contract)?,
            strict: true,
        },
    })
}

fn candidate_list_response_schema(contract: &CapabilityContract) -> Result<Value> {
    Ok(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "candidates": {
                "type": "array",
                "minItems": 1,
                "items": candidate_response_schema(contract)?
            }
        },
        "required": ["candidates"]
    }))
}

fn goal_verification_response_schema(contract: &CapabilityContract) -> Result<Value> {
    Ok(json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "satisfied": {"type": "boolean"},
            "confidence": {"type": "number", "minimum": 0.0, "maximum": 1.0},
            "missing_evidence": {
                "type": "array",
                "items": {"type": "string"}
            },
            "suggested_candidates": {
                "type": "array",
                "items": candidate_response_schema(contract)?
            }
        },
        "required": ["satisfied", "confidence", "missing_evidence", "suggested_candidates"]
    }))
}

fn artifact_review_response_schema(contract: &CapabilityContract) -> Result<Value> {
    goal_verification_response_schema(contract)
}

fn candidate_response_schema(contract: &CapabilityContract) -> Result<Value> {
    let variants = contract
        .actions
        .iter()
        .map(candidate_variant_schema)
        .collect::<Vec<_>>();
    if variants.is_empty() {
        return Err(anyhow!("Puffer tool contract has no available actions"));
    }
    Ok(json!({"oneOf": variants}))
}

fn tool_call_variant_schema(action: &crate::contract::ActionContract) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "tool_id": {"type": "string", "enum": [action.name.clone()]},
            "args": model_proposal_args_schema(action),
        },
        "required": ["tool_id", "args"]
    })
}

fn candidate_variant_schema(action: &crate::contract::ActionContract) -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "id": {"type": "string", "minLength": 1},
            "tool_id": {"type": "string", "enum": [action.name.clone()]},
            "args": model_proposal_args_schema(action),
            "completion_role": {
                "type": "string",
                "enum": completion_role_values_for_action(action)
            },
            "depends_on": {
                "type": "array",
                "items": {"type": "string", "minLength": 1}
            },
            "rationale": {"type": "string"}
        },
        "required": ["id", "tool_id", "args", "completion_role", "depends_on", "rationale"]
    })
}

fn completion_role_values_for_action(
    action: &crate::contract::ActionContract,
) -> Vec<&'static str> {
    if read_only_side_effect(&action.side_effect_class) {
        return vec!["terminal", "support", "observation", "verification"];
    }
    if action.side_effect_class == SideEffectClass::Unknown {
        return vec!["terminal", "verification", "repair"];
    }
    vec!["terminal", "repair"]
}

fn available_tool_summaries(contract: &CapabilityContract) -> Vec<Value> {
    contract
        .actions
        .iter()
        .map(|action| {
            json!({
                "tool_id": action.name,
                "description": action.description,
                "input_schema": model_proposal_args_schema(action),
                "side_effect_class": action.side_effect_class,
                "risk_level": action.risk_level,
                "verification_required": action.verification.required_before_completion,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_provider_openai::{OpenAIAuth, OpenAIRequestConfig};

    fn codex_config() -> OpenAIRequestConfig {
        OpenAIRequestConfig {
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
            version: "0.125.0".to_string(),
            auth: OpenAIAuth::OAuthBearer("token".to_string()),
            originator: "codex_cli_rs".to_string(),
            session_id: None,
            account_id: None,
            custom_headers: Vec::new(),
            query_params: Vec::new(),
        }
    }

    fn contract() -> CapabilityContract {
        CapabilityContract {
            contract_id: "puffer.tools".to_string(),
            version: "0.1.0".to_string(),
            status: crate::contract::ContractStatus::Active,
            trust_level: crate::contract::TrustLevel::Sandboxed,
            description: "tools".to_string(),
            actions: vec![crate::contract::ActionContract {
                name: "Read".to_string(),
                description: "read".to_string(),
                input_schema: json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {"file_path": {"type": "string"}},
                    "required": ["file_path"],
                }),
                output_schema: json!({"type": "object"}),
                side_effect_class: crate::contract::SideEffectClass::LocalRead,
                reversibility: crate::contract::Reversibility::Reversible,
                idempotency: crate::contract::Idempotency::Idempotent,
                risk_level: crate::contract::RiskLevel::Low,
                preconditions: Vec::new(),
                postconditions: Vec::new(),
                verification: crate::contract::VerificationSpec {
                    methods: Vec::new(),
                    observation_checks: Vec::new(),
                    method_templates: Vec::new(),
                    templates: Vec::new(),
                    required_before_completion: false,
                    confidence: 0.5,
                },
                approval: crate::contract::ApprovalSpec {
                    required: false,
                    reason: None,
                },
                failure_modes: Vec::new(),
                forbidden_uses: Vec::new(),
                argument_safety: Vec::new(),
                structured_argument_safety: Vec::new(),
                semantic_intents: Vec::new(),
                intent_extractors: Vec::new(),
                repair_rules: Vec::new(),
                cost_estimate: None,
                latency_estimate: None,
            }],
            global_constraints: Vec::new(),
            forbidden_uses: Vec::new(),
            local_rules: Vec::new(),
            examples: Vec::new(),
            contract_hash: None,
        }
    }

    fn shell_contract() -> CapabilityContract {
        let mut contract = contract();
        let mut slots = std::collections::BTreeMap::new();
        slots.insert("command".to_string(), "command".to_string());
        let action = &mut contract.actions[0];
        action.name = "Bash".to_string();
        action.description = "run shell".to_string();
        action.input_schema = json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "command": {"type": "string"},
                "timeout": {"type": "integer", "minimum": 1, "maximum": 600000}
            },
            "required": ["command"]
        });
        action.side_effect_class = crate::contract::SideEffectClass::Unknown;
        action.risk_level = crate::contract::RiskLevel::High;
        action.semantic_intents = vec![crate::contract::SemanticIntentSpec {
            intent: "shell_command".to_string(),
            slots,
            optional_slots: std::collections::BTreeMap::new(),
            defaults: std::collections::BTreeMap::new(),
            side_effect_class: Some(crate::contract::SideEffectClass::Unknown),
            slot_kinds: Default::default(),
        }];
        contract
    }

    #[test]
    fn goal_verification_request_attaches_images_as_content_items() {
        let image = "data:image/png;base64,aGVsbG8=".to_string();
        let request = build_goal_verification_request(
            &codex_config(),
            "gpt-5.5",
            "verify",
            &contract(),
            &[image.clone()],
        )
        .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();

        assert_eq!(body["text"]["format"]["type"], json!("json_schema"));
        assert_eq!(body["input"][0]["content"][1]["type"], json!("input_image"));
        assert_eq!(body["input"][0]["content"][1]["image_url"], json!(image));
    }

    #[test]
    fn candidate_schemas_do_not_cap_transition_counts() {
        let request =
            build_observe_act_request(&codex_config(), "gpt-5.5", "propose", &contract(), &[])
                .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();

        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["candidates"]["maxItems"],
            Value::Null
        );

        let request =
            build_goal_verification_request(&codex_config(), "gpt-5.5", "verify", &contract(), &[])
                .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();

        assert_eq!(
            body["text"]["format"]["schema"]["properties"]["suggested_candidates"]["maxItems"],
            Value::Null
        );
    }

    #[test]
    fn model_proposal_schemas_bound_executable_command_and_timeout() {
        let request = build_observe_act_request(
            &codex_config(),
            "gpt-5.5",
            "propose",
            &shell_contract(),
            &[],
        )
        .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let args = &body["text"]["format"]["schema"]["properties"]["candidates"]["items"]["oneOf"]
            [0]["properties"]["args"];

        assert_eq!(
            args["properties"]["command"]["maxLength"],
            json!(crate::model_policy::MAX_MODEL_EXECUTABLE_ARG_CHARS)
        );
        assert_eq!(
            args["properties"]["timeout"]["maximum"],
            json!(crate::model_policy::MAX_MODEL_FOREGROUND_TIMEOUT_MS)
        );

        let request =
            build_tool_call_request(&codex_config(), "gpt-5.5", "run", &shell_contract(), &[])
                .unwrap();
        let body: Value = serde_json::from_str(&request.body).unwrap();
        let args = &body["text"]["format"]["schema"]["oneOf"][0]["properties"]["args"];

        assert_eq!(
            args["properties"]["command"]["maxLength"],
            json!(crate::model_policy::MAX_MODEL_EXECUTABLE_ARG_CHARS)
        );
        assert_eq!(
            args["properties"]["timeout"]["maximum"],
            json!(crate::model_policy::MAX_MODEL_FOREGROUND_TIMEOUT_MS)
        );
    }

    #[test]
    fn goal_verification_prompt_requires_strong_behavioral_evidence() {
        let prompt = goal_verification_prompt(
            "Re-implement the service behavior in Python.",
            &contract(),
            &json!({}),
        )
        .unwrap();

        assert!(prompt.contains("narrow self-check is not enough"));
        assert!(prompt.contains("stateful/repeated operations"));
        assert!(prompt.contains("one current input"));
        assert!(prompt.contains("field names"));
        assert!(prompt.contains("parameter names"));
        assert!(prompt.contains("generated clients"));
        assert!(prompt.contains("propose concrete read/search/test candidates"));
        assert!(prompt.contains("performs it against local artifacts"));
        assert!(prompt.contains("\"tool_id\""));
        assert!(prompt.contains("object args"));
    }

    #[test]
    fn observe_act_prompt_pushes_computation_into_tool_actions() {
        let prompt =
            observe_act_prompt("recover a generated artifact", &contract(), &json!({})).unwrap();

        assert!(prompt.contains("performs that work against local artifacts"));
        assert!(prompt.contains("Large or long-running compute"));
        assert!(prompt.contains("Mark an action terminal only"));
    }
}
