#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { hasOpenRouterCredential } from "../lib/openrouter-auth.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const namespace = String(args.namespace ?? `plan-verify-${Date.now()}`);
const outDir = path.resolve(repoRoot, String(args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace)));
const guiflowRoot = path.resolve(String(args["guiflow-root"] ?? path.join(repoRoot, "..", "guiflow-paper")));
const requireRealOpenRouter = Boolean(args["require-real-openrouter"]);
const runRealOpenRouter = Boolean(args["real-openrouter"] || requireRealOpenRouter);
const skipAgentflow = Boolean(args["skip-agentflow"]);
const skipGuiflow = Boolean(args["skip-guiflow"]);

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });

const steps = [];
const envSummary = {
  codex: commandExists("codex"),
  agentflow: commandExists("agentflow"),
  guiflowRoot: fs.existsSync(guiflowRoot),
  openRouterKeyPresent: Boolean(process.env.OPENROUTER_API_KEY),
  openRouterKeyFilePresent: hasReadableKeyFile(process.env.PUFFER_OPENROUTER_API_KEY_FILE),
  openRouterCredentialPresent: hasOpenRouterCredential()
};

run("syntax", "Static syntax checks", [
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-explorer.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-reviewer.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-smoke.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-selftest.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-verify-plan.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/lib/admission-gate.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/lib/evidence-index.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/lib/scheduler.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/lib/prompt-evolution.mjs",
  "node --check apps/puffer-desktop/tests/fuzz/lib/openrouter-auth.mjs",
  "python3 -m py_compile apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py",
  "bash -n apps/puffer-desktop/tests/fuzz/run_openrouter_campaign_loop.sh"
].join(" && "));

run("metadata", "Metadata validation and gate selftest", [
  "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate",
  "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-selftest.mjs"
].join(" && "));

