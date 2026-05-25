#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { readOpenRouterApiKey } from "../lib/openrouter-auth.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const suitePath = path.resolve(repoRoot, String(args.suite ?? "apps/puffer-desktop/tests/fuzz/benchmarks/webarena_smoke_suite.json"));
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
if (!shard) throw new Error(`Unknown WebArena shard: ${shardId}`);
if (!apiKey && !offlineSmoke) throw new Error("OPENROUTER_API_KEY or PUFFER_OPENROUTER_API_KEY_FILE is required");
const task = loadTask(shard);

const plan = offlineSmoke ? offlinePlan() : await resilientModelPlan();
fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(jsonOutPath, `${JSON.stringify(plan, null, 2)}\n`);
fs.writeFileSync(outPath, formatPlan(plan));
process.stdout.write(`OPENROUTER_WEBARENA_EXPLORER_OK ${relative(outPath)}\n`);

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
          "You are a cheap WebArena GUI benchmark explorer subagent.",
          "You own exactly one WebArena UI-tree shard. Do not inspect unrelated shards.",
          "Return strict JSON only. Do not patch code, edit ledgers, or claim success without evaluator-compatible evidence.",
          "Allowed action types are goto, click, fill, select, press, wait, and stop.",
          "For full WebArena tasks, keep the plan short and bounded to the task intent.",
          "The runner evaluates url_match, string_match, and program_html from the shard config."
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
          JSON.stringify(redactShardForWorker(shard, task), null, 2),
          "",
          "Important: actions must stay inside this task's start_url and intent. If offline_actions is non-empty, copy those actions unless you have a clearer evaluator-compatible path.",
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
          "X-Title": "Puffer WebArena UIUX Fuzz"
        },
        body: JSON.stringify(body)
      });
      const bodyText = await response.text();
      if (!response.ok) throw new Error(`OpenRouter WebArena explorer failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
      return JSON.parse(bodyText);
    } catch (error) {
      lastError = error;
      if (attempt !== requestAttempts) await sleep(1000 * attempt * attempt);
    } finally {
      clearTimeout(timeout);
    }
  }
  throw lastError ?? new Error("OpenRouter WebArena explorer failed");
}

function offlinePlan() {
  return normalizePlan({
    version: 1,
    namespace,
    shard_id: shardId,
    shard_path: shard.shard_path ?? `webarena/${shard.area}/${shard.task_id}`,
    owned_scope: ["webarena", shard.area, String(shard.task_id)],
    intent: task.intent,
    start_url: task.start_url,
    eval_types: task.eval?.eval_types ?? [],
    actions: fallbackActions(),
    fixture_risks: ["external site unavailable", "selector drift", "network timeout"]
  });
}

function fallbackActions() {
  if (Array.isArray(shard.offline_actions) && shard.offline_actions.length > 0) return shard.offline_actions;
  return [{ type: "goto", url: task.start_url }, { type: "wait", ms: 1000 }];
}

function normalizePlan(plan) {
  const actions = Array.isArray(plan.actions) && plan.actions.length > 0 ? plan.actions : shard.offline_actions ?? [{ type: "goto", url: task.start_url }];
  return {
    version: 1,
    namespace,
    shard_id: shardId,
    shard_path: String(plan.shard_path ?? shard.shard_path ?? `webarena/${shard.area}/${shard.task_id}`),
    owned_scope: Array.isArray(plan.owned_scope) ? plan.owned_scope.map(String) : ["webarena", shard.area, String(shard.task_id)],
    intent: String(plan.intent ?? task.intent ?? ""),
    start_url: String(plan.start_url ?? task.start_url ?? ""),
    eval_types: Array.isArray(plan.eval_types) ? plan.eval_types.map(String) : task.eval?.eval_types ?? [],
    actions,
    fixture_risks: Array.isArray(plan.fixture_risks) ? plan.fixture_risks.map(String) : []
  };
}

function schema() {
  return {
    version: 1,
    namespace: "string",
    shard_id: "string",
    shard_path: "webarena/site/task",
    owned_scope: ["webarena", "site", "task"],
    intent: "task intent",
    start_url: "https://example.test",
    eval_types: ["url_match"],
    actions: [
      { type: "goto", url: "https://example.test" },
      { type: "click", role: "link", name: "Example", withinRole: "navigation" },
      { type: "select", selector: "#sales_report_period_type", value: "year" },
      { type: "fill", selector: "input[name=q]", value: "text" },
      { type: "press", selector: "input[name=q]", key: "Enter" },
      { type: "stop", answer: "final answer" }
    ],
    fixture_risks: ["risk"]
  };
}

function loadTask(shard) {
  if (shard.config_path) {
    const task = JSON.parse(fs.readFileSync(path.resolve(repoRoot, String(shard.config_path)), "utf8"));
    if (shard.storage_state) task.storage_state = shard.storage_state;
    return task;
  }
  return shard;
}

function redactShardForWorker(shard, task) {
  return {
    id: shard.id,
    area: shard.area,
    task_id: shard.task_id,
    shard_path: shard.shard_path,
    require_login: shard.require_login,
    start_url: task.start_url,
    intent: task.intent,
    sites: task.sites ?? [],
    eval_types: task.eval?.eval_types ?? [],
    geolocation: task.geolocation ?? null,
    offline_actions: shard.offline_actions ?? []
  };
}

function parseStrictJson(text) {
  const trimmed = String(text).trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) return JSON.parse(trimmed);
  const objectText = firstJsonObject(trimmed);
  if (objectText) return JSON.parse(objectText);
  throw new Error("WebArena explorer did not return JSON");
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
    "# WebArena Shard Explorer Plan",
    "",
    `- Namespace: ${plan.namespace}`,
    `- Shard: ${plan.shard_id}`,
    `- Shard path: ${plan.shard_path}`,
    `- Intent: ${plan.intent}`,
    `- Eval types: ${plan.eval_types.join(", ")}`,
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
