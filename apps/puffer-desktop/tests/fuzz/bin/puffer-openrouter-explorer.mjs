#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  materializeParams,
  readJson,
  summarizeCaseCoverage,
  summarizeRun,
  writeJson
} from "../lib/fuzz-core.mjs";
import { createRng } from "../lib/seeded-rng.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const namespace = requireArg(args, "namespace");
const shardId = requireArg(args, "shard");
const seedId = requireArg(args, "seed");
const outPath = path.resolve(repoRoot, requireArg(args, "out"));
const model = args.model ?? process.env.PUFFER_OPENROUTER_MODEL ?? "inclusionai/ling-2.6-flash";
const baseUrl = (process.env.OPENROUTER_BASE_URL ?? "https://openrouter.ai/api/v1").replace(/\/+$/, "");
const apiKey = process.env.OPENROUTER_API_KEY;
const maxSteps = Number(args.steps ?? 8);
const runDir = path.resolve(fuzzRoot, ".runs", namespace);
const plannerGuidance = readOptional(path.join(runDir, "planner.md"));
const manifest = await readJson(path.join(fuzzRoot, "manifests", "puffer-ui.json"));
const seed = await readJson(path.join(fuzzRoot, "seeds", `${seedId}.json`));
const shard = await readJson(path.join(fuzzRoot, "shards", `${shardId}.json`));
const rng = createRng(`${namespace}:openrouter-explorer`);
const selectedActions = [];

if (!apiKey) throw new Error("OPENROUTER_API_KEY is required");

const allowedActions = buildAllowedActions(seed, shard);
if (allowedActions.length === 0) {
  throw new Error(`No allowed actions for shard ${shardId} and seed ${seedId}`);
}
const explorerResources = new Set();
for (const setup of seed.setup ?? []) {
  applyResourceTransitions(explorerResources, {
    ...setup,
    params: materializeParams(setup.params ?? {}, rng)
  });
}

const messages = [
  {
    role: "system",
    content: [
      "You are a cheap GUI explorer agent with strong tool-use.",
      "Do not plan globally. Do not edit files. Do not report bugs.",
      "Use tools to build one high-value GUI interaction sequence for the assigned shard.",
      "Prefer user-visible click/type/keyboard actions plus async races allowed by the shard.",
      "Never describe CLI commands as GUI steps.",
      "Stop once the sequence is likely to trigger a blocked interaction or stale-state bug."
    ].join(" ")
  },
  {
    role: "user",
    content: buildExplorerPrompt({ namespace, shard, seed, allowedActions, maxSteps, plannerGuidance })
  }
];

for (let round = 0; round < maxSteps + 4; round += 1) {
  const response = await openRouterChat(messages);
  const message = response.choices?.[0]?.message;
  if (!message) throw new Error("OpenRouter response did not include a message");
  messages.push(message);

  const toolCalls = message.tool_calls ?? [];
  if (toolCalls.length === 0) {
    break;
  }

  let finished = false;
  for (const call of toolCalls) {
    const name = call.function?.name;
    const parsedArgs = parseToolArguments(call.function?.arguments);
    if (name === "add_step") {
      const result = addStep(parsedArgs);
      messages.push(toolResult(call.id, result));
      continue;
    }
    if (name === "finish_case") {
      messages.push(toolResult(call.id, { ok: true, selectedSteps: selectedActions.length }));
      finished = true;
      break;
    }
    messages.push(toolResult(call.id, { ok: false, error: `Unknown tool ${name}` }));
  }
  if (finished || selectedActions.length >= maxSteps) break;
}

if (selectedActions.length === 0) {
  const fallback = allowedActions[0];
  selectedActions.push({ actionId: fallback.id, params: materializeParams(fallback.params ?? {}, rng), reason: "fallback first allowed action" });
}

const testCase = buildCase({ seed, shard, namespace, selectedActions, rng });
const run = {
  version: 1,
  manifestVersion: manifest.version,
  generatedAt: new Date().toISOString(),
  options: {
    mode: "openrouter-explorer",
    model,
    namespace,
    shard: shardId,
    seed: seedId,
    steps: maxSteps
  },
  cases: [testCase],
  summary: summarizeRun(manifest, [testCase])
};

await writeJson(outPath, run);
process.stdout.write(`OPENROUTER_EXPLORER_OK ${relative(outPath)}\n`);

