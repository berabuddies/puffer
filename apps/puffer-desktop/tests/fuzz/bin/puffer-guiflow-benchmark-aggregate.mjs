#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const namespace = String(args.namespace ?? process.env.PUFFER_GUIFLOW_NAMESPACE ?? "guiflow-openrouter");
const outDir = path.resolve(repoRoot, String(args.out ?? "apps/puffer-desktop/tests/fuzz/.runs/guiflow-benchmark-campaign"));
const runsRoot = path.join(fuzzRoot, ".runs");
const shardDirs = fs.existsSync(runsRoot)
  ? fs.readdirSync(runsRoot)
    .filter((name) => name.startsWith(`${namespace}-`))
    .map((name) => path.join(runsRoot, name))
    .filter((dir) => fs.statSync(dir).isDirectory())
  : [];

const shards = shardDirs.map(readShard).sort((left, right) => left.namespace.localeCompare(right.namespace));
const materializedShards = shards.filter((item) => item.workerPlanPresent || item.status !== "missing");
const summary = summarize(materializedShards);
const report = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  summary,
  shards: materializedShards
};
fs.mkdirSync(outDir, { recursive: true });
fs.writeFileSync(path.join(outDir, "guiflow_benchmark_fuzz_report.json"), `${JSON.stringify(report, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "guiflow_benchmark_fuzz_report.md"), formatMarkdown(report));
process.stdout.write(`GUIFLOW_BENCHMARK_AGGREGATE_OK ${relative(path.join(outDir, "guiflow_benchmark_fuzz_report.json"))}\n`);

function readShard(dir) {
  const resultPath = findFirst(dir, "result.json");
  const verdictPath = findFirst(dir, "verdict.json");
  const gatePath = findFirst(dir, "verdict-gate.json");
  const workerPlanPath = path.join(dir, "worker-plan.json");
  const result = resultPath ? readJson(resultPath) : null;
  const verdict = verdictPath ? readJson(verdictPath) : null;
  const gate = gatePath ? readJson(gatePath) : null;
  const workerPlan = fs.existsSync(workerPlanPath) ? readJson(workerPlanPath) : null;
  return {
    namespace: path.basename(dir),
    dir: relative(dir),
    caseId: result?.caseId ?? workerPlan?.case_id ?? "",
    app: result?.app ?? "",
    goldBug: Boolean(result?.goldBug),
    status: result?.status ?? "missing",
    gateDisposition: gate?.disposition ?? "missing",
    verdictDecision: verdict?.decision ?? "missing",
    evidenceEntries: Array.isArray(result?.evidence_index) ? result.evidence_index.length : 0,
    workerPlanPresent: Boolean(workerPlan),
    artifacts: {
      resultJson: resultPath ? relative(resultPath) : "",
      verdictJson: verdictPath ? relative(verdictPath) : "",
      gateJson: gatePath ? relative(gatePath) : "",
      workerPlanJson: fs.existsSync(workerPlanPath) ? relative(workerPlanPath) : ""
    }
  };
}

function summarize(shards) {
  return {
    shards: shards.length,
    workerPlansPresent: shards.filter((item) => item.workerPlanPresent).length,
    completedResults: shards.filter((item) => item.status !== "missing").length,
    admitted: shards.filter((item) => item.gateDisposition === "admitted").length,
    candidates: shards.filter((item) => item.gateDisposition === "candidate").length,
    dismissed: shards.filter((item) => item.gateDisposition === "dismissed").length,
    gateFailed: shards.filter((item) => item.gateDisposition === "gate_failed").length,
    buggyAdmitted: shards.filter((item) => item.goldBug && item.gateDisposition === "admitted").length,
    fixedAdmitted: shards.filter((item) => !item.goldBug && item.gateDisposition === "admitted").length,
    harnessErrors: shards.filter((item) => item.status === "harness-error").length,
    evidenceEntries: shards.reduce((sum, item) => sum + item.evidenceEntries, 0)
  };
}

function findFirst(dir, fileName) {
  const direct = path.join(dir, fileName);
  if (fs.existsSync(direct)) return direct;
  for (const child of fs.readdirSync(dir)) {
    const nested = path.join(dir, child, fileName);
    if (fs.existsSync(nested)) return nested;
  }
  return "";
}

function formatMarkdown(report) {
  const lines = [
    "# GUIFlow Benchmark Fuzz Campaign",
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
    `- Buggy admitted: ${report.summary.buggyAdmitted}`,
    `- Fixed admitted: ${report.summary.fixedAdmitted}`,
    `- Harness errors: ${report.summary.harnessErrors}`,
    "",
    "## Shards",
    ""
  ];
  for (const shard of report.shards) {
    lines.push(`- ${shard.caseId}: status=${shard.status}, gate=${shard.gateDisposition}, goldBug=${shard.goldBug}, evidence=${shard.evidenceEntries}`);
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
