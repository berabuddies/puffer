#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const runsRoot = path.join(fuzzRoot, ".runs");
const outDir = path.join(runsRoot, "openrouter-campaign");
const reportPath = path.join(outDir, "puffer_openrouter_fuzz_report.md");
const jsonPath = path.join(outDir, "puffer_openrouter_fuzz_report.json");
const namespace = process.env.PUFFER_OPENROUTER_NAMESPACE ?? "openrouter-small";

fs.mkdirSync(outDir, { recursive: true });

const shardDirs = fs.existsSync(runsRoot)
  ? fs.readdirSync(runsRoot, { withFileTypes: true })
    .filter((entry) =>
      entry.isDirectory() &&
      entry.name.startsWith(`${namespace}-`) &&
      entry.name !== `${namespace}-runs`
    )
    .map((entry) => path.join(runsRoot, entry.name))
    .sort()
  : [];

const shards = shardDirs.map(readShard);
const summary = summarize(shards);
const payload = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  reportPath: relative(reportPath),
  summary,
  shards
};

fs.writeFileSync(jsonPath, `${JSON.stringify(payload, null, 2)}\n`);
fs.writeFileSync(reportPath, formatMarkdown(payload));
process.stdout.write(`OPENROUTER_AGGREGATE_OK ${relative(reportPath)}\n`);

function readShard(dir) {
  const name = path.basename(dir);
  const reportJson = path.join(dir, "bounded-replay-report.json");
  const reportMd = path.join(dir, "bounded-replay-report.md");
  const findingsMd = path.join(dir, "findings.md");
  const data = fs.existsSync(reportJson)
    ? JSON.parse(fs.readFileSync(reportJson, "utf8"))
    : null;
  const findingsText = fs.existsSync(findingsMd)
    ? fs.readFileSync(findingsMd, "utf8")
    : "";
  const summary = normalizeShardSummary(data?.summary ?? emptySummary());
  return {
    name,
    dir: relative(dir),
    reportJson: relative(reportJson),
    reportMd: relative(reportMd),
    findingsMd: relative(findingsMd),
    missingReplay: data === null,
    summary,
    findings: data?.findings ?? [],
    bugListAppendBlocks: extractBugListAppendBlocks(findingsText),
    finalReportPresent: findingsText.trim().length > 0
  };
}

function normalizeShardSummary(summary) {
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
    ...summary,
    nonPassingFailures: Math.max(0, nonPassingFailures),
    actionableFailures: Math.max(0, actionableFailures)
  };
}

function summarize(shards) {
  const summary = {
    shards: shards.length,
    completedReplayReports: 0,
    missingReplayReports: 0,
    finalReportsPresent: 0,
    bugListAppendBlocks: 0,
    totalReplayCases: 0,
    newCandidateFindings: 0,
    knownDuplicateFindings: 0,
    nonPassingFailures: 0,
    actionableFailures: 0,
    byClassification: {}
  };
  for (const shard of shards) {
    if (shard.missingReplay) {
      summary.missingReplayReports += 1;
    } else {
      summary.completedReplayReports += 1;
    }
    if (shard.finalReportPresent) summary.finalReportsPresent += 1;
    summary.bugListAppendBlocks += shard.bugListAppendBlocks.length;
    summary.totalReplayCases += Number(shard.summary.total ?? 0);
    summary.newCandidateFindings += Number(shard.summary.newCandidateFindings ?? 0);
    summary.knownDuplicateFindings += Number(shard.summary.knownDuplicateFindings ?? 0);
    summary.nonPassingFailures += Number(shard.summary.nonPassingFailures ?? shard.summary.actionableFailures ?? 0);
    summary.actionableFailures += Number(shard.summary.actionableFailures ?? 0);
    for (const [classification, count] of Object.entries(shard.summary.byClassification ?? {})) {
      summary.byClassification[classification] = (summary.byClassification[classification] ?? 0) + Number(count ?? 0);
    }
  }
  return summary;
}

function emptySummary() {
  return {
    total: 0,
    newCandidateFindings: 0,
    knownDuplicateFindings: 0,
    nonPassingFailures: 0,
    actionableFailures: 0
  };
}

function extractBugListAppendBlocks(text) {
  const blocks = [];
  const pattern = /BUG_LIST_APPEND[\s\S]*?END_BUG_LIST_APPEND/g;
  for (const match of text.matchAll(pattern)) blocks.push(match[0]);
  return blocks;
}

function formatMarkdown(payload) {
  const lines = [
    "# Puffer OpenRouter Small-Model UI/UX Fuzz Report",
    "",
    `Generated: ${payload.generatedAt}`,
    `Namespace: ${payload.namespace}`,
    "",
    "## Summary",
    "",
    `- Shards discovered: ${payload.summary.shards}`,
    `- Completed replay reports: ${payload.summary.completedReplayReports}`,
    `- Missing replay reports: ${payload.summary.missingReplayReports}`,
    `- Final reports present: ${payload.summary.finalReportsPresent}`,
    `- BUG_LIST_APPEND blocks: ${payload.summary.bugListAppendBlocks}`,
    `- Replay cases: ${payload.summary.totalReplayCases}`,
    `- New candidate findings: ${payload.summary.newCandidateFindings}`,
    `- Known duplicate findings: ${payload.summary.knownDuplicateFindings}`,
    `- Non-passing failures: ${payload.summary.nonPassingFailures}`,
    `- Actionable product failures: ${payload.summary.actionableFailures}`,
    "",
    "## Classification",
    ""
  ];
  appendCountLines(lines, payload.summary.byClassification);
  lines.push("", "## Shards", "");
  if (payload.shards.length === 0) {
    lines.push("- No shard output directories found.");
  }
  for (const shard of payload.shards) {
    lines.push(`### ${shard.name}`, "");
    lines.push(`- Directory: ${shard.dir}`);
    lines.push(`- Bounded replay: ${shard.missingReplay ? "missing" : shard.reportJson}`);
    lines.push(`- Findings report: ${shard.finalReportPresent ? shard.findingsMd : "missing"}`);
    lines.push(`- Replay cases: ${shard.summary.total ?? 0}`);
    lines.push(`- New candidates: ${shard.summary.newCandidateFindings ?? 0}`);
    lines.push(`- Known duplicates: ${shard.summary.knownDuplicateFindings ?? 0}`);
    lines.push(`- Non-passing failures: ${shard.summary.nonPassingFailures ?? shard.summary.actionableFailures ?? 0}`);
    lines.push(`- Actionable product failures: ${shard.summary.actionableFailures ?? 0}`);
    lines.push(`- BUG_LIST_APPEND blocks: ${shard.bugListAppendBlocks.length}`);
    lines.push("");
  }
  lines.push("## BUG_LIST_APPEND Blocks", "");
  const blocks = payload.shards.flatMap((shard) =>
    shard.bugListAppendBlocks.map((block) => ({ shard: shard.name, block }))
  );
  if (blocks.length === 0) {
    lines.push("- None");
  } else {
    for (const item of blocks) {
      lines.push(`### ${item.shard}`, "", "```text", item.block, "```", "");
    }
  }
  return `${lines.join("\n")}\n`;
}

function appendCountLines(lines, counts) {
  const entries = Object.entries(counts ?? {}).sort((left, right) => left[0].localeCompare(right[0]));
  if (entries.length === 0) {
    lines.push("- None");
    return;
  }
  for (const [key, value] of entries) lines.push(`- ${key}: ${value}`);
}

function relative(filePath) {
  return path.relative(path.resolve(fuzzRoot, "..", "..", "..", ".."), filePath).replaceAll(path.sep, "/");
}