run("evidence-replay", "Bounded replay emits evidence index", [
  `rm -rf ${sh(path.join(fuzzRoot, ".runs", `${namespace}-evidence`))}`,
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds chat-turn-race --shard chat-composer-send --limit 1 --attempts 1 --timeout 90 --rng-seed ${sh(`${namespace}-evidence`)} --namespace ${sh(`${namespace}-evidence`)}`,
  `test "$(jq '.evidence_index | length' ${sh(path.join(fuzzRoot, ".runs", `${namespace}-evidence`, "bounded-replay-report.json"))})" -gt 0`
].join(" && "), { timeout: 180_000 });

run("no-key-triage", "No-signal triage works without OpenRouter key", [
  `OPENROUTER_API_KEY= PUFFER_OPENROUTER_API_KEY_FILE= node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace ${sh(`${namespace}-evidence`)} --shard chat-composer-send --seed chat-turn-race --out ${sh(path.join(fuzzRoot, ".runs", `${namespace}-evidence`, "findings.md"))}`,
  `test "$(jq -r '.disposition' ${sh(path.join(fuzzRoot, ".runs", `${namespace}-evidence`, "verdict-gate.json"))})" = "dismissed"`
].join(" && "));

run("reviewer-aggregate", "Candidate reviewer artifacts appear in aggregate", [
  `rm -rf ${sh(path.join(fuzzRoot, ".runs", `${namespace}-reviewer-candidate`))}`,
  `mkdir -p ${sh(path.join(fuzzRoot, ".runs", `${namespace}-reviewer-candidate`))}`,
  `node -e ${sh(syntheticReviewerShardScript(namespace, fuzzRoot))}`,
  `PUFFER_OPENROUTER_NAMESPACE=${sh(namespace)} node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs`,
  `cp apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign/puffer_openrouter_fuzz_report.json ${sh(path.join(outDir, "reviewer-aggregate.json"))}`,
  `test "$(jq '.summary.candidateVerdicts' ${sh(path.join(outDir, "reviewer-aggregate.json"))})" -ge 1`,
  `test "$(jq '.summary.reviewerReportsPresent' ${sh(path.join(outDir, "reviewer-aggregate.json"))})" -ge 1`,
  `test "$(jq '.summary.reviewerHumanQueueDecisions' ${sh(path.join(outDir, "reviewer-aggregate.json"))})" -ge 1`,
  `test "$(jq '.summary.evidenceByType.action' ${sh(path.join(outDir, "reviewer-aggregate.json"))})" -ge 1`,
  `test "$(jq '.summary.gateFailureReasons | length' ${sh(path.join(outDir, "reviewer-aggregate.json"))})" -ge 1`,
  `test "$(jq '.blockerSummary.topBlockers | length' ${sh(path.join(outDir, "reviewer-aggregate.json"))})" -ge 1`,
  `jq -e '.blockerSummary.topBlockers[] | select(.type == "candidate-review")' ${sh(path.join(outDir, "reviewer-aggregate.json"))} >/dev/null`
].join(" && "));

run("evolution", "Tree evolution and bridge-aware scheduling", [
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-tree --out ${sh(path.join(outDir, "evolved.md"))} --json-out ${sh(path.join(outDir, "evolved.json"))} --starvation-floor 1`,
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit 8 --bridge-quota 2 --namespace ${sh(`${namespace}-evolved`)} --evolution ${sh(path.join(outDir, "evolved.json"))} --format json > ${sh(path.join(outDir, "schedule.json"))}`,
  `test "$(jq '.selectedShardIds | length' ${sh(path.join(outDir, "schedule.json"))})" -eq 8`,
  `test "$(jq '.normalSelectedCount' ${sh(path.join(outDir, "schedule.json"))})" -eq 6`,
  `test "$(jq '.bridgeSelectedCount' ${sh(path.join(outDir, "schedule.json"))})" -eq 2`,
  `test "$(jq -r '.selectionPolicy.mode' ${sh(path.join(outDir, "schedule.json"))})" = "normal-first-bridge-quota"`,
  `test "$(jq '.selectionPolicy.bridgeQuota' ${sh(path.join(outDir, "schedule.json"))})" -eq 2`,
  `test "$(jq -r '.items[0].bridge // false' ${sh(path.join(outDir, "schedule.json"))})" = "false"`
].join(" && "));

run("evolution-policy", "Synthetic split/demote/starvation evolution policy", [
  `rm -rf ${sh(path.join(outDir, "evolution-policy-shards"))}`,
  `mkdir -p ${sh(path.join(outDir, "evolution-policy-shards"))}`,
  `node -e ${sh(syntheticEvolutionPolicyScript(outDir))}`,
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-tree --shard-dir ${sh(path.join(outDir, "evolution-policy-shards"))} --feedback-ledger ${sh(path.join(outDir, "evolution-policy-feedback.json"))} --out ${sh(path.join(outDir, "evolution-policy.md"))} --json-out ${sh(path.join(outDir, "evolution-policy.json"))} --starvation-floor 1`,
  `jq -e '.shards["synthetic-hot-parent"].actions | index("split")' ${sh(path.join(outDir, "evolution-policy.json"))} >/dev/null`,
  `jq -e '.shards["synthetic-cold-leaf"].actions | index("demote")' ${sh(path.join(outDir, "evolution-policy.json"))} >/dev/null`,
  `jq -e '.shards["synthetic-starved-leaf"].actions | index("force-starvation-floor")' ${sh(path.join(outDir, "evolution-policy.json"))} >/dev/null`
].join(" && "));

run("bridge-replay", "Two bridge shards replay through evidence path", [
  `rm -rf ${sh(path.join(fuzzRoot, ".runs", `${namespace}-bridge-chat`))} ${sh(path.join(fuzzRoot, ".runs", `${namespace}-bridge-model`))}`,
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds chat-turn-race --shard bridge-chat-permission-session-reload --limit 1 --attempts 1 --timeout 90 --rng-seed ${sh(`${namespace}-bridge-chat`)} --namespace ${sh(`${namespace}-bridge-chat`)}`,
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds provider-auth-model-race --shard bridge-new-agent-settings-model --limit 1 --attempts 1 --timeout 90 --rng-seed ${sh(`${namespace}-bridge-model`)} --namespace ${sh(`${namespace}-bridge-model`)}`,
  `test "$(jq '.evidence_index | length' ${sh(path.join(fuzzRoot, ".runs", `${namespace}-bridge-chat`, "bounded-replay-report.json"))})" -gt 0`,
  `test "$(jq '.evidence_index | length' ${sh(path.join(fuzzRoot, ".runs", `${namespace}-bridge-model`, "bounded-replay-report.json"))})" -gt 0`
].join(" && "), { timeout: 240_000 });

