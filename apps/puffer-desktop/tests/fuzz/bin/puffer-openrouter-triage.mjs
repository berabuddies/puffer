#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { promptEvolutionExcerpt } from "../lib/prompt-evolution.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const namespace = requireArg(args, "namespace");
const shard = requireArg(args, "shard");
const seed = requireArg(args, "seed");
const model = args.model ?? process.env.PUFFER_OPENROUTER_MODEL ?? "inclusionai/ling-2.6-flash";
const outPath = path.resolve(repoRoot, args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace, "findings.md"));
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

if (!hasActionableReplaySignal(replaySummary, replayFindings)) {
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, deterministicNoFindingReport({
    namespace,
    shard,
    seed,
    replaySummary,
    artifacts
  }));
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
if (!content) {
  throw new Error("OpenRouter response did not include message content");
}

fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(outPath, `${content}\n`);
process.stdout.write(`OPENROUTER_TRIAGE_OK ${relative(outPath)}\n`);

function buildPrompt({ namespace, shard, seed, model, artifacts }) {
  return [
    `Model: ${model}`,
    `Namespace: ${namespace}`,
    `Shard: ${shard}`,
    `Seed: ${seed}`,
    "",
    "Output Markdown with these sections:",
    "1. Commands and replay cases reviewed",
    "2. Accepted findings",
    "3. Rejected candidates",
    "4. Coverage gaps",
    "",
    "For each accepted finding, include this exact block:",
    "BUG_LIST_APPEND",
    "title: <short user-visible bug title>",
    "status: pending",
    "severity: P0|P1|P2",
    "area: <component or flow>",
    `shard: ${shard}`,
    `source-run: ${namespace}`,
    `evidence: apps/puffer-desktop/tests/fuzz/.runs/${namespace}/findings.md`,
    "stability: <e.g. 2/2>",
    "expected: <expected behavior>",
    "actual: <actual behavior>",
    "impact: <user impact>",
    "repro: <minimal steps>",
    "notes: <duplicate/out-of-shard/source pointers if relevant>",
    "END_BUG_LIST_APPEND",
    "",
    "If there are no accepted findings, say so explicitly and do not emit BUG_LIST_APPEND.",
    "Do not emit BUG_LIST_APPEND unless bounded replay reports a new candidate, product candidate, stable failure, or actionable failure.",
    "",
    section("Planner guidance", artifacts.planner),
    section("Prompt evolution guidance", promptEvolutionExcerpt(artifacts.promptEvolution, 8000)),
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

function deterministicNoFindingReport({ namespace, shard, seed, replaySummary, artifacts }) {
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
    "No accepted findings. Bounded replay did not report any new candidate, product candidate, stable failure, or actionable failure, so BUG_LIST_APPEND is intentionally suppressed.",
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
