#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const outDir = path.join(fuzzRoot, ".runs", "agentflow-campaign");
const reportPath = path.join(outDir, "puffer_agentflow_fuzz_report.md");
const jsonPath = path.join(outDir, "puffer_agentflow_fuzz_report.json");

const areas = [
  { name: "chat-turn-lifecycle", seed: "chat-turn-race", priority: "P0/P1" },
  { name: "workspace-session-switching", seed: "workspace-session-race", priority: "P1" },
  { name: "provider-auth-model", seed: "provider-auth-model-race", priority: "P1" },
  { name: "modal-focus-keyboard", seed: "modal-focus-race", priority: "P1" },
  { name: "files-terminal", seed: "files-terminal-race", priority: "P1/P2" },
  { name: "browser-tabs-input", seed: "browser-tab-race", priority: "P1/P2" },
  { name: "settings-mcp-permissions", seed: "settings-mcp-permission-race", priority: "P2" },
  { name: "pipelines-drafts", seed: "pipelines-draft-race", priority: "P2" }
];
const selectedAreas = selectAreas(areas);

fs.mkdirSync(outDir, { recursive: true });

const shards = selectedAreas.map((area) => readShard(area));
const summary = aggregateSummary(shards);
const payload = {
  version: 1,
  generatedAt: new Date().toISOString(),
  reportPath: relative(reportPath),
  selectedAreas: selectedAreas.map((area) => area.name),
  summary,
  shards
};

fs.writeFileSync(jsonPath, `${JSON.stringify(payload, null, 2)}\n`);
fs.writeFileSync(reportPath, formatMarkdown(payload));

process.stdout.write(`AGGREGATE_OK partial=${summary.missingReports > 0 ? "true" : "false"} ${relative(reportPath)}\n`);

