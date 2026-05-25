#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { readOpenRouterApiKey } from "../lib/openrouter-auth.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const suitePath = path.resolve(repoRoot, requireArg(args, "suite"));
const caseId = requireArg(args, "case-id");
const namespace = requireArg(args, "namespace");
const outPath = path.resolve(repoRoot, args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace, "worker-plan.md"));
const jsonOutPath = path.resolve(repoRoot, args["json-out"] ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace, "worker-plan.json"));
const model = args.model ?? process.env.PUFFER_OPENROUTER_MODEL ?? "inclusionai/ling-2.6-flash";
const baseUrl = (process.env.OPENROUTER_BASE_URL ?? "https://openrouter.ai/api/v1").replace(/\/+$/, "");
const offlineSmoke = args.offline === "true" || process.env.PUFFER_OPENROUTER_OFFLINE_SMOKE === "1";
const apiKey = readOpenRouterApiKey();
const requestAttempts = Math.max(1, Number(process.env.PUFFER_OPENROUTER_REQUEST_ATTEMPTS ?? 4));
const requestTimeoutMs = Math.max(1000, Number(process.env.PUFFER_OPENROUTER_REQUEST_TIMEOUT_MS ?? 30000));
const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
const testCase = (suite.cases ?? []).find((item) => item.id === caseId);
if (!testCase) throw new Error(`Unknown benchmark case: ${caseId}`);
if (!apiKey && !offlineSmoke) throw new Error("OPENROUTER_API_KEY or PUFFER_OPENROUTER_API_KEY_FILE is required");

const plan = offlineSmoke ? offlinePlan() : await modelPlan();
fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(jsonOutPath, `${JSON.stringify(plan, null, 2)}\n`);
fs.writeFileSync(outPath, formatPlan(plan));
process.stdout.write(`OPENROUTER_BENCHMARK_EXPLORER_OK ${relative(outPath)}\n`);

async function modelPlan() {
  const response = await openRouterChat({
    model,
    temperature: 0.2,
    max_tokens: 2048,
    messages: [
      {
        role: "system",
        content: [
          "You are a cheap GUI benchmark explorer subagent.",
          "You own exactly one UI-tree shard: benchmark/app/case/oracle.",
          "Do not edit files, do not patch product code, do not write BUGS.md.",
          "Return strict JSON only. Your job is to focus the deterministic replay and flag likely fixture risks."
        ].join(" ")
      },
      {
        role: "user",
        content: [
          `Namespace: ${namespace}`,
          `Suite: ${suite.name}`,
          `Case: ${caseId}`,
          "",
          "Case JSON:",
          JSON.stringify(testCase, null, 2),
          "",
          "Return JSON with this shape:",
          JSON.stringify(schema(), null, 2)
        ].join("\n")
      }
    ]
  });
  const content = response?.choices?.[0]?.message?.content ?? "";
  return normalizePlan(parseStrictJson(content));
}

async function openRouterChat(body) {
  for (let attempt = 1; attempt <= requestAttempts; attempt += 1) {
    const controller = new AbortController();
    const timeout = setTimeout(() => controller.abort(), requestTimeoutMs);
    try {
      const response = await fetch(`${baseUrl}/chat/completions`, {
        method: "POST",
        signal: controller.signal,
        headers: {
          "Authorization": `Bearer ${apiKey}`,
          "Content-Type": "application/json",
          "HTTP-Referer": "https://github.com/berabuddies/puffer",
          "X-Title": "Puffer UIUX Benchmark Fuzz"
        },
        body: JSON.stringify(body)
      });
      const bodyText = await response.text();
      if (!response.ok) throw new Error(`OpenRouter benchmark explorer failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
      return JSON.parse(bodyText);
    } catch (error) {
      if (attempt === requestAttempts) throw error;
      await sleep(1000 * attempt * attempt);
    } finally {
      clearTimeout(timeout);
    }
  }
  throw new Error("OpenRouter benchmark explorer failed");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

function offlinePlan() {
  return normalizePlan({
    version: 1,
    namespace,
    case_id: caseId,
    shard_path: `benchmark/${suite.name}/${testCase.app}/${caseId}`,
    owned_scope: [suite.name, testCase.app, caseId],
    execution_focus: "Run the manifest actions exactly and evaluate the benchmark oracle.",
    likely_bug_signal: testCase.gold_bug ? "The gold label expects a deterministic oracle failure." : "The gold label expects the oracle to pass.",
    fixture_risks: ["local app checkout missing", "npm dependencies missing", "selector drift"],
    replay_expectations: (testCase.expectations ?? []).map((item) => JSON.stringify(item))
  });
}

function normalizePlan(plan) {
  return {
    version: 1,
    namespace,
    case_id: caseId,
    shard_path: String(plan.shard_path ?? `benchmark/${suite.name}/${testCase.app}/${caseId}`),
    owned_scope: Array.isArray(plan.owned_scope) ? plan.owned_scope.map(String) : [suite.name, testCase.app, caseId],
    execution_focus: String(plan.execution_focus ?? "Run deterministic benchmark replay."),
    likely_bug_signal: String(plan.likely_bug_signal ?? ""),
    fixture_risks: Array.isArray(plan.fixture_risks) ? plan.fixture_risks.map(String) : [],
    replay_expectations: Array.isArray(plan.replay_expectations) ? plan.replay_expectations.map(String) : []
  };
}

function parseStrictJson(text) {
  const trimmed = String(text).trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) throw new Error("benchmark explorer did not return raw JSON");
  return JSON.parse(trimmed);
}

function schema() {
  return {
    version: 1,
    namespace: "string",
    case_id: "string",
    shard_path: "benchmark/suite/app/case",
    owned_scope: ["suite", "app", "case"],
    execution_focus: "short replay strategy",
    likely_bug_signal: "what oracle failure would mean",
    fixture_risks: ["risk"],
    replay_expectations: ["expectation summary"]
  };
}

function formatPlan(plan) {
  return [
    "# Benchmark Shard Explorer Plan",
    "",
    `- Namespace: ${plan.namespace}`,
    `- Case: ${plan.case_id}`,
    `- Shard path: ${plan.shard_path}`,
    `- Owned scope: ${plan.owned_scope.join(" / ")}`,
    `- Execution focus: ${plan.execution_focus}`,
    `- Likely bug signal: ${plan.likely_bug_signal}`,
    "",
    "## Fixture Risks",
    ...plan.fixture_risks.map((item) => `- ${item}`),
    "",
    "## Replay Expectations",
    ...plan.replay_expectations.map((item) => `- ${item}`),
    ""
  ].join("\n");
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

function requireArg(parsed, key) {
  if (!parsed[key]) throw new Error(`--${key} is required`);
  return String(parsed[key]);
}

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
