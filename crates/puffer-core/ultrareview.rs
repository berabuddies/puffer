// Multi-agent code review orchestrator backing the `/ultrareview`
// slash command. Pipeline: planner → 5 parallel lanes → confidence
// filter + dedup → filter LLM → markdown. Talks to the `openai`
// provider's /v1/chat/completions endpoint, inheriting puffer's
// resolved base URL and credentials.

use anyhow::{anyhow, bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;

use puffer_provider_registry::{AuthStore, ProviderRegistry, StoredCredential};

/// Provider whose base URL + credentials the orchestrator borrows from
/// puffer. `canonical_provider_id` maps imported codex credentials here.
const ULTRAREVIEW_PROVIDER_ID: &str = "openai";
/// Defensive fallback; in practice the `openai` provider always carries
/// a base URL (overridable via puffer config).
const DEFAULT_BASE_URL: &str = "https://api.openai.com";
const DEFAULT_MODEL: &str = "gpt-5.5";
const LANE_TIMEOUT_SECS: u64 = 420;
const CONFIDENCE_THRESHOLD: f64 = 0.5;
const DIFF_TRUNCATE_LANES: usize = 80_000;
const DIFF_TRUNCATE_FILTER: usize = 60_000;
const MAX_LANE_CONCURRENCY: usize = 3;

const LANE_TO_ID: &[(&str, &str)] = &[
    ("security", "reviewer-security"),
    ("logic", "reviewer-logic"),
    ("duplication", "reviewer-duplication"),
    ("editorial", "reviewer-editorial"),
    ("architecture-fit", "reviewer-architecture"),
];

const PLANNER_ID: &str = "reviewer-planner";
const FILTER_ID: &str = "reviewer-filter";

pub struct OrchestrateRequest {
    pub diff: String,
    pub model: String,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub agents_dir: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct OrchestrateResult {
    pub markdown: String,
    pub planner_lanes: Vec<String>,
    pub planner_rationale: String,
    pub aggregated_count: usize,
    pub final_count: usize,
    pub per_lane: HashMap<String, LaneRun>,
    pub timings_s: Timings,
}

#[derive(Debug, Serialize, Clone)]
pub struct LaneRun {
    pub ok: bool,
    pub duration_s: f64,
    pub finding_count: usize,
    pub error: Option<String>,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct Timings {
    pub planner: f64,
    pub lanes: f64,
    pub filter: f64,
    pub total: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct AgentDef {
    #[allow(dead_code)]
    id: String,
    prompt: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Finding {
    #[serde(default)]
    reasoning: String,
    file_line: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    fix: String,
    #[serde(default = "default_severity")]
    severity: String,
    #[serde(default)]
    confidence: f64,
    #[serde(default)]
    lanes: Vec<String>,
}

fn default_severity() -> String {
    "SHOULD-FIX".to_string()
}

/// Resolves the `openai` provider's base URL and credential from puffer's
/// registry + auth store. `canonical_provider_id` maps imported codex
/// credentials onto this provider id.
pub fn resolve_credentials(
    providers: &ProviderRegistry,
    auth_store: &AuthStore,
) -> (Option<String>, Option<String>) {
    let base_url = providers
        .provider(ULTRAREVIEW_PROVIDER_ID)
        .map(|p| p.base_url.clone());
    let api_key = match auth_store.get(ULTRAREVIEW_PROVIDER_ID) {
        Some(StoredCredential::ApiKey { key }) => Some(key.clone()),
        Some(StoredCredential::OAuth(c)) => Some(c.access_token.clone()),
        None => None,
    };
    (base_url, api_key)
}

/// Blocking entry for the TUI's background thread: fetch the PR diff via
/// `gh pr diff`, run the pipeline (reporting phase progress through
/// `progress`), and return the rendered markdown plus a timing footer.
pub fn run_review_blocking(
    cwd: &Path,
    pr_arg: &str,
    base_url: Option<String>,
    api_key: Option<String>,
    progress: &dyn Fn(String),
    cancel: &crate::CancelToken,
) -> Result<String> {
    progress("fetching diff…".to_string());
    let diff = fetch_diff_via_gh_in(cwd, pr_arg)?;
    if diff.trim().is_empty() {
        bail!("empty diff for {pr_arg}");
    }
    cancel.check()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let out = runtime.block_on(orchestrate_async(
        OrchestrateRequest {
            diff,
            model: DEFAULT_MODEL.to_string(),
            base_url,
            api_key,
            agents_dir: None,
        },
        progress,
        cancel,
    ))?;
    let footer = format!(
        "\n_planner: {:.0}s · lanes: {:.0}s · filter: {:.0}s · total: {:.0}s · aggregated: {} → kept: {}_",
        out.timings_s.planner,
        out.timings_s.lanes,
        out.timings_s.filter,
        out.timings_s.total,
        out.aggregated_count,
        out.final_count,
    );
    Ok(format!("{}\n{}", out.markdown, footer))
}

/// Runs the UltraReview planner, lane reviewers, aggregation, and final filter.
pub async fn orchestrate_async(
    req: OrchestrateRequest,
    progress: &dyn Fn(String),
    cancel: &crate::CancelToken,
) -> Result<OrchestrateResult> {
    let agents_dir = req.agents_dir.unwrap_or_else(default_agents_dir);
    if !agents_dir.exists() {
        bail!("agents dir does not exist: {:?}", agents_dir);
    }

    let base_url = req
        .base_url
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
    let api_key = req
        .api_key
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| anyhow!("no API key supplied; configure the `openai` provider in puffer"))?;
    let model = if req.model.is_empty() {
        DEFAULT_MODEL.to_string()
    } else {
        req.model
    };

    let client = Client::builder()
        .timeout(Duration::from_secs(LANE_TIMEOUT_SECS))
        .build()?;
    let t_start = Instant::now();

    cancel.check()?;
    let t_planner = Instant::now();
    let force_all = std::env::var("PUFFER_ULTRAREVIEW_FORCE_ALL_LANES")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE"))
        .unwrap_or(false);
    let (planner_lanes, planner_rationale) = if force_all {
        (
            LANE_TO_ID.iter().map(|(n, _)| n.to_string()).collect(),
            "PUFFER_ULTRAREVIEW_FORCE_ALL_LANES=1: planner skipped".to_string(),
        )
    } else {
        let planner_prompt = load_agent_prompt(&agents_dir, PLANNER_ID)?;
        let planner_user = format!(
            "--- diff ---\n{}\n--- end diff ---\n\nOutput the JSON object now.",
            truncate(&req.diff, DIFF_TRUNCATE_LANES)
        );
        call_planner(
            &client,
            &base_url,
            &api_key,
            &model,
            &planner_prompt,
            &planner_user,
        )
        .await
    };
    let planner_secs = t_planner.elapsed().as_secs_f64();
    progress(format!(
        "planner → {} lane(s): {} · reviewing…",
        planner_lanes.len(),
        planner_lanes.join(", ")
    ));

    let t_lanes = Instant::now();
    let diff_clipped = truncate(&req.diff, DIFF_TRUNCATE_LANES).to_string();
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut lane_runs: HashMap<String, LaneRun> = HashMap::new();
    {
        let mut joinset: JoinSet<(String, LaneRun, Vec<Finding>)> = JoinSet::new();
        let mut lanes_iter = planner_lanes.iter();
        for _ in 0..MAX_LANE_CONCURRENCY {
            if let Some(lane_name) = lanes_iter.next() {
                spawn_lane(
                    &mut joinset,
                    &client,
                    &base_url,
                    &api_key,
                    &model,
                    &agents_dir,
                    &diff_clipped,
                    lane_name.clone(),
                );
            }
        }
        while let Some(joined) = joinset.join_next().await {
            match joined {
                Ok((lane_name, lane_run, findings)) => {
                    progress(format!("✓ {} ({} finding(s))", lane_name, findings.len()));
                    all_findings.extend(findings);
                    lane_runs.insert(lane_name, lane_run);
                }
                Err(e) => eprintln!("ultrareview lane join error: {e}"),
            }
            if cancel.is_cancelled() {
                joinset.abort_all(); // drop in-flight lane calls so ESC is responsive
                break;
            }
            if let Some(lane_name) = lanes_iter.next() {
                spawn_lane(
                    &mut joinset,
                    &client,
                    &base_url,
                    &api_key,
                    &model,
                    &agents_dir,
                    &diff_clipped,
                    lane_name.clone(),
                );
            }
        }
    }
    cancel.check()?;
    let lanes_secs = t_lanes.elapsed().as_secs_f64();

    let mut by_loc: HashMap<String, Finding> = HashMap::new();
    for f in all_findings.into_iter() {
        if f.confidence < CONFIDENCE_THRESHOLD {
            continue;
        }
        match by_loc.get_mut(&f.file_line) {
            Some(existing) => {
                if f.confidence > existing.confidence {
                    let mut merged = existing.lanes.clone();
                    for ln in &f.lanes {
                        if !merged.contains(ln) {
                            merged.push(ln.clone());
                        }
                    }
                    *existing = f;
                    existing.lanes = merged;
                } else {
                    for ln in f.lanes {
                        if !existing.lanes.contains(&ln) {
                            existing.lanes.push(ln);
                        }
                    }
                }
            }
            None => {
                by_loc.insert(f.file_line.clone(), f);
            }
        }
    }
    let aggregated: Vec<Finding> = by_loc.into_values().collect();
    let aggregated_count = aggregated.len();
    progress(format!(
        "aggregated {aggregated_count} finding(s) → filtering…"
    ));

    let t_filter = Instant::now();
    let filter_prompt = load_agent_prompt(&agents_dir, FILTER_ID)?;
    let filter_user = build_filter_user(&req.diff, &aggregated);
    // On filter error, fail open (keep everything) to preserve recall, but
    // flag the degradation so the reader knows the findings are unfiltered.
    let (keep_set, filter_failed) = match call_filter(
        &client,
        &base_url,
        &api_key,
        &model,
        &filter_prompt,
        &filter_user,
    )
    .await
    {
        Ok(keep) => (keep, false),
        Err(_) => (
            aggregated.iter().map(|f| f.file_line.clone()).collect(),
            true,
        ),
    };
    let final_findings: Vec<Finding> = aggregated
        .iter()
        .filter(|f| keep_set.contains(&f.file_line))
        .cloned()
        .collect();
    let filter_secs = t_filter.elapsed().as_secs_f64();
    let final_count = final_findings.len();

    let dropped = aggregated_count.saturating_sub(final_count);
    let markdown = render_markdown(
        &final_findings,
        &planner_lanes,
        &planner_rationale,
        dropped,
        filter_failed,
    );

    Ok(OrchestrateResult {
        markdown,
        planner_lanes,
        planner_rationale,
        aggregated_count,
        final_count,
        per_lane: lane_runs,
        timings_s: Timings {
            planner: round2(planner_secs),
            lanes: round2(lanes_secs),
            filter: round2(filter_secs),
            total: round2(t_start.elapsed().as_secs_f64()),
        },
    })
}

/// Fetches a pull request diff through the GitHub CLI.
pub fn fetch_diff_via_gh(pr_url: &str) -> Result<String> {
    fetch_diff_via_gh_in(Path::new("."), pr_url)
}

fn fetch_diff_via_gh_in(cwd: &Path, pr_url: &str) -> Result<String> {
    let out = Command::new("gh")
        .args(["pr", "diff", pr_url])
        .current_dir(cwd)
        .output()
        .context("invoking gh CLI")?;
    if !out.status.success() {
        bail!(
            "gh pr diff failed (status {:?}): {}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Returns the default directory containing bundled UltraReview agent prompts.
pub fn default_agents_dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(root) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            let candidate = root.join("resources/agents");
            if candidate.exists() {
                return candidate;
            }
        }
    }
    PathBuf::from("resources/agents")
}

fn load_agent_prompt(agents_dir: &Path, id: &str) -> Result<String> {
    let path = agents_dir.join(format!("{id}.yaml"));
    let text = std::fs::read_to_string(&path).with_context(|| format!("read agent {:?}", path))?;
    let agent: AgentDef =
        serde_yaml::from_str(&text).with_context(|| format!("parse agent {:?}", path))?;
    Ok(agent.prompt)
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) && end > 0 {
        end -= 1;
    }
    &s[..end]
}

fn build_lane_user(diff: &str) -> String {
    format!(
        "Review this diff strictly within your declared lane.\n\n```diff\n{diff}\n```\n\nOutput the JSON array of findings now."
    )
}

fn build_filter_user(diff: &str, findings: &[Finding]) -> String {
    let cand_blocks: Vec<String> = findings
        .iter()
        .map(|f| {
            let lane_tag = f.lanes.join(", ");
            format!(
                "- [{}] ({}) [conf={:.2}] {}\n  reasoning: {}\n  body: {}",
                f.file_line,
                lane_tag,
                f.confidence,
                f.title,
                truncate(&f.reasoning, 300),
                truncate(&f.body, 300),
            )
        })
        .collect();
    format!(
        "--- diff ---\n{diff}\n--- end diff ---\n\n--- candidate findings ---\n{cands}\n--- end candidates ---\n\nOutput the JSON object listing file_line strings to keep.",
        diff = truncate(diff, DIFF_TRUNCATE_FILTER),
        cands = cand_blocks.join("\n"),
    )
}

fn spawn_lane(
    joinset: &mut JoinSet<(String, LaneRun, Vec<Finding>)>,
    client: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    agents_dir: &Path,
    diff_clipped: &str,
    lane_name: String,
) {
    let Some(lane_id) = LANE_TO_ID
        .iter()
        .find(|(name, _)| *name == lane_name.as_str())
        .map(|(_, id)| id.to_string())
    else {
        return;
    };
    let prompt = match load_agent_prompt(agents_dir, &lane_id) {
        Ok(p) => p,
        Err(e) => {
            let error = format!("load prompt {lane_id}: {e}");
            joinset.spawn(async move {
                (
                    lane_name,
                    LaneRun {
                        ok: false,
                        duration_s: 0.0,
                        finding_count: 0,
                        error: Some(error),
                    },
                    vec![],
                )
            });
            return;
        }
    };
    let client = client.clone();
    let base_url = base_url.to_string();
    let api_key = api_key.to_string();
    let model = model.to_string();
    let user = build_lane_user(diff_clipped);
    joinset.spawn(async move {
        run_lane(
            client, base_url, api_key, model, lane_name, lane_id, prompt, user,
        )
        .await
    });
}

async fn chat_completion(
    client: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    system: &str,
    user: &str,
) -> Result<String> {
    let body = json!({
        "model": model,
        "messages": [
            {"role": "system", "content": system},
            {"role": "user", "content": user},
        ],
        "response_format": {"type": "json_object"},
    });
    let resp = client
        .post(format!("{base_url}/v1/chat/completions"))
        .bearer_auth(api_key)
        .json(&body)
        .send()
        .await
        .context("HTTP send chat/completions")?;
    let status = resp.status();
    let text = resp.text().await.context("read response body")?;
    if !status.is_success() {
        bail!("chat/completions {status}: {}", truncate(&text, 400));
    }
    let parsed: Value = serde_json::from_str(&text)
        .with_context(|| format!("parse chat/completions response: {}", truncate(&text, 200)))?;
    let content = parsed["choices"][0]["message"]["content"]
        .as_str()
        .ok_or_else(|| anyhow!("missing choices[0].message.content"))?
        .to_string();
    Ok(content)
}

async fn call_planner(
    client: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
    user: &str,
) -> (Vec<String>, String) {
    match chat_completion(client, base_url, api_key, model, prompt, user).await {
        Ok(text) => match serde_json::from_str::<Value>(strip_json_fence(&text)) {
            Ok(obj) => {
                let rationale = obj
                    .get("rationale")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let lanes: Vec<String> = obj
                    .get("lanes_to_run")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .filter(|s| LANE_TO_ID.iter().any(|(name, _)| *name == s.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();
                if lanes.is_empty() {
                    fallback_planner("planner returned empty lane list")
                } else {
                    (normalize_planner_lanes(lanes), rationale)
                }
            }
            Err(e) => fallback_planner(&format!("planner JSON parse: {e}")),
        },
        Err(e) => fallback_planner(&format!("planner error: {e}")),
    }
}

fn fallback_planner(reason: &str) -> (Vec<String>, String) {
    eprintln!("  planner fallback: {reason}");
    let all = LANE_TO_ID.iter().map(|(n, _)| n.to_string()).collect();
    (all, format!("fallback: {reason}"))
}

/// Enforces the planner invariants the prompt documents but cannot guarantee:
/// dedupe, always include `logic`, ensure at least two lanes, and clamp to the
/// full lane set. A no-op when the planner already complies.
fn normalize_planner_lanes(lanes: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out: Vec<String> = lanes
        .into_iter()
        .filter(|l| seen.insert(l.clone()))
        .collect();
    if !out.iter().any(|l| l == "logic") {
        out.insert(0, "logic".to_string());
    }
    if out.len() < 2 {
        if let Some((name, _)) = LANE_TO_ID.iter().find(|(n, _)| !out.iter().any(|l| l == n)) {
            out.push(name.to_string());
        }
    }
    out.truncate(LANE_TO_ID.len());
    out
}

async fn run_lane(
    client: Client,
    base_url: String,
    api_key: String,
    model: String,
    lane_name: String,
    lane_id: String,
    prompt: String,
    user: String,
) -> (String, LaneRun, Vec<Finding>) {
    let t0 = Instant::now();
    if prompt.is_empty() {
        return (
            lane_name,
            LaneRun {
                ok: false,
                duration_s: 0.0,
                finding_count: 0,
                error: Some(format!("missing prompt for {lane_id}")),
            },
            vec![],
        );
    }
    let result = chat_completion(&client, &base_url, &api_key, &model, &prompt, &user).await;
    let duration = t0.elapsed().as_secs_f64();
    match result {
        Ok(text) => match parse_lane_findings(&text, &lane_name) {
            Ok(findings) => (
                lane_name,
                LaneRun {
                    ok: true,
                    duration_s: round2(duration),
                    finding_count: findings.len(),
                    error: None,
                },
                findings,
            ),
            Err(e) => (
                lane_name,
                LaneRun {
                    ok: false,
                    duration_s: round2(duration),
                    finding_count: 0,
                    error: Some(format!("parse: {e}")),
                },
                vec![],
            ),
        },
        Err(e) => (
            lane_name,
            LaneRun {
                ok: false,
                duration_s: round2(duration),
                finding_count: 0,
                error: Some(format!("http: {e}")),
            },
            vec![],
        ),
    }
}

fn parse_lane_findings(text: &str, lane_name: &str) -> Result<Vec<Finding>> {
    let v: Value = serde_json::from_str(strip_json_fence(text)).context("lane JSON parse")?;
    let arr: Vec<Value> = match v {
        Value::Array(arr) => arr,
        Value::Object(map) => {
            // The request forces response_format=json_object, so lanes return
            // an object envelope; accept the common keys models pick.
            for key in &[
                "findings", "issues", "items", "results", "comments", "review",
            ] {
                if let Some(Value::Array(arr)) = map.get(*key) {
                    return Ok(arr_to_findings(arr.clone(), lane_name));
                }
            }
            // An empty object is a legitimate "no findings"; a non-empty object
            // with no recognized array field is an unexpected envelope — surface
            // it as a parse error rather than silently reporting zero findings.
            if map.is_empty() {
                Vec::new()
            } else {
                bail!(
                    "lane returned an object with no recognized array field (keys: {:?})",
                    map.keys().collect::<Vec<_>>()
                );
            }
        }
        _ => Vec::new(),
    };
    Ok(arr_to_findings(arr, lane_name))
}

fn arr_to_findings(arr: Vec<Value>, lane_name: &str) -> Vec<Finding> {
    arr.into_iter()
        .filter_map(|v| serde_json::from_value::<Finding>(v).ok())
        .filter(|f| !f.file_line.is_empty())
        .map(|mut f| {
            f.lanes = vec![lane_name.to_string()];
            f
        })
        .collect()
}

async fn call_filter(
    client: &Client,
    base_url: &str,
    api_key: &str,
    model: &str,
    prompt: &str,
    user: &str,
) -> Result<HashSet<String>> {
    let text = chat_completion(client, base_url, api_key, model, prompt, user).await?;
    let v: Value = serde_json::from_str(strip_json_fence(&text)).context("filter JSON parse")?;
    let mut keep = HashSet::new();
    if let Some(arr) = v.get("keep").and_then(|x| x.as_array()) {
        for entry in arr {
            if let Some(s) = entry.as_str() {
                keep.insert(s.trim().to_string());
            }
        }
    }
    Ok(keep)
}

fn strip_json_fence(text: &str) -> &str {
    let t = text.trim();
    if !t.starts_with("```") {
        return t;
    }
    let mut rest = &t[3..];
    if let Some(idx) = rest.find('\n') {
        if rest[..idx].starts_with("json") {
            rest = &rest[idx + 1..];
        }
    }
    if let Some(idx) = rest.rfind("```") {
        return rest[..idx].trim();
    }
    t
}

fn render_markdown(
    findings: &[Finding],
    lanes: &[String],
    rationale: &str,
    dropped_by_filter: usize,
    filter_failed: bool,
) -> String {
    let blockers: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.severity.eq_ignore_ascii_case("BLOCKER"))
        .collect();
    let shouldfix: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.severity.eq_ignore_ascii_case("SHOULD-FIX"))
        .collect();
    let nits: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.severity.eq_ignore_ascii_case("NIT"))
        .collect();

    let mut out = String::new();
    out.push_str("# Ultrareview Report\n\n");
    if filter_failed {
        out.push_str("> ⚠️ Filter stage failed — findings below are unfiltered and may include false positives.\n\n");
    }
    out.push_str(&format!(
        "**Lanes run:** {} ({})\n",
        lanes.len(),
        truncate(rationale, 120)
    ));
    out.push_str(&format!(
        "**Total findings:** {} kept after filter (filter dropped {})\n",
        findings.len(),
        dropped_by_filter
    ));
    out.push_str(&format!(
        "  Blockers: {}, Should-fix: {}, Nits: {}\n\n",
        blockers.len(),
        shouldfix.len(),
        nits.len()
    ));

    for (title, group) in [
        ("Blockers", &blockers),
        ("Should-fix", &shouldfix),
        ("Nits", &nits),
    ] {
        out.push_str(&format!("## {title}\n"));
        if group.is_empty() {
            out.push_str("- (none)\n");
        } else {
            for f in group {
                let lane_tag = if f.lanes.is_empty() {
                    "?".to_string()
                } else {
                    f.lanes.join(", ")
                };
                out.push_str(&format!(
                    "- [{}] ({}) [conf={:.2}] {}\n",
                    f.file_line, lane_tag, f.confidence, f.title
                ));
                if !f.body.is_empty() {
                    out.push_str(&format!(
                        "  {}\n",
                        truncate(&f.body, 500).replace('\n', " ")
                    ));
                }
                if !f.fix.is_empty() {
                    out.push_str(&format!("  Fix: {}\n", truncate(&f.fix, 200)));
                }
            }
        }
        out.push('\n');
    }
    out.push_str("## Executive summary\n");
    out.push_str(&format!(
        "{} findings retained from {} planner-selected lanes after filter pass.\n",
        findings.len(),
        lanes.len()
    ));
    out
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}
