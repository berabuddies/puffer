import { readdir, readFile } from "node:fs/promises";

export async function buildPromptEvolutionPack(options = {}) {
  const [
    baseGuide,
    bugList,
    feedbackLedger,
    bugMemory,
    issueText,
    pictureFiles
  ] = await Promise.all([
    readOptional(options.baseGuidePath),
    readOptional(options.bugListPath),
    readJsonOptional(options.feedbackLedgerPath),
    readJsonOptional(options.bugMemoryPath),
    readOptional(options.issuePath),
    listPictureFiles(options.picsDir)
  ]);

  const bugStats = summarizeBugList(bugList);
  const feedbackStats = summarizeFeedback(feedbackLedger);
  const issueNotes = summarizeIssue(issueText);
  const pack = {
    version: 1,
    generatedAt: new Date().toISOString(),
    inputs: {
      baseGuidePath: options.baseGuidePath ?? "",
      bugListPath: options.bugListPath ?? "",
      feedbackLedgerPath: options.feedbackLedgerPath ?? "",
      bugMemoryPath: options.bugMemoryPath ?? "",
      issuePath: options.issuePath ?? "",
      picsDir: options.picsDir ?? ""
    },
    bugStats,
    feedbackStats,
    bugMemoryStats: summarizeBugMemory(bugMemory),
    issueNotes,
    pictureFiles
  };
  return {
    pack,
    markdown: formatPromptEvolutionMarkdown({ baseGuide, pack })
  };
}

export function formatPromptEvolutionMarkdown({ baseGuide, pack }) {
  const lines = [
    "# Puffer UI/UX Prompt Evolution Runtime Pack",
    "",
    `Generated: ${pack.generatedAt}`,
    "",
    "## Base Gold Standard",
    "",
    baseGuide.trim() || "- Base guide unavailable.",
    "",
    "## Historical Signal Summary",
    "",
    `- Ledger entries: ${pack.bugStats.total}`,
    `- Pending entries: ${pack.bugStats.pending}`,
    `- Fixed entries: ${pack.bugStats.fixed}`,
    `- Duplicate entries: ${pack.bugStats.duplicate}`,
    `- Rejected entries: ${pack.bugStats.rejected}`,
    `- Feedback runs: ${pack.feedbackStats.runs}`,
    `- Feedback shards: ${pack.feedbackStats.shards}`,
    `- Replay cases: ${pack.feedbackStats.total}`,
    `- Stable failures: ${pack.feedbackStats.stableFailed}`,
    `- New candidates: ${pack.feedbackStats.newCandidateFindings}`,
    `- Known duplicate findings: ${pack.feedbackStats.knownDuplicateFindings}`,
    `- Actionable failures: ${pack.feedbackStats.actionableFailures}`,
    `- Memory accepted/manual candidates: ${pack.bugMemoryStats.acceptedCandidates}`,
    `- Memory flaky cases: ${pack.bugMemoryStats.flaky}`,
    `- Memory harness cases: ${pack.bugMemoryStats.harness}`,
    `- Memory no-finding cases: ${pack.bugMemoryStats.noFinding}`,
    "",
    "## Runtime Prompt Adjustments",
    "",
    ...runtimeAdjustments(pack),
    "",
    "## Issue/Meeting Notes",
    "",
    ...pack.issueNotes,
    "",
    "## Supplemental Picture Inputs",
    "",
    ...(pack.pictureFiles.length > 0
      ? pack.pictureFiles.map((item) => `- ${item}`)
      : ["- No picture inputs discovered."]),
    "",
    "## Worker Output Contract",
    "",
    "- Explorer workers should output candidate cases only; they should not claim findings.",
    "- Triage workers should emit BUG_LIST_APPEND only when replay evidence crosses the accepted finding standard.",
    "- Reports must include rejected-candidate reasons, not just accepted findings.",
    "- Each accepted item must include a minimal repro, expected behavior, actual behavior, impact, evidence path, and stability.",
    ""
  ];
  return `${lines.join("\n")}\n`;
}

export function promptEvolutionExcerpt(text, limit = 8000) {
  const normalized = String(text || "").trim();
  if (!normalized) return "(none)";
  if (normalized.length <= limit) return normalized;
  return `${normalized.slice(0, limit)}\n...[truncated ${normalized.length - limit} chars]`;
}