if (skipGuiflow) {
  skip("guiflow-smoke", "GUIFlow smoke benchmark skipped by flag");
} else if (!envSummary.guiflowRoot) {
  skip("guiflow-smoke", `GUIFlow root missing: ${guiflowRoot}`, { required: true });
} else {
  run("guiflow-smoke", "GUIFlow buggy/fixed smoke benchmark", [
    `node apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-smoke.mjs --root ${sh(guiflowRoot)} --suite ${sh(path.join(guiflowRoot, "benchmarks", "smoke_suite.json"))} --out ${sh(path.join(outDir, "guiflow-smoke"))}`,
    `test "$(jq '.summary.buggyAdmitted' ${sh(path.join(outDir, "guiflow-smoke", "guiflow-smoke-report.json"))})" -ge 1`,
    `test "$(jq '.summary.fixedAdmitted' ${sh(path.join(outDir, "guiflow-smoke", "guiflow-smoke-report.json"))})" -eq 0`,
    `test "$(jq '[.results[].artifacts.screenshot | select(length > 0)] | length' ${sh(path.join(outDir, "guiflow-smoke", "guiflow-smoke-report.json"))})" -eq "$(jq '.summary.total' ${sh(path.join(outDir, "guiflow-smoke", "guiflow-smoke-report.json"))})"`
  ].join(" && "), { timeout: 120_000 });
}

if (skipAgentflow) {
  skip("agentflow-offline", "AgentFlow offline campaign skipped by flag");
} else if (!envSummary.agentflow || !envSummary.codex) {
  skip("agentflow-offline", "AgentFlow or Codex is unavailable", { required: true });
} else {
  run("agentflow-offline", "Codex-planned AgentFlow offline campaign", [
    `export PUFFER_OPENROUTER_OFFLINE_SMOKE=1`,
    `export PUFFER_OPENROUTER_NAMESPACE=${sh(`${namespace}-agentflow`)}`,
    `export PUFFER_OPENROUTER_SHARD_LIMIT=1`,
    `export PUFFER_OPENROUTER_CONCURRENCY=1`,
    `export PUFFER_OPENROUTER_CASES=1`,
    `export PUFFER_OPENROUTER_CODEX_PLAN_TIMEOUT_SECONDS=180`,
    `export PUFFER_OPENROUTER_PLAN_NODE_TIMEOUT_SECONDS=210`,
    `export PUFFER_OPENROUTER_TIMEOUT_SECONDS=240`,
    `export PUFFER_OPENROUTER_FEEDBACK_LEDGER=${sh(path.join(outDir, "agentflow-feedback-ledger.json"))}`,
    `export PUFFER_OPENROUTER_COVERAGE_LEDGER=${sh(path.join(outDir, "agentflow-coverage-ledger.json"))}`,
    `agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py --runs-dir ${sh(path.join(outDir, "agentflow-local-runs"))} --output summary`,
    `PUFFER_OPENROUTER_NAMESPACE=${sh(`${namespace}-agentflow`)} node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs`,
    `cp apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign/puffer_openrouter_fuzz_report.json ${sh(path.join(outDir, "agentflow-aggregate.json"))}`,
    `test "$(jq '.summary.missingReplayReports' ${sh(path.join(outDir, "agentflow-aggregate.json"))})" -eq 0`,
    `test "$(jq '.summary.verdictReportsPresent' ${sh(path.join(outDir, "agentflow-aggregate.json"))})" -ge 1`,
    `rg -q "OPENROUTER_PLAN_OK codex" ${sh(path.join(outDir, "agentflow-local-runs"))}`
  ].join(" && "), { timeout: 300_000 });
}

