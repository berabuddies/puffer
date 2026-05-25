#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { buildNoFindingVerdict, evaluateFindingAdmission, normalizeVerdict } from "../lib/admission-gate.mjs";
import { promptEvolutionExcerpt } from "../lib/prompt-evolution.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const namespace = requireArg(args, "namespace");
const shard = requireArg(args, "shard");
const seed = requireArg(args, "seed");
const model = args.model ?? process.env.PUFFER_OPENROUTER_MODEL ?? "inclusionai/ling-2.6-flash";
const outPath = path.resolve(repoRoot, args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace, "findings.md"));
const verdictPath = path.resolve(repoRoot, args["verdict-out"] ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace, "verdict.json"));
const gatePath = path.resolve(repoRoot, args["gate-out"] ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace, "verdict-gate.json"));
const runDir = path.resolve(fuzzRoot, ".runs", namespace);
const apiKey = process.env.OPENROUTER_API_KEY;
const baseUrl = (process.env.OPENROUTER_BASE_URL ?? "https://openrouter.ai/api/v1").replace(/\/+$/, "");

if (!apiKey) {
  throw new Error("OPENROUTER_API_KEY is required");
}

const artifacts = {
  planner: readOptional(path.join(runDir, "planner.md")),
  top: readOptional(path.join(runDir, "top.md")),
  replay: readOptional(path.join(runDir, "bounded-replay-report.md")),
  replayJson: readOptional(path.join(runDir, "bounded-replay-report.json")),
  report: readOptional(path.join(runDir, "report.md")),
  promptEvolution:
    readOptional(path.join(runDir, "prompt-evolution.md")) ||
    readOptional(path.join(fuzzRoot, "prompt_evolution.md"))
};
const replayData = parseJsonOptional(artifacts.replayJson);
const replaySummary = replayData?.summary ?? {};
const replayFindings = Array.isArray(replayData?.findings) ? replayData.findings : [];
const evidenceIndex = Array.isArray(replayData?.evidence_index) ? replayData.evidence_index : [];

if (!hasActionableReplaySignal(replaySummary, replayFindings)) {
  const verdict = buildNoFindingVerdict({ namespace, shard, seed, replaySummary });
  const gate = evaluateFindingAdmission(verdict, evidenceIndex);
  writeTriageArtifacts({
    namespace,
    shard,
    seed,
    replaySummary,
    artifacts,
    verdict,
    gate
  });
  process.stdout.write(`OPENROUTER_TRIAGE_OK ${relative(outPath)}\n`);
  process.exit(0);
}

const payload = await openRouterChat({
  model,
  temperature: 0.2,
  max_tokens: 4096,
  messages: [
    {
      role: "system",
      content: [
        "You are a small-model UI/UX fuzz shard triager.",
        "Do not plan globally. Do not suggest product code changes.",
        "Only classify the provided shard artifacts and write a precise report.",
        "Accept only user-visible, reproducible interaction blockers from the assigned shard.",
        "Reject fixture-only, environment-only, dependency-only, and tooling-only failures."
      ].join(" ")
    },
    {
      role: "user",
      content: buildPrompt({ namespace, shard, seed, model, artifacts })
    }
  ]
});
const content = payload?.choices?.[0]?.message?.content?.trim();
const verdict = parseVerdictOrDismiss(content, { namespace, shard, seed });
const gate = evaluateFindingAdmission(verdict, evidenceIndex);
writeTriageArtifacts({ namespace, shard, seed, replaySummary, artifacts, verdict, gate, rawContent: content });
process.stdout.write(`OPENROUTER_TRIAGE_OK ${relative(outPath)}\n`);

function buildPrompt({ namespace, shard, seed, model, artifacts }) {
  return [
    `Model: ${model}`,
    `Namespace: ${namespace}`,
    `Shard: ${shard}`,
    `Seed: ${seed}`,
    "",
    "Output strict JSON only. Do not wrap it in Markdown.",
    "The JSON schema is:",
    JSON.stringify(verdictSchema(), null, 2),
    "",
    "Admission requires cited evidence. Every primary_cause and citation must use an id from evidence_index, the exact recorded type, and the recorded sha256 as quote_hash.",
    "A verdict can only use decision=admit when primary_cause cites a predicate evidence entry.",
    "Use decision=candidate when the evidence is real but the primary cause is not a predicate.",
    "Use decision=dismiss for harness failures, environment failures, duplicates, flaky failures, or unclear product impact.",
    "Do not invent evidence ids. Do not cite evidence omitted from the evidence index.",
    "",
    section("Planner guidance", artifacts.planner),
    section("Prompt evolution guidance", promptEvolutionExcerpt(artifacts.promptEvolution, 8000)),
    section("Evidence index", evidenceIndexExcerpt(replayData?.evidence_index ?? [])),
    section("Top replay candidates", artifacts.top),
    section("Bounded replay markdown", artifacts.replay),
    section("Bounded replay JSON", artifacts.replayJson),
    section("Generated fuzz report", artifacts.report)
  ].join("\n");
}