function runtimeAdjustments(pack) {
  const lines = [];
  if (pack.feedbackStats.runs === 0) {
    lines.push("- No replay feedback has been recorded in the ledger yet; keep the acceptance bar strict and require direct replay evidence.");
  }
  if (pack.feedbackStats.knownDuplicateFindings > 0 || pack.feedbackStats.duplicateRate > 0.25) {
    lines.push("- Duplicate pressure is non-trivial; triage must compare root cause and user intent before emitting BUG_LIST_APPEND.");
  }
  if (pack.feedbackStats.stableFailed === 0 && pack.feedbackStats.actionableFailures === 0) {
    lines.push("- Recent broad runs produced no stable actionable failures; explorers should bias toward shorter core-loop race probes instead of broad random walks.");
  }
  if (pack.feedbackStats.flakeRate > 0.05) {
    lines.push("- Flake rate is above threshold; require repeated attempts or a deterministic visible stuck/corrupt state before promotion.");
  }
  if (pack.bugMemoryStats.harness > 0) {
    lines.push("- Recent bug memory includes harness/precondition failures; keep these under coverage gaps and do not emit BUG_LIST_APPEND for them.");
  }
  if (pack.bugMemoryStats.noFinding > pack.bugMemoryStats.acceptedCandidates) {
    lines.push("- No-finding pressure is high; planner should bias toward uncovered runtime edges and temporal async invariants rather than broad random walks.");
  }
  if (pack.bugStats.total === 0) {
    lines.push("- The main fuzz bug ledger is empty; accepted reports must be especially explicit so the first entries are high quality.");
  }
  lines.push("- Always separate product bugs from harness gaps. Harness gaps can be reported under coverage gaps but must not become BUG_LIST_APPEND.");
  lines.push("- Prefer one high-signal accepted finding over multiple vague candidates.");
  return lines;
}

function summarizeBugMemory(memory) {
  return {
    reportsScanned: Number(memory?.reportsScanned ?? 0),
    acceptedCandidates: Array.isArray(memory?.acceptedCandidates) ? memory.acceptedCandidates.length : 0,
    duplicates: Array.isArray(memory?.duplicates) ? memory.duplicates.length : 0,
    flaky: Array.isArray(memory?.flaky) ? memory.flaky.length : 0,
    harness: Array.isArray(memory?.harness) ? memory.harness.length : 0,
    noFinding: Number(memory?.noFinding ?? 0)
  };
}

function summarizeBugList(text) {
  const rows = String(text || "").split("\n").filter((line) => line.startsWith("| PUF-FUZZ-"));
  const stats = {
    total: rows.length,
    pending: 0,
    fixed: 0,
    duplicate: 0,
    rejected: 0,
    outOfScope: 0
  };
  for (const row of rows) {
    const parts = row.split("|").map((item) => item.trim());
    const status = parts[2] ?? "";
    if (status === "pending") stats.pending += 1;
    if (status === "fixed") stats.fixed += 1;
    if (status === "duplicate") stats.duplicate += 1;
    if (status === "rejected") stats.rejected += 1;
    if (status === "out-of-scope") stats.outOfScope += 1;
  }
  return stats;
}

function summarizeFeedback(ledger) {
  const runs = Array.isArray(ledger?.runs) ? ledger.runs : [];
  const shards = Object.values(ledger?.shards ?? {});
  const totals = {
    runs: runs.length,
    shards: shards.length,
    total: 0,
    passed: 0,
    stableFailed: 0,
    flaky: 0,
    actionableFailures: 0,
    newCandidateFindings: 0,
    knownDuplicateFindings: 0,
    knownDuplicateFailures: 0,
    duplicateRate: 0,
    flakeRate: 0
  };
  for (const run of runs) {
    totals.total += Number(run.total ?? 0);
    totals.passed += Number(run.passed ?? 0);
    totals.stableFailed += Number(run.stableFailed ?? 0);
    totals.flaky += Number(run.flaky ?? 0);
    totals.actionableFailures += Number(run.actionableFailures ?? 0);
    totals.newCandidateFindings += Number(run.newCandidateFindings ?? 0);
    totals.knownDuplicateFindings += Number(run.knownDuplicateFindings ?? 0);
    totals.knownDuplicateFailures += Number(run.knownDuplicateFailures ?? 0);
  }
  totals.duplicateRate = totals.total === 0 ? 0 : Number((totals.knownDuplicateFailures / totals.total).toFixed(3));
  totals.flakeRate = totals.total === 0 ? 0 : Number((totals.flaky / totals.total).toFixed(3));
  return totals;
}

function summarizeIssue(text) {
  const lines = String(text || "")
    .split("\n")
    .map((line) => line.replace(/\bimage(?: copy(?: \d+)?)?\.png\b/g, "").trim())
    .filter(Boolean)
    .filter((line) => line !== "[]");
  if (lines.length === 0) return ["- No issue notes provided."];
  return lines.slice(0, 16).map((line) => `- ${line.replace(/\s+/g, " ")}`);
}

async function listPictureFiles(dir) {
  if (!dir) return [];
  try {
    return (await readdir(dir, { withFileTypes: true }))
      .filter((entry) => entry.isFile())
      .map((entry) => entry.name)
      .filter((name) => /\.(?:png|jpe?g|webp)$/i.test(name))
      .sort();
  } catch (error) {
    if (error && error.code === "ENOENT") return [];
    throw error;
  }
}

async function readOptional(filePath) {
  if (!filePath) return "";
  try {
    return await readFile(filePath, "utf8");
  } catch (error) {
    if (error && error.code === "ENOENT") return "";
    throw error;
  }
}

async function readJsonOptional(filePath) {
  const text = await readOptional(filePath);
  if (!text.trim()) return {};
  try {
    return JSON.parse(text);
  } catch {
    return {};
  }
}