if (skipAgentflow) {
  skip("scale-readiness", "AgentFlow scale-readiness check skipped by flag");
} else if (!envSummary.agentflow) {
  skip("scale-readiness", "AgentFlow is unavailable", { required: true });
} else {
  run("scale-readiness", "50-shard synthetic fanout and aggregate scale gate", [
    `export PUFFER_OPENROUTER_OFFLINE_SMOKE=1`,
    `export PUFFER_OPENROUTER_SYNTHETIC_SHARDS=1`,
    `export PUFFER_OPENROUTER_FORCE_FALLBACK_PLAN=1`,
    `export PUFFER_OPENROUTER_NAMESPACE=${sh(`${namespace}-scale50`)}`,
    `export PUFFER_OPENROUTER_SHARD_LIMIT=50`,
    `export PUFFER_OPENROUTER_CONCURRENCY=10`,
    `export PUFFER_OPENROUTER_CASES=1`,
    `export PUFFER_OPENROUTER_TIMEOUT_SECONDS=180`,
    `export PUFFER_OPENROUTER_PLAN_NODE_TIMEOUT_SECONDS=30`,
    `export PUFFER_OPENROUTER_FEEDBACK_LEDGER=${sh(path.join(outDir, "scale-feedback-ledger.json"))}`,
    `export PUFFER_OPENROUTER_COVERAGE_LEDGER=${sh(path.join(outDir, "scale-coverage-ledger.json"))}`,
    `rm -rf ${sh(path.join(outDir, "scale-agentflow-local-runs"))} ${sh(path.join(outDir, "scale-feedback-ledger.json"))} ${sh(path.join(outDir, "scale-coverage-ledger.json"))}`,
    `agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py --runs-dir ${sh(path.join(outDir, "scale-agentflow-local-runs"))} --output summary`,
    `PUFFER_OPENROUTER_NAMESPACE=${sh(`${namespace}-scale50`)} node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs`,
    `cp apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign/puffer_openrouter_fuzz_report.json ${sh(path.join(outDir, "scale-aggregate.json"))}`,
    `test "$(jq '.summary.shards' ${sh(path.join(outDir, "scale-aggregate.json"))})" -eq 50`,
    `test "$(jq '.summary.missingReplayReports' ${sh(path.join(outDir, "scale-aggregate.json"))})" -eq 0`,
    `test "$(jq '.summary.verdictReportsPresent' ${sh(path.join(outDir, "scale-aggregate.json"))})" -eq 50`,
    `test "$(jq '.summary.evidenceEntries' ${sh(path.join(outDir, "scale-aggregate.json"))})" -eq 50`,
    `test "$(jq '.summary.evidenceByType.action' ${sh(path.join(outDir, "scale-aggregate.json"))})" -eq 50`,
    `test "$(jq '.blockerSummary.ready' ${sh(path.join(outDir, "scale-aggregate.json"))})" = "true"`,
    `test "$(jq '.blockerSummary.topBlockers | length' ${sh(path.join(outDir, "scale-aggregate.json"))})" -eq 0`,
    `test "$(jq '.runs | length' ${sh(path.join(outDir, "scale-feedback-ledger.json"))})" -eq 50`,
    `test "$(jq '[.runs[].namespace] | unique | length' ${sh(path.join(outDir, "scale-feedback-ledger.json"))})" -eq 50`,
    `test "$(jq '.runtimeCoverage.shards | length' ${sh(path.join(outDir, "scale-coverage-ledger.json"))})" -ge 1`
  ].join(" && "), { timeout: 420_000 });
}

if (!runRealOpenRouter) {
  skip("openrouter-real", "Real OpenRouter campaign not requested", { external: true });
} else if (!envSummary.openRouterCredentialPresent) {
  skip("openrouter-real", "OPENROUTER_API_KEY or PUFFER_OPENROUTER_API_KEY_FILE missing", { required: requireRealOpenRouter, external: true });
} else {
  run("openrouter-real", "Real OpenRouter small-model campaign", [
    `export PUFFER_OPENROUTER_NAMESPACE=${sh(`${namespace}-real-openrouter`)}`,
    `export PUFFER_OPENROUTER_SHARD_LIMIT=1`,
    `export PUFFER_OPENROUTER_CONCURRENCY=1`,
    `export PUFFER_OPENROUTER_CASES=1`,
    `export PUFFER_OPENROUTER_FEEDBACK_LEDGER=${sh(path.join(outDir, "real-openrouter-feedback-ledger.json"))}`,
    `export PUFFER_OPENROUTER_COVERAGE_LEDGER=${sh(path.join(outDir, "real-openrouter-coverage-ledger.json"))}`,
    `agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py --runs-dir ${sh(path.join(outDir, "real-openrouter-agentflow-runs"))} --output summary`,
    `PUFFER_OPENROUTER_NAMESPACE=${sh(`${namespace}-real-openrouter`)} node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs`,
    `cp apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign/puffer_openrouter_fuzz_report.json ${sh(path.join(outDir, "real-openrouter-aggregate.json"))}`,
    `test "$(jq '.summary.missingReplayReports' ${sh(path.join(outDir, "real-openrouter-aggregate.json"))})" -eq 0`
  ].join(" && "), { timeout: 420_000 });
}

