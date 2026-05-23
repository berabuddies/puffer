import { readdir, readFile } from "node:fs/promises";
import path from "node:path";
import { writeJson, writeText } from "./fuzz-core.mjs";

export async function buildBugMemory(options = {}) {
  const runsDir = options.runsDir;
  const limit = Number(options.limit ?? 500);
  const reports = runsDir ? await findReplayReports(runsDir, limit) : [];
  const memory = {
    version: 1,
    generatedAt: new Date().toISOString(),
    reportsScanned: reports.length,
    classes: {},
    acceptedCandidates: [],
    duplicates: [],
    flaky: [],
    harness: [],
    noFinding: 0
  };
  for (const reportPath of reports) {
    const report = JSON.parse(await readFile(reportPath, "utf8"));
    for (const result of report.results ?? []) {
      const classification = result.classification ?? "unknown";
      memory.classes[classification] = (memory.classes[classification] ?? 0) + 1;
      const row = {
        reportPath,
        namespace: report.namespace ?? "",
        caseId: result.caseId ?? "",
        seed: result.seed ?? "",
        status: result.status ?? "",
        classification,
        failureSignature: result.failureSignature ?? "",
        coverage: result.coverage ?? []
      };
      if (classification === "no-finding") {
        memory.noFinding += 1;
      } else if (result.knownDuplicate) {
        memory.duplicates.push(row);
      } else if (classification.startsWith("product-candidate:") || classification.startsWith("needs-manual-triage")) {
        memory.acceptedCandidates.push(row);
      } else if (classification.startsWith("flaky")) {
        memory.flaky.push(row);
      } else if (classification.startsWith("harness-precondition")) {
        memory.harness.push(row);
      }
    }
  }
  return memory;
}

export async function writeBugMemory(filePath, memory) {
  await writeJson(filePath, memory);
}

export async function writeBugMemoryMarkdown(filePath, memory) {
  await writeText(filePath, formatBugMemoryMarkdown(memory));
}

export function formatBugMemoryMarkdown(memory) {
  const lines = [
    "# Puffer UI/UX Fuzz Bug Memory",
    "",
    `Generated: ${memory.generatedAt}`,
    `Replay reports scanned: ${memory.reportsScanned}`,
    `No-finding cases: ${memory.noFinding}`,
    `Accepted/manual candidates: ${memory.acceptedCandidates.length}`,
    `Known duplicates: ${memory.duplicates.length}`,
    `Flaky cases: ${memory.flaky.length}`,
    `Harness/precondition cases: ${memory.harness.length}`,
    "",
    "## Classification Counts",
    ""
  ];
  for (const [classification, count] of Object.entries(memory.classes).sort()) {
    lines.push(`- ${classification}: ${count}`);
  }
  appendRows(lines, "Accepted Or Manual Candidates", memory.acceptedCandidates);
  appendRows(lines, "Duplicate Examples", memory.duplicates.slice(0, 20));
  appendRows(lines, "Flaky Examples", memory.flaky.slice(0, 20));
  appendRows(lines, "Harness Examples", memory.harness.slice(0, 20));
  return `${lines.join("\n")}\n`;
}

async function findReplayReports(rootDir, limit) {
  const reports = [];
  async function visit(current) {
    if (reports.length >= limit) return;
    let entries;
    try {
      entries = await readdir(current, { withFileTypes: true });
    } catch {
      return;
    }
    for (const entry of entries) {
      if (reports.length >= limit) break;
      const entryPath = path.join(current, entry.name);
      if (entry.isDirectory()) {
        await visit(entryPath);
      } else if (entry.name === "bounded-replay-report.json") {
        reports.push(entryPath);
      }
    }
  }
  await visit(rootDir);
  return reports.sort();
}

function appendRows(lines, title, rows) {
  lines.push("", `## ${title}`, "");
  if (rows.length === 0) {
    lines.push("- None");
    return;
  }
  for (const row of rows) {
    lines.push(`- ${row.namespace}/${row.caseId}: ${row.status}; ${row.classification}; ${row.failureSignature || "no signature"}`);
  }
}
