#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const namespace = String(args.namespace ?? process.env.PUFFER_WEBARENA_NAMESPACE ?? "webarena-openrouter");
const outDir = path.resolve(repoRoot, String(args.out ?? "apps/puffer-desktop/tests/fuzz/.runs/webarena-campaign"));
const runsRoot = path.join(fuzzRoot, ".runs");
const shardDirs = fs.existsSync(runsRoot)
  ? fs.readdirSync(runsRoot)
    .filter((name) => name.startsWith(`${namespace}-`))
    .map((name) => path.join(runsRoot, name))
    .filter((dir) => fs.statSync(dir).isDirectory())
  : [];

const shards = shardDirs.map(readShard).sort((left, right) => left.namespace.localeCompare(right.namespace));
const materialized = shards.filter((item) => item.workerPlanPresent || item.status !== "missing");
const summary = summarize(materialized);
const report = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  summary,
  shards: materialized
};
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, "webarena_fuzz_report.json"), `${JSON.stringify(report, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "webarena_fuzz_report.md"), formatMarkdown(report));
process.stdout.write(`WEBARENA_AGGREGATE_OK ${relative(path.join(outDir, "webarena_fuzz_report.json"))}\n`);

function readShard(dir) {
  const resultPath = path.join(dir, "result.json");
  const verdictPath = path.join(dir, "verdict.json");
  const gatePath = path.join(dir, "verdict-gate.json");
  const workerPlanPath = path.join(dir, "worker-plan.json");
  const result = fs.existsSync(resultPath) ? readJson(resultPath) : null;
  const verdict = fs.existsSync(verdictPath) ? readJson(verdictPath) : null;
  const gate = fs.existsSync(gatePath) ? readJson(gatePath) : null;
  const workerPlan = fs.existsSync(workerPlanPath) ? readJson(workerPlanPath) : null;
  return {
    namespace: path.basename(dir),
    dir: relative(dir),
    shardId: result?.shardId ?? workerPlan?.shard_id ?? "",
    status: result?.status ?? "missing",
    gateDisposition: gate?.disposition ?? "missing",
    verdictDecision: verdict?.decision ?? "missing",
    score: Number(result?.score ?? 0),
    passed: Boolean(result?.passed),
    checks: result?.checks ?? [],
    evidenceEntries: Array.isArray(result?.evidence_index) ? result.evidence_index.length : 0,
    workerPlanPresent: Boolean(workerPlan),
    artifacts: {
      resultJson: fs.existsSync(resultPath) ? relative(resultPath) : "",
      verdictJson: fs.existsSync(verdictPath) ? relative(verdictPath) : "",
      gateJson: fs.existsSync(gatePath) ? relative(gatePath) : "",
      workerPlanJson: fs.existsSync(workerPlanPath) ? relative(workerPlanPath) : ""
    }
  };
}

function summarize(shards) {
  const completed = shards.filter((item) => item.status !== "missing");
  const passed = completed.filter((item) => item.passed);
  return {
    shards: shards.length,
    workerPlansPresent: shards.filter((item) => item.workerPlanPresent).length,
    completedResults: completed.length,
    admitted: shards.filter((item) => item.gateDisposition === "admitted").length,
    dismissed: shards.filter((item) => item.gateDisposition === "dismissed").length,
    candidates: shards.filter((item) => item.gateDisposition === "candidate").length,
    gateFailed: shards.filter((item) => item.gateDisposition === "gate_failed").length,
    harnessErrors: shards.filter((item) => item.status === "harness-error").length,
    passed: passed.length,
    totalScore: completed.reduce((sum, item) => sum + item.score, 0),
    averageScore: completed.length === 0 ? 0 : completed.reduce((sum, item) => sum + item.score, 0) / completed.length,
    evidenceEntries: shards.reduce((sum, item) => sum + item.evidenceEntries, 0)
  };
}

function formatMarkdown(report) {
  const lines = [
    "# WebArena Fuzz Campaign",
    "",
    `Generated: ${report.generatedAt}`,
    `Namespace: ${report.namespace}`,
    "",
    "## Summary",
    "",
    `- Shards: ${report.summary.shards}`,
    `- Worker plans: ${report.summary.workerPlansPresent}`,
    `- Completed results: ${report.summary.completedResults}`,
    `- Admitted: ${report.summary.admitted}`,
    `- Harness errors: ${report.summary.harnessErrors}`,
    `- Passed: ${report.summary.passed}`,
    `- Average score: ${report.summary.averageScore.toFixed(4)}`,
    "",
    "## Shards",
    ""
  ];
  for (const shard of report.shards) {
    lines.push(`- ${shard.shardId}: status=${shard.status}, gate=${shard.gateDisposition}, score=${shard.score}, passed=${shard.passed}`);
  }
  return `${lines.join("\n")}\n`;
}

function readJson(filePath) {
  return JSON.parse(fs.readFileSync(filePath, "utf8"));
}

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith("--")) continue;
    const key = item.slice(2);
    const next = argv[index + 1];
    if (!next || next.startsWith("--")) {
      parsed[key] = "true";
    } else {
      parsed[key] = next;
      index += 1;
    }
  }
  return parsed;
}

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