function readShard(area) {
  const artifactDir = path.join(fuzzRoot, ".runs", `agentflow-${area.name}`);
  const reportJson = path.join(artifactDir, "bounded-replay-report.json");
  const reportMd = path.join(artifactDir, "bounded-replay-report.md");
  if (!fs.existsSync(reportJson)) {
    return {
      ...area,
      artifactDir: relative(artifactDir),
      reportJson: relative(reportJson),
      reportMd: relative(reportMd),
      missing: true,
      summary: emptySummary(),
      findings: [],
      routeCounts: {},
      cases: []
    };
  }
  const data = JSON.parse(fs.readFileSync(reportJson, "utf8"));
  return {
    ...area,
    artifactDir: relative(artifactDir),
    reportJson: relative(reportJson),
    reportMd: relative(reportMd),
    missing: false,
    summary: normalizeSummary(data.summary ?? {}),
    findings: data.findings ?? [],
    routeCounts: routeCounts(data.results ?? []),
    allRouteCounts: allRouteCounts(data.results ?? []),
    cases: (data.results ?? []).map((item) => ({
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
}

function selectAreas(allAreas) {
  const raw = process.env.PUFFER_AGENTFLOW_AREAS ?? "";
  const requested = raw
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
  if (requested.length === 0) return allAreas;
  const byName = new Map(allAreas.map((area) => [area.name, area]));
  const bySeed = new Map(allAreas.map((area) => [area.seed, area]));
  const selected = requested.map((item) => byName.get(item) ?? bySeed.get(item));
  const missing = requested.filter((item, index) => !selected[index]);
  if (missing.length > 0) {
    throw new Error(`Unknown PUFFER_AGENTFLOW_AREAS item(s): ${missing.join(", ")}`);
  }
  return selected;
}

function aggregateSummary(shards) {
  const summary = emptySummary();
  summary.missingReports = 0;
  summary.shards = shards.length;
  summary.completedShards = 0;
  summary.routeCounts = {};
  summary.allRouteCounts = {};
  summary.byClassification = {};
  for (const shard of shards) {
    if (shard.missing) {
      summary.missingReports += 1;
      continue;
    }
    summary.completedShards += 1;
    addNumbers(summary, shard.summary);
    for (const [route, count] of Object.entries(shard.routeCounts)) {
      summary.routeCounts[route] = (summary.routeCounts[route] ?? 0) + count;
    }
    for (const [route, count] of Object.entries(shard.allRouteCounts)) {
      summary.allRouteCounts[route] = (summary.allRouteCounts[route] ?? 0) + count;
    }
    for (const [classification, count] of Object.entries(shard.summary.byClassification ?? {})) {
      summary.byClassification[classification] = (summary.byClassification[classification] ?? 0) + count;
    }
  }
  return summary;
}

function addNumbers(target, source) {
  for (const key of [
    "total",
    "passed",
    "stableFailed",
    "flaky",
    "timeout",
    "productCandidateFindings",
    "newCandidateFindings",
    "knownDuplicateFindings",
    "knownDuplicateFailures",
    "nonPassingFailures",
    "actionableFailures"
  ]) {
    target[key] = (target[key] ?? 0) + Number(source[key] ?? 0);
  }
}

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
    nonPassingFailures: 0,
    actionableFailures: 0,
    byClassification: {}
  };
}

function normalizeSummary(summary) {
  const byClassification = summary.byClassification ?? {};
  const nonPassingFailures = Number(
    summary.nonPassingFailures ??
    ((summary.total ?? 0) - (summary.passed ?? 0) - (summary.knownDuplicateFailures ?? 0))
  );
  const actionableFailures = summary.nonPassingFailures === undefined
    ? Object.entries(byClassification)
      .filter(([classification]) =>
        classification.startsWith("product-candidate:") ||
        classification === "needs-manual-triage" ||
        classification.startsWith("needs-manual-triage:")
      )
      .reduce((total, [, count]) => total + Number(count ?? 0), 0)
    : Number(summary.actionableFailures ?? 0);
  return {
    ...emptySummary(),
    ...summary,
    byClassification,
    nonPassingFailures: Math.max(0, nonPassingFailures),
    actionableFailures: Math.max(0, actionableFailures)
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
    "# Puffer AgentFlow UI/UX Fuzz Campaign Report",
    "",
    `Generated: ${payload.generatedAt}`,
    `Selected areas: ${payload.selectedAreas.join(", ")}`,
    "",
    "## Summary",
    "",
    `- Shards: ${payload.summary.completedShards}/${payload.summary.shards}`,
    `- Missing bounded reports: ${payload.summary.missingReports}`,
    `- Total replay cases: ${payload.summary.total}`,
    `- Passed: ${payload.summary.passed}`,
    `- Stable failed: ${payload.summary.stableFailed}`,
    `- Flaky: ${payload.summary.flaky}`,
    `- Timed out: ${payload.summary.timeout}`,
    `- New product-candidate findings: ${payload.summary.newCandidateFindings}`,
    `- Known duplicate findings: ${payload.summary.knownDuplicateFindings}`,
    `- Known duplicate failures: ${payload.summary.knownDuplicateFailures}`,
    `- Non-passing failures: ${payload.summary.nonPassingFailures}`,
    `- Actionable product failures: ${payload.summary.actionableFailures}`,
    "",
    "## Classification",
    ""
  ];
  appendCountLines(lines, payload.summary.byClassification);
  lines.push("", "## Primary Replayed Route Coverage", "");
  appendCountLines(lines, payload.summary.routeCounts);
  lines.push("", "## All Replayed Route Tags", "");
  appendCountLines(lines, payload.summary.allRouteCounts);
  lines.push("", "## Shards", "");
  for (const shard of payload.shards) appendShard(lines, shard);
  lines.push("", "## Candidate Findings", "");
  const findings = payload.shards.flatMap((shard) => shard.findings.map((finding) => ({ ...finding, shard: shard.name })));
  if (findings.length === 0) {
    lines.push("- None");
  } else {
    for (const finding of findings) appendFinding(lines, finding);
  }
  return `${lines.join("\n")}\n`;
}

function appendShard(lines, shard) {
  lines.push(`### ${shard.name}`, "");
  lines.push(`- Seed: ${shard.seed}`);
  lines.push(`- Priority: ${shard.priority}`);
  lines.push(`- Bounded report: ${shard.missing ? "missing" : shard.reportJson}`);
  lines.push(`- Cases: ${shard.summary.total}`);
  lines.push(`- Passed: ${shard.summary.passed}`);
  lines.push(`- Stable failed: ${shard.summary.stableFailed}`);
  lines.push(`- Non-passing failures: ${shard.summary.nonPassingFailures}`);
  lines.push(`- Actionable product failures: ${shard.summary.actionableFailures}`);
  lines.push(`- Known duplicate findings: ${shard.summary.knownDuplicateFindings}`);
  lines.push(`- Primary routes: ${formatCounts(shard.routeCounts)}`);
  lines.push(`- All route tags: ${formatCounts(shard.allRouteCounts)}`);
  if (shard.cases.length > 0) {
    lines.push(`- Replayed cases: ${shard.cases.map((item) => `${item.caseId}:${item.status}:${item.primaryRoute}`).join(", ")}`);
  }
  lines.push("");
}

function appendFinding(lines, finding) {
  lines.push(`### ${finding.title ?? finding.caseId ?? "Finding"}`, "");
  lines.push(`- Shard: ${finding.shard}`);
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

function appendCountLines(lines, counts) {
  const entries = Object.entries(counts ?? {}).sort((left, right) => left[0].localeCompare(right[0]));
  if (entries.length === 0) {
    lines.push("- None");
    return;
  }
  for (const [key, value] of entries) lines.push(`- ${key}: ${value}`);
}

function formatCounts(counts) {
  const entries = Object.entries(counts ?? {}).sort((left, right) => left[0].localeCompare(right[0]));
  if (entries.length === 0) return "none";
  return entries.map(([key, value]) => `${key}=${value}`).join(", ");
}

function relative(targetPath) {
  return path.relative(repoRoot, targetPath);
}
