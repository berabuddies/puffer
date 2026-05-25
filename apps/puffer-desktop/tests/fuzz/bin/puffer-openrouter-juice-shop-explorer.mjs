#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { readOpenRouterApiKey } from "../lib/openrouter-auth.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const suitePath = path.resolve(repoRoot, String(args.suite ?? "apps/puffer-desktop/tests/fuzz/benchmarks/juice_shop_shards.json"));
const shardId = requireArg(args, "shard-id");
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
const shard = (suite.shards ?? []).find((item) => item.id === shardId);
if (!shard) throw new Error(`Unknown Juice Shop shard: ${shardId}`);
if (!apiKey && !offlineSmoke) throw new Error("OPENROUTER_API_KEY or PUFFER_OPENROUTER_API_KEY_FILE is required");

const plan = offlineSmoke ? offlinePlan() : await resilientModelPlan();
fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(jsonOutPath, `${JSON.stringify(plan, null, 2)}\n`);
fs.writeFileSync(outPath, formatPlan(plan));
process.stdout.write(`OPENROUTER_JUICE_EXPLORER_OK ${relative(outPath)}\n`);

async function resilientModelPlan() {
  try {
    return await modelPlan();
  } catch (caught) {
    const plan = offlinePlan();
    plan.fixture_risks.push(`model planner fallback: ${String(caught?.message ?? caught).slice(0, 500)}`);
    return plan;
  }
}

async function modelPlan() {
  const response = await openRouterChat({
    model,
    temperature: 0.2,
    max_tokens: 2048,
    messages: [
      {
        role: "system",
        content: [
          "You are a cheap GUI/security benchmark explorer subagent for OWASP Juice Shop.",
          "You own exactly one UI-tree shard. Do not inspect unrelated shards.",
          "Return strict JSON only. Do not patch code, edit ledgers, or claim success without native challenge-score evidence.",
          "Allowed action types are goto, click, fill, press, wait, request, and submitFeedback.",
          "Prefer the shard's offline_actions exactly when present because they are native-score seed actions.",
          "Only deviate from offline_actions when your replacement is still inside allowed_paths and is more likely to change /api/Challenges."
        ].join(" ")
      },
      {
        role: "user",
        content: [
          `Namespace: ${namespace}`,
          `Suite: ${suite.name}`,
          `Shard: ${shardId}`,
          "",
          "Shard JSON:",
          JSON.stringify(shard, null, 2),
          "",
          "Important: actions must be bounded to this shard. If offline_actions is non-empty, copy those actions unless there is a clear native-score reason to do otherwise.",
          "",
          "Return JSON with this exact shape:",
          JSON.stringify(schema(), null, 2)
        ].join("\n")
      }
    ]
  });
  const content = response?.choices?.[0]?.message?.content ?? "";
  return normalizePlan(parseStrictJson(content));
}

async function openRouterChat(body) {
  let lastError = null;
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
          "X-Title": "Puffer Juice Shop UIUX Fuzz"
        },
        body: JSON.stringify(body)
      });
      const bodyText = await response.text();
      if (!response.ok) throw new Error(`OpenRouter Juice Shop explorer failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
      return JSON.parse(bodyText);
    } catch (error) {
      lastError = error;
      if (attempt !== requestAttempts) await sleep(1000 * attempt * attempt);
    } finally {
      clearTimeout(timeout);
    }
  }
  throw lastError ?? new Error("OpenRouter Juice Shop explorer failed");
}

function offlinePlan() {
  return normalizePlan({
    version: 1,
    namespace,
    shard_id: shardId,
    shard_path: `juice-shop/${shard.area}/${shard.id}`,
    owned_scope: ["juice-shop", shard.area, shard.id],
    goal: shard.goal,
    target_challenges: shard.target_challenges ?? [],
    allowed_paths: shard.allowed_paths ?? [],
    actions: fallbackActions(),
    fixture_risks: ["container startup timeout", "selector drift", "challenge disabled in Docker"]
  });
}

function fallbackActions() {
  if (Array.isArray(shard.offline_actions) && shard.offline_actions.length > 0) return shard.offline_actions;
  const paths = Array.isArray(shard.allowed_paths) ? shard.allowed_paths : [];
  const preferred = paths.find((item) => item !== "/api/Challenges") ?? "/";
  return [{ type: "goto", path: preferred }, { type: "wait", ms: 1000 }];
}

function normalizePlan(plan) {
  const actions = Array.isArray(plan.actions) && plan.actions.length > 0 ? plan.actions : shard.offline_actions ?? [];
  return {
    version: 1,
    namespace,
    shard_id: shardId,
    shard_path: String(plan.shard_path ?? `juice-shop/${shard.area}/${shard.id}`),
    owned_scope: Array.isArray(plan.owned_scope) ? plan.owned_scope.map(String) : ["juice-shop", shard.area, shard.id],
    goal: String(plan.goal ?? shard.goal ?? ""),
    target_challenges: Array.isArray(plan.target_challenges) ? plan.target_challenges.map(String) : shard.target_challenges ?? [],
    allowed_paths: Array.isArray(plan.allowed_paths) ? plan.allowed_paths.map(String) : shard.allowed_paths ?? [],
    actions,
    fixture_risks: Array.isArray(plan.fixture_risks) ? plan.fixture_risks.map(String) : []
  };
}

function schema() {
  return {
    version: 1,
    namespace: "string",
    shard_id: "string",
    shard_path: "juice-shop/area/shard",
    owned_scope: ["juice-shop", "area", "shard"],
    goal: "short goal",
    target_challenges: ["nativeChallengeKey"],
    allowed_paths: ["/allowed/path"],
    actions: [
      { type: "goto", path: "/relative-path" },
      { type: "fill", selector: "#email", value: "text" },
      { type: "click", selector: "#loginButton" },
      { type: "request", method: "POST", path: "/api/...", json: {} },
      { type: "submitFeedback", comment: "text", rating: 0 },
      { type: "wait", ms: 500 }
    ],
    fixture_risks: ["risk"]
  };
}

function parseStrictJson(text) {
  const trimmed = String(text).trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) return JSON.parse(trimmed);
  const objectText = firstJsonObject(trimmed);
  if (objectText) return JSON.parse(objectText);
  throw new Error("Juice Shop explorer did not return JSON");
}

function firstJsonObject(text) {
  const start = text.indexOf("{");
  if (start === -1) return "";
  let depth = 0;
  let inString = false;
  let escaped = false;
  for (let index = start; index < text.length; index += 1) {
    const char = text[index];
    if (escaped) {
      escaped = false;
      continue;
    }
    if (char === "\\") {
      escaped = true;
      continue;
    }
    if (char === "\"") {
      inString = !inString;
      continue;
    }
    if (inString) continue;
    if (char === "{") depth += 1;
    if (char === "}") {
      depth -= 1;
      if (depth === 0) return text.slice(start, index + 1);
    }
  }
  return "";
}

function formatPlan(plan) {
  return [
    "# Juice Shop Shard Explorer Plan",
    "",
    `- Namespace: ${plan.namespace}`,
    `- Shard: ${plan.shard_id}`,
    `- Shard path: ${plan.shard_path}`,
    `- Goal: ${plan.goal}`,
    `- Target challenges: ${plan.target_challenges.join(", ")}`,
    "",
    "## Actions",
    ...plan.actions.map((item, index) => `${index + 1}. ${JSON.stringify(item)}`),
    "",
    "## Fixture Risks",
    ...plan.fixture_risks.map((item) => `- ${item}`),
    ""
  ].join("\n");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
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
