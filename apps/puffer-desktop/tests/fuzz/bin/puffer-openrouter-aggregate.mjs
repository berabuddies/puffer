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
      !isCampaignControlDir(entry.name, namespace)
    )
    .map((entry) => path.join(runsRoot, entry.name))
    .sort()
  : [];

const shards = shardDirs.map(readShard);
const summary = summarize(shards);
const blockerSummary = summarizeBlockers(shards);
const payload = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  reportPath: relative(reportPath),
  summary,
  blockerSummary,
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
  const verdictJson = path.join(dir, "verdict.json");
  const gateJson = path.join(dir, "verdict-gate.json");
  const reviewerJson = path.join(dir, "reviewer.json");
  const data = fs.existsSync(reportJson)
    ? JSON.parse(fs.readFileSync(reportJson, "utf8"))
    : null;
  const verdict = fs.existsSync(verdictJson)
    ? JSON.parse(fs.readFileSync(verdictJson, "utf8"))
    : null;
  const gate = fs.existsSync(gateJson)
    ? JSON.parse(fs.readFileSync(gateJson, "utf8"))
    : null;
  const reviewer = fs.existsSync(reviewerJson)
    ? JSON.parse(fs.readFileSync(reviewerJson, "utf8"))
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
    verdictJson: relative(verdictJson),
    gateJson: relative(gateJson),
    reviewerJson: relative(reviewerJson),
    missingReplay: data === null,
    summary,
    evidenceCount: Array.isArray(data?.evidence_index) ? data.evidence_index.length : 0,
    evidenceByType: countEvidenceTypes(data?.evidence_index ?? []),
    findings: data?.findings ?? [],
    verdict,
    gate,
    reviewer,
    bugListAppendBlocks: extractBugListAppendBlocks(findingsText),
    finalReportPresent: findingsText.trim().length > 0
  };
}

