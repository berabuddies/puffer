#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const shardName = "modal-focus-keyboard-smoke";
const seed = "modal-focus-race";
const artifactDir = path.join(fuzzRoot, ".runs", "agentflow-smoke-modal-focus-keyboard-smoke");
const reportJson = path.join(artifactDir, "bounded-replay-report.json");
const reportMd = path.join(artifactDir, "bounded-replay-report.md");
const outDir = path.join(fuzzRoot, ".runs", "agentflow-smoke");
const outJson = path.join(outDir, "puffer_agentflow_smoke_report.json");
const outMd = path.join(outDir, "puffer_agentflow_smoke_report.md");

fs.mkdirSync(outDir, { recursive: true });

if (!fs.existsSync(reportJson)) {
  const payload = {
    version: 1,
    generatedAt: new Date().toISOString(),
    missingReport: relative(reportJson),
    summary: emptySummary(),
    results: [],
    findings: []
  };
  fs.writeFileSync(outJson, `${JSON.stringify(payload, null, 2)}\n`);
  fs.writeFileSync(outMd, formatMarkdown(payload));
  process.stderr.write(`Missing bounded replay report: ${relative(reportJson)}\n`);
  process.exit(2);
}

const data = JSON.parse(fs.readFileSync(reportJson, "utf8"));
const payload = {
  version: 1,
  generatedAt: new Date().toISOString(),
  shardName,
  seed,
  artifactDir: relative(artifactDir),
  reportJson: relative(reportJson),
  reportMd: relative(reportMd),
  summary: { ...emptySummary(), ...(data.summary ?? {}) },
  primaryRouteCounts: routeCounts(data.results ?? []),
  allRouteCounts: allRouteCounts(data.results ?? []),
  findings: data.findings ?? [],
  results: (data.results ?? []).map((item) => ({
    caseId: item.caseId,
    status: item.status,
    classification: item.classification,
    knownDuplicate: Boolean(item.knownDuplicate),
    primaryRoute: primaryRoute(item.coverage ?? []),
    routes: (item.coverage ?? []).filter((tag) => String(tag).startsWith("route:")),
    coverage: item.coverage ?? [],
    steps: item.steps ?? [],
    attempts: item.attempts?.length ?? 0
  }))
};

fs.writeFileSync(outJson, `${JSON.stringify(payload, null, 2)}\n`);
fs.writeFileSync(outMd, formatMarkdown(payload));
process.stdout.write(`SMOKE_AGGREGATE_OK ${relative(outMd)}\n`);

function emptySummary() {
  return {
    total: 0,
    passed: 0,
    stableFailed: 0,
    flaky: 0,
    timeout: 0,
    productCandidateFindings: 0,
    newCandidateFindings: 0,
    knownDuplicateFindings: 0,
    knownDuplicateFailures: 0,
    actionableFailures: 0,
    byClassification: {}
  };
}

function routeCounts(results) {
  const counts = {};
  for (const item of results) {
    const route = primaryRoute(item.coverage ?? []);
    counts[route] = (counts[route] ?? 0) + 1;
  }
  return counts;
}

function allRouteCounts(results) {
  const counts = {};
  for (const item of results) {
    const routes = new Set((item.coverage ?? []).filter((tag) => String(tag).startsWith("route:")));
    for (const route of routes) counts[route] = (counts[route] ?? 0) + 1;
  }
  return counts;
}

function primaryRoute(coverage) {
  const routes = coverage
    .filter((tag) => String(tag).startsWith("route:"))
    .sort();
  return routes.find((tag) => !["route:workspace", "route:agent-detail"].includes(tag)) ?? routes[0] ?? "route:none";
}

function formatMarkdown(payload) {
  const lines = [
    "# Puffer AgentFlow Smoke Report",
    "",
    `Generated: ${payload.generatedAt}`,
    `Shard: ${payload.shardName ?? "missing"}`,
    `Seed: ${payload.seed ?? "unknown"}`,
    "",
    "## Summary",
    "",
    `- Missing bounded report: ${payload.missingReport ?? "no"}`,
    `- Total replay cases: ${payload.summary.total}`,
    `- Passed: ${payload.summary.passed}`,
    `- Stable failed: ${payload.summary.stableFailed}`,
    `- Flaky: ${payload.summary.flaky}`,
    `- Timed out: ${payload.summary.timeout}`,
    `- New product-candidate findings: ${payload.summary.newCandidateFindings}`,
    `- Known duplicate findings: ${payload.summary.knownDuplicateFindings}`,
    `- Actionable failures: ${payload.summary.actionableFailures}`,
    "",
    "## Classification",
    ""
  ];
  appendCounts(lines, payload.summary.byClassification);
  lines.push("", "## Primary Replayed Route Coverage", "");
  appendCounts(lines, payload.primaryRouteCounts);
  lines.push("", "## All Replayed Route Tags", "");
  appendCounts(lines, payload.allRouteCounts);
  lines.push("", "## Replayed Cases", "");
  if ((payload.results ?? []).length === 0) {
    lines.push("- None");
  } else {
    for (const item of payload.results) {
      lines.push(`- ${item.caseId}: ${item.status}; ${item.primaryRoute}; attempts=${item.attempts}`);
    }
  }
  lines.push("", "## Candidate Findings", "");
  if ((payload.findings ?? []).length === 0) {
    lines.push("- None");
  } else {
    for (const finding of payload.findings) appendFinding(lines, finding);
  }
  return `${lines.join("\n")}\n`;
}

function appendFinding(lines, finding) {
  lines.push(`### ${finding.title ?? finding.caseId ?? "Finding"}`, "");
  lines.push(`- Classification: ${finding.classification ?? "unknown"}`);
  lines.push(`- Status: ${finding.status ?? "unknown"}`);
  lines.push(`- Known duplicate: ${finding.knownDuplicate ? "yes" : "no"}`);
  lines.push(`- Seed/case: ${finding.seed ?? "unknown"} / ${finding.caseId ?? "unknown"}`);
  lines.push(`- Failure signature: ${finding.failureSignature ?? ""}`);
  lines.push(`- Coverage: ${(finding.coverage ?? []).join(", ")}`);
  lines.push(`- Trigger steps: ${(finding.steps ?? []).join(" -> ")}`);
  if (finding.excerpt) lines.push("", "```text", finding.excerpt, "```");
  lines.push("");
}

function appendCounts(lines, counts) {
  const entries = Object.entries(counts ?? {}).sort((left, right) => left[0].localeCompare(right[0]));
  if (entries.length === 0) {
    lines.push("- None");
    return;
  }
  for (const [key, value] of entries) lines.push(`- ${key}: ${value}`);
}

function relative(targetPath) {
  return path.relative(repoRoot, targetPath);
}