function hasActionableReplaySignal(summary, findings) {
  return Number(summary.actionableFailures ?? 0) > 0 ||
    Number(summary.newCandidateFindings ?? 0) > 0 ||
    Number(summary.productCandidateFindings ?? 0) > 0 ||
    Number(summary.stableFailed ?? 0) > 0 ||
    findings.length > 0;
}

async function openRouterChat(body) {
  for (let attempt = 1; attempt <= 4; attempt += 1) {
    try {
      const response = await fetch(`${baseUrl}/chat/completions`, {
        method: "POST",
        headers: {
          "Authorization": `Bearer ${apiKey}`,
          "Content-Type": "application/json",
          "HTTP-Referer": "https://github.com/berabuddies/puffer",
          "X-Title": "Puffer UIUX Fuzz"
        },
        body: JSON.stringify(body)
      });
      const bodyText = await response.text();
      if (!response.ok) {
        throw new Error(`OpenRouter request failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
      }
      return JSON.parse(bodyText);
    } catch (error) {
      if (attempt === 4) throw error;
      await sleep(750 * attempt);
    }
  }
  throw new Error("OpenRouter request failed");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function deterministicNoFindingReport({ namespace, shard, seed, replaySummary, artifacts, verdict, gate }) {
  return [
    "## Commands and replay cases reviewed",
    `- Namespace: ${namespace}`,
    `- Shard: ${shard}`,
    `- Seed: ${seed}`,
    `- Replay cases: ${Number(replaySummary.total ?? 0)}`,
    `- Passed: ${Number(replaySummary.passed ?? 0)}`,
    `- Stable failed: ${Number(replaySummary.stableFailed ?? 0)}`,
    `- New candidate findings: ${Number(replaySummary.newCandidateFindings ?? 0)}`,
    `- Product candidate findings: ${Number(replaySummary.productCandidateFindings ?? 0)}`,
    `- Non-passing failures: ${Number(replaySummary.nonPassingFailures ?? 0)}`,
    `- Actionable product failures: ${Number(replaySummary.actionableFailures ?? 0)}`,
    "",
    "## Accepted findings",
    "",
    "No accepted findings. Bounded replay did not report any new candidate, product candidate, stable failure, or actionable failure.",
    "",
    "## Structured verdict",
    "",
    "```json",
    JSON.stringify(verdict, null, 2),
    "```",
    "",
    "## Citation gate",
    "",
    "```json",
    JSON.stringify(gate, null, 2),
    "```",
    "",
    "## Rejected candidates",
    "",
    "- None promoted by bounded replay.",
    "",
    "## Coverage gaps",
    "",
    coverageExcerpt(artifacts.top || artifacts.report),
    ""
  ].join("\n");
}

function writeTriageArtifacts({ namespace, shard, seed, replaySummary, artifacts, verdict, gate, rawContent }) {
  const normalized = normalizeVerdict(verdict);
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.mkdirSync(path.dirname(verdictPath), { recursive: true });
  fs.mkdirSync(path.dirname(gatePath), { recursive: true });
  fs.writeFileSync(verdictPath, `${JSON.stringify(normalized, null, 2)}\n`);
  fs.writeFileSync(gatePath, `${JSON.stringify(gate, null, 2)}\n`);
  fs.writeFileSync(outPath, formatFindingReport({
    namespace,
    shard,
    seed,
    replaySummary,
    artifacts,
    verdict: normalized,
    gate,
    rawContent
  }));
}

function formatFindingReport({ namespace, shard, seed, replaySummary, artifacts, verdict, gate, rawContent }) {
  if (verdict.decision === "dismiss" && gate.disposition === "dismissed") {
    return deterministicNoFindingReport({ namespace, shard, seed, replaySummary, artifacts, verdict, gate });
  }
  const lines = [
    "## Commands and replay cases reviewed",
    `- Namespace: ${namespace}`,
    `- Shard: ${shard}`,
    `- Seed: ${seed}`,
    `- Replay cases: ${Number(replaySummary.total ?? 0)}`,
    `- Actionable product failures: ${Number(replaySummary.actionableFailures ?? 0)}`,
    "",
    "## Structured verdict",
    "",
    "```json",
    JSON.stringify(verdict, null, 2),
    "```",
    "",
    "## Citation gate",
    "",
    "```json",
    JSON.stringify(gate, null, 2),
    "```",
    "",
    "## Accepted findings",
    ""
  ];
  if (gate.disposition === "admitted") {
    lines.push(
      "Admitted by citation gate. Main-agent ledger append command:",
      "",
      "```sh",
      "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list \\",
      "  --append-from-verdict \\",
      `  --verdict apps/puffer-desktop/tests/fuzz/.runs/${namespace}/verdict.json \\`,
      `  --gate apps/puffer-desktop/tests/fuzz/.runs/${namespace}/verdict-gate.json`,
      "```"
    );
  } else {
    lines.push(`No admitted findings. Gate disposition: ${gate.disposition}.`);
  }
  lines.push("", "## Rejected candidates", "");
  if (gate.failureReasons?.length) {
    for (const reason of gate.failureReasons) lines.push(`- ${reason}`);
  } else {
    lines.push("- None");
  }
  if (rawContent) {
    lines.push("", "## Raw model output", "", "```text", rawContent, "```");
  }
  lines.push("", "## Coverage gaps", "", coverageExcerpt(artifacts.top || artifacts.report), "");
  return `${lines.join("\n")}\n`;
}

function parseVerdictOrDismiss(content, context) {
  if (!content) {
    return {
      version: 1,
      decision: "dismiss",
      title: "Triage model returned no verdict",
      severity: "P3",
      area: context.shard,
      shard: context.shard,
      source_run: context.namespace,
      primary_cause: null,
      citations: [],
      expected: "Triage should return strict JSON.",
      actual: "OpenRouter response did not include message content.",
      impact: "No product bug can be admitted without a structured verdict.",
      repro: [],
      notes: `Seed ${context.seed}`
    };
  }
  const jsonText = extractJsonObject(content);
  try {
    return JSON.parse(jsonText);
  } catch (error) {
    return {
      version: 1,
      decision: "dismiss",
      title: "Triage model returned invalid JSON",
      severity: "P3",
      area: context.shard,
      shard: context.shard,
      source_run: context.namespace,
      primary_cause: null,
      citations: [],
      expected: "Triage should return strict JSON.",
      actual: String(error?.message ?? error),
      impact: "Invalid triage output is not admitted.",
      repro: [],
      notes: content.slice(0, 500)
    };
  }
}

function extractJsonObject(content) {
  const fenced = String(content).match(/```(?:json)?\s*([\s\S]*?)```/i);
  const text = fenced ? fenced[1] : String(content);
  const start = text.indexOf("{");
  const end = text.lastIndexOf("}");
  if (start < 0 || end < start) return text;
  return text.slice(start, end + 1);
}

function verdictSchema() {
  return {
    version: 1,
    decision: "admit|candidate|dismiss",
    title: "short user-visible bug title",
    severity: "P0|P1|P2|P3",
    area: "component or flow",
    shard,
    source_run: namespace,
    primary_cause: {
      id: "ev-predicate-0001",
      type: "predicate",
      quote_hash: "sha256 from evidence_index"
    },
    citations: [
      {
        id: "ev-action-0001",
        type: "action",
        quote_hash: "sha256 from evidence_index"
      }
    ],
    expected: "expected behavior",
    actual: "actual behavior",
    impact: "user impact",
    repro: ["minimal step 1", "minimal step 2"],
    notes: "duplicate/out-of-shard/source pointers if relevant"
  };
}

function evidenceIndexExcerpt(entries) {
  if (!Array.isArray(entries) || entries.length === 0) return "(missing)";
  return JSON.stringify(entries.slice(0, 80).map((entry) => ({
    id: entry.id,
    type: entry.type,
    byte_span: entry.byte_span,
    sha256: entry.sha256,
    value: String(entry.value ?? "").slice(0, 600)
  })), null, 2);
}

function coverageExcerpt(text) {
  const lines = String(text || "").split("\n");
  const start = lines.findIndex((line) => line.trim() === "## Coverage");
  if (start < 0) return "- Coverage summary unavailable.";
  const next = lines.findIndex((line, index) => index > start && line.startsWith("## "));
  return lines.slice(start + 1, next < 0 ? start + 20 : next).join("\n").trim() || "- Coverage summary unavailable.";
}

function section(title, text) {
  return [`## ${title}`, truncate(text || "(missing)", 12000)].join("\n");
}

function truncate(text, limit) {
  if (text.length <= limit) return text;
  return `${text.slice(0, limit)}\n...[truncated ${text.length - limit} chars]`;
}

function readOptional(filePath) {
  try {
    return fs.readFileSync(filePath, "utf8");
  } catch (error) {
    if (error && error.code === "ENOENT") return "";
    throw error;
  }
}

function parseJsonOptional(text) {
  if (!text.trim()) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const key = argv[index];
    if (!key.startsWith("--")) continue;
    const name = key.slice(2);
    const value = argv[index + 1];
    if (value === undefined || value.startsWith("--")) {
      parsed[name] = "true";
    } else {
      parsed[name] = value;
      index += 1;
    }
  }
  return parsed;
}

function requireArg(args, name) {
  const value = args[name];
  if (!value) throw new Error(`--${name} is required`);
  return value;
}

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
