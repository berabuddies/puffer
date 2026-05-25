#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

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
  openRouterKeyPresent: Boolean(process.env.OPENROUTER_API_KEY)
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
  `OPENROUTER_API_KEY= node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace ${sh(`${namespace}-evidence`)} --shard chat-composer-send --seed chat-turn-race --out ${sh(path.join(fuzzRoot, ".runs", `${namespace}-evidence`, "findings.md"))}`,
  `test "$(jq -r '.disposition' ${sh(path.join(fuzzRoot, ".runs", `${namespace}-evidence`, "verdict-gate.json"))})" = "dismissed"`
].join(" && "));

run("evolution", "Tree evolution and bridge-aware scheduling", [
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-tree --out ${sh(path.join(outDir, "evolved.md"))} --json-out ${sh(path.join(outDir, "evolved.json"))} --starvation-floor 1`,
  `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit 2 --namespace ${sh(`${namespace}-evolved`)} --evolution ${sh(path.join(outDir, "evolved.json"))} --format json > ${sh(path.join(outDir, "schedule.json"))}`,
  `test "$(jq '.selectedShardIds | length' ${sh(path.join(outDir, "schedule.json"))})" -eq 2`
].join(" && "));

if (skipGuiflow) {
  skip("guiflow-smoke", "GUIFlow smoke benchmark skipped by flag");
} else if (!envSummary.guiflowRoot) {
  skip("guiflow-smoke", `GUIFlow root missing: ${guiflowRoot}`, { required: true });
} else {
  run("guiflow-smoke", "GUIFlow buggy/fixed smoke benchmark", [
    `node apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-smoke.mjs --root ${sh(guiflowRoot)} --suite ${sh(path.join(guiflowRoot, "benchmarks", "smoke_suite.json"))} --out ${sh(path.join(outDir, "guiflow-smoke"))}`,
    `test "$(jq '.summary.buggyAdmitted' ${sh(path.join(outDir, "guiflow-smoke", "guiflow-smoke-report.json"))})" -ge 1`,
    `test "$(jq '.summary.fixedAdmitted' ${sh(path.join(outDir, "guiflow-smoke", "guiflow-smoke-report.json"))})" -eq 0`
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

if (!runRealOpenRouter) {
  skip("openrouter-real", "Real OpenRouter campaign not requested", { external: true });
} else if (!envSummary.openRouterKeyPresent) {
  skip("openrouter-real", "OPENROUTER_API_KEY missing", { required: requireRealOpenRouter, external: true });
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
const report = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  outDir: relative(outDir),
  environment: envSummary,
  summary,
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
    "",
    "## Steps",
    ""
  ];
  for (const step of report.steps) {
    lines.push(`- ${step.id}: ${step.status}${step.external ? " (external)" : ""}${step.required ? "" : " (optional)"}`);
  }
  return `${lines.join("\n")}\n`;
}

function commandExists(name) {
  return spawnSync("bash", ["-lc", `command -v ${sh(name)} >/dev/null 2>&1`], {
    cwd: repoRoot,
    encoding: "utf8"
  }).status === 0;
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
