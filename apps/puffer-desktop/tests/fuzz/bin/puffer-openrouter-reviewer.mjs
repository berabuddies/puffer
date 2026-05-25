#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { REVIEWER_DECISIONS, parseStrictJsonObject } from "../lib/admission-gate.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const verdictPath = path.resolve(repoRoot, requireArg(args, "verdict"));
const gatePath = path.resolve(repoRoot, requireArg(args, "gate"));
const replayPath = path.resolve(repoRoot, requireArg(args, "replay"));
const outPath = path.resolve(repoRoot, args.out ?? path.join(path.dirname(verdictPath), "reviewer.json"));
const model = args.model ?? process.env.PUFFER_OPENROUTER_REVIEWER_MODEL ??
  process.env.PUFFER_OPENROUTER_MODEL ?? "inclusionai/ling-2.6-flash";
const apiKey = process.env.OPENROUTER_API_KEY;
const offlineReview = args.offline === "true" || process.env.PUFFER_OPENROUTER_REVIEWER_OFFLINE === "1";
const baseUrl = (process.env.OPENROUTER_BASE_URL ?? "https://openrouter.ai/api/v1").replace(/\/+$/, "");

const verdict = JSON.parse(fs.readFileSync(verdictPath, "utf8"));
const gate = JSON.parse(fs.readFileSync(gatePath, "utf8"));
const replay = JSON.parse(fs.readFileSync(replayPath, "utf8"));

if (gate.disposition !== "candidate") {
  writeReview({
    version: 1,
    decision: "dismiss",
    confidence: 1,
    reason: `Gate disposition is ${gate.disposition}, not candidate.`,
    cited_evidence: [],
    notes: "Reviewer is only used for predicate-missing candidate verdicts."
  });
  process.stdout.write(`OPENROUTER_REVIEWER_OK ${relative(outPath)}\n`);
  process.exit(0);
}

const evidence = citedEvidence(verdict, replay.evidence_index ?? []);
if (offlineReview) {
  writeReview({
    version: 1,
    decision: evidence.length > 0 ? "human_queue" : "dismiss",
    confidence: evidence.length > 0 ? 0.5 : 1,
    reason: evidence.length > 0
      ? "Offline reviewer smoke saw cited evidence but did not make a network-backed promotion decision."
      : "Offline reviewer smoke found no cited evidence.",
    cited_evidence: evidence.map((entry) => entry.id),
    notes: "Offline reviewer smoke validates candidate-review artifact shape only."
  });
  process.stdout.write(`OPENROUTER_REVIEWER_OK ${relative(outPath)}\n`);
  process.exit(0);
}

if (!apiKey) throw new Error("OPENROUTER_API_KEY is required");
const payload = await openRouterChat({
  model,
  temperature: 0.1,
  max_tokens: 2048,
  messages: [
    {
      role: "system",
      content: [
        "You are an adversarial reviewer for GUI fuzz candidates.",
        "You only see cited evidence, not the full report.",
        "Do not suggest fixes. Decide whether this candidate deserves promotion.",
        "Dismiss fixture-only, environment-only, duplicate, flaky, and unsupported claims."
      ].join(" ")
    },
    {
      role: "user",
      content: buildPrompt({ verdict, gate, evidence })
    }
  ]
});
const content = payload?.choices?.[0]?.message?.content?.trim();
writeReview(parseReviewOrDismiss(content));
process.stdout.write(`OPENROUTER_REVIEWER_OK ${relative(outPath)}\n`);

function buildPrompt({ verdict, gate, evidence }) {
  return [
    "Output strict JSON only with this schema:",
    JSON.stringify({
      version: 1,
      decision: "admit|dismiss|human_queue",
      confidence: "0.0-1.0",
      reason: "short reason",
      cited_evidence: ["evidence ids used"],
      notes: "review notes"
    }, null, 2),
    "",
    "Candidate verdict:",
    JSON.stringify(verdict, null, 2),
    "",
    "Gate result:",
    JSON.stringify(gate, null, 2),
    "",
    "Cited evidence narrow windows:",
    JSON.stringify(evidence, null, 2)
  ].join("\n");
}

async function openRouterChat(body) {
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await fetch(`${baseUrl}/chat/completions`, {
        method: "POST",
        headers: {
          "Authorization": `Bearer ${apiKey}`,
          "Content-Type": "application/json",
          "HTTP-Referer": "https://github.com/berabuddies/puffer",
          "X-Title": "Puffer UIUX Fuzz Reviewer"
        },
        body: JSON.stringify(body)
      });
      const bodyText = await response.text();
      if (!response.ok) {
        throw new Error(`OpenRouter reviewer request failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
      }
      return JSON.parse(bodyText);
    } catch (error) {
      if (attempt === 3) throw error;
      await sleep(750 * attempt);
    }
  }
  throw new Error("OpenRouter reviewer request failed");
}

function citedEvidence(verdict, evidenceIndex) {
  const ids = new Set([
    verdict.primary_cause?.id,
    ...(verdict.citations ?? []).map((citation) => citation.id)
  ].filter(Boolean));
  return evidenceIndex
    .filter((entry) => ids.has(entry.id))
    .map((entry) => ({
      id: entry.id,
      type: entry.type,
      byte_span: entry.byte_span,
      sha256: entry.sha256,
      value: String(entry.value ?? "").slice(0, 1200),
      metadata: entry.metadata ?? {}
    }));
}

function parseReviewOrDismiss(content) {
  try {
    const parsed = parseStrictJsonObject(content ?? "", "reviewer response");
    const decision = REVIEWER_DECISIONS.has(parsed.decision) ? parsed.decision : "dismiss";
    return {
      version: 1,
      decision,
      confidence: clamp(Number(parsed.confidence ?? 0), 0, 1),
      reason: String(parsed.reason ?? ""),
      cited_evidence: Array.isArray(parsed.cited_evidence) ? parsed.cited_evidence.map(String) : [],
      notes: String(parsed.notes ?? "")
    };
  } catch (error) {
    return {
      version: 1,
      decision: "dismiss",
      confidence: 1,
      reason: `Reviewer returned invalid JSON: ${String(error?.message ?? error)}`,
      cited_evidence: [],
      notes: String(content ?? "").slice(0, 500)
    };
  }
}

function writeReview(review) {
  fs.mkdirSync(path.dirname(outPath), { recursive: true });
  fs.writeFileSync(outPath, `${JSON.stringify(review, null, 2)}\n`);
}

function clamp(value, min, max) {
  if (!Number.isFinite(value)) return min;
  return Math.max(min, Math.min(max, value));
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

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