function isCampaignControlDir(name, namespace) {
  return name === `${namespace}-runs` ||
    name === `${namespace}-local-runs` ||
    name === `${namespace}-preflight` ||
    name === `${namespace}-scheduler-preselect` ||
    name.endsWith("-local-runs") ||
    name.endsWith("-preflight") ||
    name.endsWith("-scheduler-preselect");
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
    legacyBugListAppendBlocks: 0,
    verdictReportsPresent: 0,
    admittedVerdicts: 0,
    candidateVerdicts: 0,
    dismissedVerdicts: 0,
    gateFailedVerdicts: 0,
    reviewerReportsPresent: 0,
    reviewerAdmitDecisions: 0,
    reviewerDismissDecisions: 0,
    reviewerHumanQueueDecisions: 0,
    totalReplayCases: 0,
    evidenceEntries: 0,
    newCandidateFindings: 0,
    knownDuplicateFindings: 0,
    nonPassingFailures: 0,
    actionableFailures: 0,
    byClassification: {},
    evidenceByType: {},
    gateFailureReasons: {}
  };
  for (const shard of shards) {
    if (shard.missingReplay) {
      summary.missingReplayReports += 1;
    } else {
      summary.completedReplayReports += 1;
    }
    if (shard.finalReportPresent) summary.finalReportsPresent += 1;
    if (shard.verdict) summary.verdictReportsPresent += 1;
    if (shard.gate?.disposition === "admitted") summary.admittedVerdicts += 1;
    if (shard.gate?.disposition === "candidate") summary.candidateVerdicts += 1;
    if (shard.gate?.disposition === "dismissed") summary.dismissedVerdicts += 1;
    if (shard.gate?.disposition === "gate_failed") summary.gateFailedVerdicts += 1;
    if (shard.reviewer) summary.reviewerReportsPresent += 1;
    if (shard.reviewer?.decision === "admit") summary.reviewerAdmitDecisions += 1;
    if (shard.reviewer?.decision === "dismiss") summary.reviewerDismissDecisions += 1;
    if (shard.reviewer?.decision === "human_queue") summary.reviewerHumanQueueDecisions += 1;
    summary.legacyBugListAppendBlocks += shard.bugListAppendBlocks.length;
    summary.totalReplayCases += Number(shard.summary.total ?? 0);
    summary.evidenceEntries += shard.evidenceCount;
    summary.newCandidateFindings += Number(shard.summary.newCandidateFindings ?? 0);
    summary.knownDuplicateFindings += Number(shard.summary.knownDuplicateFindings ?? 0);
    summary.nonPassingFailures += Number(shard.summary.nonPassingFailures ?? shard.summary.actionableFailures ?? 0);
    summary.actionableFailures += Number(shard.summary.actionableFailures ?? 0);
    for (const [classification, count] of Object.entries(shard.summary.byClassification ?? {})) {
      summary.byClassification[classification] = (summary.byClassification[classification] ?? 0) + Number(count ?? 0);
    }
    for (const [type, count] of Object.entries(shard.evidenceByType ?? {})) {
      summary.evidenceByType[type] = (summary.evidenceByType[type] ?? 0) + Number(count ?? 0);
    }
    if (["candidate", "gate_failed"].includes(shard.gate?.disposition)) {
      for (const reason of shard.gate?.failureReasons ?? []) {
        summary.gateFailureReasons[reason] = (summary.gateFailureReasons[reason] ?? 0) + 1;
      }
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

function countEvidenceTypes(evidenceIndex) {
  const counts = {};
  for (const entry of evidenceIndex ?? []) {
    const type = String(entry.type ?? "unknown");
    counts[type] = (counts[type] ?? 0) + 1;
  }
  return counts;
}

function extractBugListAppendBlocks(text) {
  const blocks = [];
  const pattern = /BUG_LIST_APPEND[\s\S]*?END_BUG_LIST_APPEND/g;
  for (const match of text.matchAll(pattern)) blocks.push(match[0]);
  return blocks;
}

function summarizeBlockers(shards) {
  const topBlockers = [];
  const counts = {};
  for (const shard of shards) {
    for (const blocker of blockersForShard(shard)) {
      topBlockers.push(blocker);
      counts[blocker.type] = (counts[blocker.type] ?? 0) + 1;
    }
  }
  topBlockers.sort((left, right) => {
    if (severityRank(left.severity) !== severityRank(right.severity)) {
      return severityRank(left.severity) - severityRank(right.severity);
    }
    return left.shard.localeCompare(right.shard) || left.type.localeCompare(right.type);
  });
  return {
    ready: topBlockers.length === 0,
    total: topBlockers.length,
    counts,
    topBlockers: topBlockers.slice(0, 20)
  };
}

function blockersForShard(shard) {
  const blockers = [];
  if (shard.missingReplay) {
    blockers.push(blocker(shard, "missing-replay", "P0", "Bounded replay report is missing", shard.reportJson));
    return blockers;
  }
  if (!shard.finalReportPresent) {
    blockers.push(blocker(shard, "missing-findings-report", "P2", "Findings report is missing", shard.findingsMd));
  }
  if (!shard.verdict) {
    blockers.push(blocker(shard, "missing-verdict", "P1", "Structured verdict is missing", shard.verdictJson));
  }
  if (!shard.gate) {
    blockers.push(blocker(shard, "missing-citation-gate", "P1", "Citation gate result is missing", shard.gateJson));
  }
  if (shard.gate?.disposition === "gate_failed") {
    blockers.push(blocker(
      shard,
      "gate-failed",
      "P1",
      `Citation gate failed: ${(shard.gate.failureReasons ?? []).join("; ") || "unknown reason"}`,
      shard.gateJson
    ));
  }
  if (shard.gate?.disposition === "candidate") {
    blockers.push(blocker(shard, "candidate-review", "P2", "Candidate verdict needs reviewer or human triage", shard.gateJson));
  }
  if (shard.reviewer?.decision === "human_queue") {
    blockers.push(blocker(shard, "reviewer-human-queue", "P2", "Reviewer deferred candidate to human queue", shard.reviewerJson));
  }
  if (Number(shard.summary.actionableFailures ?? 0) > 0 && shard.gate?.disposition !== "admitted") {
    blockers.push(blocker(shard, "unadmitted-actionable-failure", "P1", "Replay reports actionable failures without an admitted verdict", shard.reportJson));
  }
  if (Number(shard.summary.nonPassingFailures ?? 0) > 0 && shard.gate?.disposition === "dismissed") {
    blockers.push(blocker(shard, "dismissed-nonpassing-failure", "P2", "Non-passing replay was dismissed by triage", shard.reportJson));
  }
  if (shard.bugListAppendBlocks.length > 0) {
    blockers.push(blocker(shard, "legacy-bug-list-append", "P2", "Legacy BUG_LIST_APPEND output is still present", shard.findingsMd));
  }
  return blockers;
}

function blocker(shard, type, severity, detail, artifact) {
  return {
    type,
    severity,
    shard: shard.name,
    detail,
    artifact
  };
}

function severityRank(severity) {
  return { P0: 0, P1: 1, P2: 2, P3: 3 }[severity] ?? 9;
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
    `- Legacy BUG_LIST_APPEND blocks: ${payload.summary.legacyBugListAppendBlocks}`,
    `- Verdict reports present: ${payload.summary.verdictReportsPresent}`,
    `- Admitted verdicts: ${payload.summary.admittedVerdicts}`,
    `- Candidate verdicts: ${payload.summary.candidateVerdicts}`,
    `- Dismissed verdicts: ${payload.summary.dismissedVerdicts}`,
    `- Gate-failed verdicts: ${payload.summary.gateFailedVerdicts}`,
    `- Reviewer reports present: ${payload.summary.reviewerReportsPresent}`,
    `- Reviewer decisions admit/dismiss/human_queue: ${payload.summary.reviewerAdmitDecisions}/${payload.summary.reviewerDismissDecisions}/${payload.summary.reviewerHumanQueueDecisions}`,
    `- Replay cases: ${payload.summary.totalReplayCases}`,
    `- Evidence entries: ${payload.summary.evidenceEntries}`,
    `- New candidate findings: ${payload.summary.newCandidateFindings}`,
    `- Known duplicate findings: ${payload.summary.knownDuplicateFindings}`,
    `- Non-passing failures: ${payload.summary.nonPassingFailures}`,
    `- Actionable product failures: ${payload.summary.actionableFailures}`,
    "",
    "## Top Blockers",
    "",
    `- Ready: ${payload.blockerSummary.ready ? "yes" : "no"}`,
    `- Total blockers: ${payload.blockerSummary.total}`,
    ""
  ];
  appendCountLines(lines, payload.blockerSummary.counts);
  if (payload.blockerSummary.topBlockers.length === 0) {
    lines.push("", "- None");
  } else {
    lines.push("");
    for (const item of payload.blockerSummary.topBlockers) {
      lines.push(`- ${item.severity} ${item.type} in ${item.shard}: ${item.detail} (${item.artifact})`);
    }
  }
  lines.push(
    "",
    "## Evidence By Type",
    ""
  );
  appendCountLines(lines, payload.summary.evidenceByType);
  lines.push(
    "",
    "## Gate Failure Reasons",
    ""
  );
  appendCountLines(lines, payload.summary.gateFailureReasons);
  lines.push(
    "",
    "## Classification",
    ""
  );
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
    lines.push(`- Verdict: ${shard.verdict ? shard.verdictJson : "missing"}`);
    lines.push(`- Citation gate: ${shard.gate ? `${shard.gateJson} (${shard.gate.disposition})` : "missing"}`);
    lines.push(`- Reviewer: ${shard.reviewer ? `${shard.reviewerJson} (${shard.reviewer.decision})` : "missing"}`);
    lines.push(`- Replay cases: ${shard.summary.total ?? 0}`);
    lines.push(`- Evidence entries: ${shard.evidenceCount}`);
    lines.push(`- New candidates: ${shard.summary.newCandidateFindings ?? 0}`);
    lines.push(`- Known duplicates: ${shard.summary.knownDuplicateFindings ?? 0}`);
    lines.push(`- Non-passing failures: ${shard.summary.nonPassingFailures ?? shard.summary.actionableFailures ?? 0}`);
    lines.push(`- Actionable product failures: ${shard.summary.actionableFailures ?? 0}`);
    lines.push(`- Legacy BUG_LIST_APPEND blocks: ${shard.bugListAppendBlocks.length}`);
    if (shard.gate?.failureReasons?.length) {
      lines.push(`- Gate failures: ${shard.gate.failureReasons.join("; ")}`);
    }
    lines.push("");
  }
  lines.push("## Legacy BUG_LIST_APPEND Blocks", "");
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