const summary = summarize();
const phaseCoverage = buildPhaseCoverage();
const report = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  outDir: relative(outDir),
  environment: envSummary,
  summary,
  phaseCoverage,
  steps
};
fs.writeFileSync(path.join(outDir, "plan-verification.json"), `${JSON.stringify(report, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "plan-verification.md"), formatMarkdown(report));
process.stdout.write(`PUFFER_UIUX_PLAN_VERIFY ${summary.overallStatus} ${relative(path.join(outDir, "plan-verification.json"))}\n`);
if (summary.overallStatus !== "passed") process.exitCode = 1;

function run(id, title, command, options = {}) {
  const startedAt = new Date().toISOString();
  const stdoutPath = path.join(outDir, `${id}.stdout.log`);
  const stderrPath = path.join(outDir, `${id}.stderr.log`);
  const result = spawnSync("bash", ["-lc", command], {
    cwd: repoRoot,
    env: { ...process.env },
    encoding: "utf8",
    timeout: options.timeout ?? 60_000,
    maxBuffer: 50 * 1024 * 1024
  });
  fs.writeFileSync(stdoutPath, result.stdout ?? "");
  fs.writeFileSync(stderrPath, result.stderr ?? "");
  const passed = result.status === 0;
  steps.push({
    id,
    title,
    status: passed ? "passed" : "failed",
    exitCode: result.status,
    signal: result.signal,
    startedAt,
    finishedAt: new Date().toISOString(),
    required: options.required !== false,
    external: Boolean(options.external),
    stdout: relative(stdoutPath),
    stderr: relative(stderrPath)
  });
}

function skip(id, title, options = {}) {
  steps.push({
    id,
    title,
    status: "skipped",
    required: Boolean(options.required),
    external: Boolean(options.external),
    startedAt: new Date().toISOString(),
    finishedAt: new Date().toISOString()
  });
}

function summarize() {
  const localRequired = steps.filter((step) => step.required && !step.external);
  const externalRequired = steps.filter((step) => step.required && step.external);
  const localStatus = localRequired.every((step) => step.status === "passed") ? "passed" : "failed";
  const requiredExternalStatus = externalRequired.length === 0
    ? "not-required"
    : externalRequired.every((step) => step.status === "passed")
      ? "passed"
      : "failed";
  const overallStatus = localStatus === "passed" && requiredExternalStatus !== "failed"
    ? "passed"
    : "failed";
  return {
    total: steps.length,
    passed: steps.filter((step) => step.status === "passed").length,
    failed: steps.filter((step) => step.status === "failed").length,
    skipped: steps.filter((step) => step.status === "skipped").length,
    overallStatus,
    localStatus,
    requiredExternalStatus
  };
}

function buildPhaseCoverage() {
  const phaseDefinitions = [
    {
      id: "phase-0",
      title: "Baseline Freeze and Smoke",
      localSteps: ["metadata", "agentflow-offline"],
      evidence: "Validates baseline metadata, selftest invariants, and Codex-planned AgentFlow orchestration without requiring network access.",
      caveat: realOpenRouterCaveat()
    },
    {
      id: "phase-1",
      title: "Evidence Index and Structured Verdict",
      localSteps: ["metadata", "evidence-replay", "no-key-triage"],
      evidence: "Checks evidence_index emission and strict JSON no-signal verdict generation."
    },
    {
      id: "phase-2",
      title: "5-Clause Citation Gate",
      localSteps: ["metadata", "no-key-triage"],
      evidence: "Selftest covers admitted verdicts, malformed verdicts, hallucinated evidence ids, and missing predicate rejection."
    },
    {
      id: "phase-3",
      title: "Candidate Ledger and Reviewer Agent",
      localSteps: ["metadata", "reviewer-aggregate"],
      evidence: "Aggregates a synthetic predicate-missing candidate plus reviewer human_queue decision."
    },
    {
      id: "phase-4",
      title: "Bridge Shards",
      localSteps: ["evolution", "bridge-replay"],
      evidence: "Schedules bridge shards after normal shards with explicit bridge counts, then replays two bridge shards through the same evidence path."
    },
    {
      id: "phase-5",
      title: "Tree Resplit, Demote, and Starvation Floor",
      localSteps: ["evolution", "evolution-policy"],
      evidence: "Checks deterministic evolved scheduling plus synthetic split, demote, and starvation-floor decisions."
    },
    {
      id: "phase-6",
      title: "Codex-Planned Campaign Integration",
      localSteps: ["agentflow-offline", "scale-readiness"],
      externalSteps: ["openrouter-real"],
      evidence: "Verifies Codex planner wiring in offline mode plus 50-shard synthetic fanout, isolated ledgers, verdict artifacts, and aggregate reporting; real small-model explorer coverage is tracked as an external gate.",
      caveat: realOpenRouterCaveat()
    },
    {
      id: "phase-7",
      title: "Puffer Internal Validation",
      localSteps: ["syntax", "metadata", "evidence-replay", "no-key-triage", "bridge-replay"],
      evidence: "Combines syntax, metadata, replay evidence, no-finding triage, and bridge replay checks against Puffer."
    },
    {
      id: "phase-8",
      title: "GUIFlow Benchmark Adapter",
      localSteps: ["guiflow-smoke"],
      evidence: "Requires buggy GUIFlow smoke app to admit a predicate-backed finding, fixed app to admit none, and every case to export screenshot/verdict/gate artifacts."
    },
    {
      id: "phase-9",
      title: "Reporting and Handoff",
      localSteps: ["syntax", "reviewer-aggregate", "agentflow-offline", "scale-readiness"],
      evidence: "Confirms machine-readable reports include aggregate reviewer counts, planner campaign artifacts, and 50-shard scale summaries."
    }
  ];

  return phaseDefinitions.map((definition) => {
    const localStepStatuses = collectStepStatuses(definition.localSteps ?? []);
    const externalStepStatuses = collectStepStatuses(definition.externalSteps ?? []);
    const missingLocalSteps = localStepStatuses.filter((step) => step.status === "missing");
    const failedLocalSteps = localStepStatuses.filter((step) => step.status === "failed" || step.status === "skipped-required");
    const localStatus = missingLocalSteps.length > 0 || failedLocalSteps.length > 0 ? "failed" : "passed";
    const externalStatus = summarizeExternalPhaseStatus(externalStepStatuses);
    const status = localStatus === "failed"
      ? "failed"
      : externalStatus === "failed"
        ? "blocked_external"
        : "passed";
    return {
      id: definition.id,
      title: definition.title,
      status,
      localStatus,
      externalStatus,
      localSteps: localStepStatuses,
      externalSteps: externalStepStatuses,
      evidence: definition.evidence,
      caveat: definition.caveat
    };
  });
}

function collectStepStatuses(stepIds) {
  return stepIds.map((stepId) => {
    const step = steps.find((candidate) => candidate.id === stepId);
    if (!step) {
      return { id: stepId, status: "missing" };
    }
    if (step.status === "skipped" && step.required) {
      return { id: stepId, status: "skipped-required" };
    }
    return {
      id: step.id,
      status: step.status,
      required: step.required,
      external: step.external
    };
  });
}

function summarizeExternalPhaseStatus(stepStatuses) {
  if (stepStatuses.length === 0) return "not-applicable";
  if (stepStatuses.every((step) => step.status === "passed")) return "passed";
  if (stepStatuses.some((step) => step.status === "failed" || step.status === "skipped-required")) return "failed";
  if (stepStatuses.some((step) => step.status === "skipped")) return "not-run";
  return "unknown";
}

function realOpenRouterCaveat() {
  if (runRealOpenRouter && envSummary.openRouterCredentialPresent) {
    return "Real OpenRouter campaign was requested and key-backed execution was available.";
  }
  if (runRealOpenRouter) {
    return "Real OpenRouter campaign was requested but OPENROUTER_API_KEY or PUFFER_OPENROUTER_API_KEY_FILE was missing.";
  }
  return "Real OpenRouter small-model campaign was not requested in this local verifier run.";
}

function formatMarkdown(report) {
  const lines = [
    "# Puffer UI/UX Fuzz Plan Verification",
    "",
    `Generated: ${report.generatedAt}`,
    `Namespace: ${report.namespace}`,
    `Output: ${report.outDir}`,
    "",
    "## Summary",
    "",
    `- Overall status: ${report.summary.overallStatus}`,
    `- Local status: ${report.summary.localStatus}`,
    `- Required external status: ${report.summary.requiredExternalStatus}`,
    `- Passed: ${report.summary.passed}`,
    `- Failed: ${report.summary.failed}`,
    `- Skipped: ${report.summary.skipped}`,
    "",
    "## Environment",
    "",
    `- Codex: ${report.environment.codex ? "present" : "missing"}`,
    `- AgentFlow: ${report.environment.agentflow ? "present" : "missing"}`,
    `- GUIFlow root: ${report.environment.guiflowRoot ? "present" : "missing"}`,
    `- OpenRouter key: ${report.environment.openRouterKeyPresent ? "present" : "missing"}`,
    `- OpenRouter key file: ${report.environment.openRouterKeyFilePresent ? "present" : "missing"}`,
    `- OpenRouter credential: ${report.environment.openRouterCredentialPresent ? "present" : "missing"}`,
    "",
    "## Phase Coverage",
    ""
  ];
  for (const phase of report.phaseCoverage) {
    lines.push(`- ${phase.id} ${phase.title}: ${phase.status} (external: ${phase.externalStatus})`);
    lines.push(`  Evidence: ${phase.evidence}`);
    lines.push(`  Local steps: ${formatStepList(phase.localSteps)}`);
    if (phase.externalSteps.length > 0) {
      lines.push(`  External steps: ${formatStepList(phase.externalSteps)}`);
    }
    if (phase.caveat) {
      lines.push(`  Caveat: ${phase.caveat}`);
    }
  }
  lines.push(
    "",
    "## Steps",
    ""
  );
  for (const step of report.steps) {
    lines.push(`- ${step.id}: ${step.status}${step.external ? " (external)" : ""}${step.required ? "" : " (optional)"}`);
  }
  return `${lines.join("\n")}\n`;
}

function formatStepList(stepStatuses) {
  if (stepStatuses.length === 0) return "none";
  return stepStatuses.map((step) => `${step.id}=${step.status}`).join(", ");
}

function commandExists(name) {
  return spawnSync("bash", ["-lc", `command -v ${sh(name)} >/dev/null 2>&1`], {
    cwd: repoRoot,
    encoding: "utf8"
  }).status === 0;
}

function hasReadableKeyFile(filePath) {
  if (!filePath) return false;
  return fs.existsSync(filePath) && fs.statSync(filePath).size > 0;
}

function syntheticReviewerShardScript(namespace, fuzzRootPath) {
  const dir = path.join(fuzzRootPath, ".runs", `${namespace}-reviewer-candidate`);
  return `
const fs = require("node:fs");
const dir = ${JSON.stringify(dir)};
fs.writeFileSync(dir + "/bounded-replay-report.json", JSON.stringify({
  version: 1,
  namespace: ${JSON.stringify(`${namespace}-reviewer-candidate`)},
  summary: {
    total: 1,
    passed: 0,
    stableFailed: 1,
    newCandidateFindings: 1,
    nonPassingFailures: 1,
    actionableFailures: 1,
    byClassification: { "needs-manual-triage": 1 }
  },
  findings: [],
  evidence_index: [
    { id: "ev-action-0001", type: "action", byte_span: [0, 10], sha256: "abc", value: "click", metadata: {} }
  ]
}, null, 2) + "\\n");
fs.writeFileSync(dir + "/bounded-replay-report.md", "# synthetic reviewer candidate\\n");
fs.writeFileSync(dir + "/findings.md", "# synthetic reviewer candidate\\n");
fs.writeFileSync(dir + "/verdict.json", JSON.stringify({
  version: 1,
  decision: "candidate",
  title: "Synthetic predicate-missing candidate",
  severity: "P2",
  area: "selftest",
  shard: "selftest-shard",
  source_run: ${JSON.stringify(`${namespace}-reviewer-candidate`)},
  primary_cause: { id: "ev-action-0001", type: "action", quote_hash: "abc" },
  citations: [{ id: "ev-action-0001", type: "action", quote_hash: "abc" }],
  expected: "candidate appears in aggregate",
  actual: "candidate appears with reviewer decision",
  impact: "aggregate reviewer counters are visible",
  repro: ["synthetic"],
  notes: "synthetic"
}, null, 2) + "\\n");
fs.writeFileSync(dir + "/verdict-gate.json", JSON.stringify({
  version: 1,
  disposition: "candidate",
  passed: false,
  candidateEligible: true,
  failureReasons: ["primary-cause-is-predicate: primary cause ev-action-0001 resolves to action"]
}, null, 2) + "\\n");
fs.writeFileSync(dir + "/reviewer.json", JSON.stringify({
  version: 1,
  decision: "human_queue",
  confidence: 0.5,
  reason: "synthetic aggregate reviewer smoke",
  cited_evidence: ["ev-action-0001"],
  notes: "synthetic"
}, null, 2) + "\\n");
`;
}

function syntheticEvolutionPolicyScript(outputDir) {
  const shardDir = path.join(outputDir, "evolution-policy-shards");
  const feedbackPath = path.join(outputDir, "evolution-policy-feedback.json");
  return `
const fs = require("node:fs");
const shardDir = ${JSON.stringify(shardDir)};
const feedbackPath = ${JSON.stringify(feedbackPath)};
const shard = (id, startNode) => ({
  id,
  title: id,
  seed: "chat-turn-race",
  startNode,
  ownedNodes: [startNode],
  allowedSetupNodes: [],
  ownedCoverage: [],
  allowedAsyncEvents: [],
  invariants: []
});
fs.writeFileSync(shardDir + "/synthetic-hot-parent.json", JSON.stringify(shard("synthetic-hot-parent", "chat"), null, 2) + "\\n");
fs.writeFileSync(shardDir + "/synthetic-cold-leaf.json", JSON.stringify(shard("synthetic-cold-leaf", "workspace/landing-search"), null, 2) + "\\n");
fs.writeFileSync(shardDir + "/synthetic-starved-leaf.json", JSON.stringify(shard("synthetic-starved-leaf", "browser/address-navigation"), null, 2) + "\\n");
fs.writeFileSync(feedbackPath, JSON.stringify({
  version: 1,
  runs: [
    { shardId: "synthetic-cold-leaf", recordedAt: "2026-05-25T00:00:00.000Z", total: 1, passed: 1, stableFailed: 0, actionableFailures: 0, newCandidateFindings: 0, coveredTags: [] },
    { shardId: "synthetic-cold-leaf", recordedAt: "2026-05-25T00:01:00.000Z", total: 1, passed: 1, stableFailed: 0, actionableFailures: 0, newCandidateFindings: 0, coveredTags: [] },
    { shardId: "synthetic-hot-parent", recordedAt: "2026-05-25T00:02:00.000Z", total: 5, passed: 0, stableFailed: 5, actionableFailures: 5, newCandidateFindings: 2, coveredTags: ["route:chat-composer", "control:chat.send"] },
    { shardId: "synthetic-hot-parent", recordedAt: "2026-05-25T00:03:00.000Z", total: 5, passed: 0, stableFailed: 5, actionableFailures: 5, newCandidateFindings: 2, coveredTags: ["route:chat-composer", "control:chat.send"] }
  ],
  shards: {},
  notes: []
}, null, 2) + "\\n");
`;
}

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (!key.startsWith("--")) continue;
    const name = key.slice(2);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) {
      parsed[name] = true;
    } else {
      parsed[name] = value;
      index += 1;
    }
  }
  return parsed;
}

function sh(value) {
  return `'${String(value).replaceAll("'", "'\\''")}'`;
}

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