async function openRouterChat(messages) {
  const response = await fetch(`${baseUrl}/chat/completions`, {
    method: "POST",
    headers: {
      "Authorization": `Bearer ${apiKey}`,
      "Content-Type": "application/json",
      "HTTP-Referer": "https://github.com/berabuddies/puffer",
      "X-Title": "Puffer UIUX Fuzz"
    },
    body: JSON.stringify({
      model,
      temperature: 0.4,
      max_tokens: 2048,
      messages,
      tools: explorerTools(allowedActions),
      tool_choice: "auto"
    })
  });
  const bodyText = await response.text();
  if (!response.ok) {
    throw new Error(`OpenRouter explorer request failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
  }
  return JSON.parse(bodyText);
}

function addStep(input) {
  if (selectedActions.length >= maxSteps) {
    return { ok: false, error: `max steps reached: ${maxSteps}` };
  }
  const action = allowedActions.find((item) => item.id === input.action);
  if (!action) {
    return { ok: false, error: `action is not allowed in this shard: ${input.action}` };
  }
  const missing = (action.requires ?? []).filter((tag) => !explorerResources.has(tag));
  if (missing.length > 0) {
    return {
      ok: false,
      error: `missing required state/resource for ${action.id}: ${missing.join(", ")}`,
      availableResources: [...explorerResources].sort()
    };
  }
  const blocked = (action.blocks ?? []).filter((tag) => explorerResources.has(tag));
  if (blocked.length > 0) {
    return {
      ok: false,
      error: `blocked by active state/resource for ${action.id}: ${blocked.join(", ")}`,
      availableResources: [...explorerResources].sort()
    };
  }
  const step = {
    ...action,
    params: coerceParams(action.params ?? {}, input.params ?? {}, rng)
  };
  applyResourceTransitions(explorerResources, step);
  selectedActions.push({
    actionId: action.id,
    params: step.params,
    reason: String(input.reason ?? "")
  });
  return {
    ok: true,
    accepted: action.id,
    selectedSteps: selectedActions.length,
    remainingSteps: Math.max(0, maxSteps - selectedActions.length),
    availableResources: [...explorerResources].sort()
  };
}

function buildCase({ seed, shard, namespace, selectedActions, rng }) {
  const steps = [];
  const resources = new Set();
  for (const setup of seed.setup ?? []) {
    const step = {
      ...setup,
      phase: "setup",
      params: materializeParams(setup.params ?? {}, rng)
    };
    steps.push(step);
    applyResourceTransitions(resources, step);
  }

  for (let index = 0; index < selectedActions.length; index += 1) {
    const action = allowedActions.find((item) => item.id === selectedActions[index].actionId);
    if (!action) continue;
    const step = {
      id: `${action.id}-${index + 1}`,
      action: action.id,
      kind: action.kind,
      target: action.target,
      phase: "fuzz",
      params: selectedActions[index].params,
      coverage: action.coverage ?? [],
      requires: action.requires ?? [],
      blocks: action.blocks ?? [],
      consumes: action.consumes ?? [],
      invalidates: action.invalidates ?? [],
      produces: action.produces ?? [],
      expectedDaemon: action.expectedDaemon ?? null,
      note: selectedActions[index].reason || "Selected by OpenRouter explorer"
    };
    steps.push(step);
    applyResourceTransitions(resources, step);
  }

  appendAutoSettleSteps(seed, resources, steps, rng);
  for (const invariant of shard.invariants ?? seed.invariants ?? []) {
    steps.push({
      id: `assert-${invariant}`,
      action: "assert",
      kind: "assertion",
      target: invariant,
      phase: "assert",
      assertions: [invariant],
      coverage: [`invariant:${invariant}`]
    });
  }

  return {
    caseId: `${seed.id}-explorer-0001`,
    seedId: seed.id,
    title: `${seed.title} (${shard.id} explorer)`,
    rngSeed: `${namespace}:openrouter-explorer`,
    focus: `${seed.focus} Shard: ${shard.title}.`,
    severityTarget: seed.severityTarget,
    explorer: {
      model,
      namespace,
      shard: shard.id,
      selectedActions
    },
    steps,
    coverage: [...summarizeCaseCoverage(steps)].sort()
  };
}

function buildAllowedActions(seed, shard) {
  const ownedCoverage = new Set(shard.ownedCoverage ?? []);
  const allowedAsync = new Set((shard.allowedAsyncEvents ?? []).map((item) => `async:${item}`));
  const byCoverage = (action) =>
    (action.coverage ?? []).some((tag) => ownedCoverage.has(tag) || allowedAsync.has(tag));
  const setupActions = new Set((seed.setup ?? []).map((step) => step.action));
  const selected = (seed.actions ?? []).filter((action) => {
    if (setupActions.has(action.id)) return false;
    return byCoverage(action);
  });
  return selected.map((action) => ({
    id: action.id,
    kind: action.kind,
    target: action.target,
    params: action.params ?? {},
    requires: action.requires ?? [],
    blocks: action.blocks ?? [],
    consumes: action.consumes ?? [],
    invalidates: action.invalidates ?? [],
    produces: action.produces ?? [],
    expectedDaemon: action.expectedDaemon ?? null,
    coverage: action.coverage ?? []
  }));
}

function explorerTools(allowedActions) {
  return [
    {
      type: "function",
      function: {
        name: "add_step",
        description: "Add one GUI, keyboard, daemon, or daemon-event step to this shard exploration sequence.",
        parameters: {
          type: "object",
          additionalProperties: false,
          properties: {
            action: {
              type: "string",
              enum: allowedActions.map((item) => item.id)
            },
            params: {
              type: "object",
              description: "Action parameters using the allowed values shown in the prompt.",
              additionalProperties: true
            },
            reason: {
              type: "string",
              description: "Why this step is useful for this shard."
            }
          },
          required: ["action"]
        }
      }
    },
    {
      type: "function",
      function: {
        name: "finish_case",
        description: "Finish the current exploration sequence.",
        parameters: {
          type: "object",
          additionalProperties: false,
          properties: {
            title: { type: "string" },
            notes: { type: "string" }
          },
          required: ["title"]
        }
      }
    }
  ];
}

function buildExplorerPrompt({ namespace, shard, seed, allowedActions, maxSteps, plannerGuidance }) {
  return [
    `Namespace: ${namespace}`,
    `Shard: ${shard.id} - ${shard.title}`,
    `Seed: ${seed.id} - ${seed.title}`,
    `Focus: ${seed.focus}`,
    `Severity target: ${seed.severityTarget}`,
    `Start node: ${shard.startNode}`,
    `Owned coverage: ${(shard.ownedCoverage ?? []).join(", ")}`,
    `Allowed async events: ${(shard.allowedAsyncEvents ?? []).join(", ")}`,
    `Max tool steps: ${maxSteps}`,
    "",
    "Main-agent planner guidance:",
    plannerGuidance || "(none)",
    "",
    "Use add_step repeatedly to build one high-value interaction sequence.",
    "Prefer sequences that combine user actions with allowed async races.",
    "Start with prerequisite GUI state. For example, type-composer must happen before send-prompt.",
    "If add_step returns missing required state/resource, choose a prerequisite action next.",
    "Reason strings must describe user GUI intent, not framework CLI commands.",
    "Do not call actions outside the enum exposed by the tool.",
    "",
    "Allowed actions and parameter schemas:",
    JSON.stringify(allowedActions.map((action) => ({
      id: action.id,
      kind: action.kind,
      target: action.target,
      params: action.params,
      requires: action.requires,
      blocks: action.blocks,
      coverage: action.coverage
    })), null, 2)
  ].join("\n");
}

function readOptional(filePath) {
  try {
    return fs.readFileSync(filePath, "utf8");
  } catch (error) {
    if (error && error.code === "ENOENT") return "";
    throw error;
  }
}

function coerceParams(schema, input, rng) {
  const result = materializeParams(schema, rng);
  for (const [key, value] of Object.entries(input ?? {})) {
    const spec = schema[key];
    if (!spec) continue;
    if (Array.isArray(spec.oneOf) && spec.oneOf.some((item) => item === value)) {
      result[key] = value;
      continue;
    }
    if (Array.isArray(spec.intRange) && Number.isInteger(value)) {
      const [min, max] = spec.intRange.map(Number);
      if (value >= min && value <= max) result[key] = value;
    }
  }
  return result;
}

function appendAutoSettleSteps(seed, resources, steps, rng) {
  const cleanupOrder = [
    ["resource:permission.pending", "answer-permission", { answer: "approve" }],
    ["resource:question.pending", "answer-question", { answer: "yes" }],
    ["resource:turn.canceling", "settle-canceled-turn", {}],
    ["resource:turn.running", "complete-turn", {}]
  ];
  for (const [resource, actionId, defaultParams] of cleanupOrder) {
    if (!resources.has(resource)) continue;
    const action = (seed.actions ?? []).find((item) => item.id === actionId);
    if (!action) continue;
    const step = {
      id: `settle-${action.id}-${steps.length + 1}`,
      action: action.id,
      kind: action.kind,
      target: action.target,
      phase: "settle",
      params: { ...defaultParams, ...materializeParams(action.params ?? {}, rng) },
      coverage: action.coverage ?? [],
      requires: action.requires ?? [],
      blocks: action.blocks ?? [],
      consumes: action.consumes ?? [],
      invalidates: action.invalidates ?? [],
      produces: action.produces ?? [],
      expectedDaemon: action.expectedDaemon ?? null,
      note: "Auto-added by OpenRouter explorer before invariant checks."
    };
    steps.push(step);
    applyResourceTransitions(resources, step);
  }
}

function applyResourceTransitions(resources, step) {
  for (const tag of step.consumes ?? []) resources.delete(tag);
  for (const tag of step.invalidates ?? []) resources.delete(tag);
  for (const tag of step.coverage ?? []) resources.add(tag);
  for (const tag of step.produces ?? []) resources.add(tag);
}

function parseToolArguments(raw) {
  if (!raw) return {};
  if (typeof raw === "object") return raw;
  try {
    return JSON.parse(raw);
  } catch {
    return {};
  }
}

function toolResult(toolCallId, payload) {
  return {
    role: "tool",
    tool_call_id: toolCallId,
    content: JSON.stringify(payload)
  };
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
