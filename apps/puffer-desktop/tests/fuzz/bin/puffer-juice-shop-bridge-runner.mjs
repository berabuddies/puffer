#!/usr/bin/env node
import { execFile } from "node:child_process";
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { promisify } from "node:util";
import { fileURLToPath } from "node:url";

import { evaluateFindingAdmission, buildNoFindingVerdict } from "../lib/admission-gate.mjs";
import { buildEvidenceIndex } from "../lib/evidence-index.mjs";

const execFileAsync = promisify(execFile);
const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const desktopRequire = createRequire(path.join(repoRoot, "apps", "puffer-desktop", "package.json"));
const { chromium } = desktopRequire("playwright");
const args = parseArgs(process.argv.slice(2));
const suitePath = path.resolve(repoRoot, String(args.suite ?? "apps/puffer-desktop/tests/fuzz/benchmarks/juice_shop_shards.json"));
const namespace = requireArg(args, "namespace");
const leftShardId = requireArg(args, "left-shard");
const rightShardId = requireArg(args, "right-shard");
const outDir = path.resolve(repoRoot, String(args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace)));
const image = String(args.image ?? process.env.PUFFER_JUICE_SHOP_IMAGE ?? "bkimminich/juice-shop");
const port = Number(args.port ?? process.env.PUFFER_JUICE_SHOP_PORT ?? 13100);
const keepContainer = args["keep-container"] === "true" || process.env.PUFFER_JUICE_SHOP_KEEP_CONTAINER === "1";
const externalBaseUrl = String(args["base-url"] ?? "").replace(/\/+$/, "");
const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
const leftShard = requireShard(suite, leftShardId);
const rightShard = requireShard(suite, rightShardId);
fs.mkdirSync(outDir, { recursive: true });

const containerName = safeName(`puffer-juice-bridge-${namespace}`);
let startedContainer = false;
let baseUrl = externalBaseUrl;
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage();
const records = [];
const consoleErrors = [];
const pageErrors = [];
page.on("console", (message) => {
  if (message.type() === "error") consoleErrors.push(message.text());
});
page.on("pageerror", (error) => pageErrors.push(error.message));

let status = "passed";
let error = "";
let beforeScore = null;
let middleScore = null;
let afterScore = null;
let leftSolved = [];
let rightSolved = [];
let matchedLeftTargets = [];
let matchedRightTargets = [];
try {
  if (!baseUrl) {
    baseUrl = `http://127.0.0.1:${port}`;
    await startContainer({ containerName, image, port });
    startedContainer = true;
  }
  await waitForReady(baseUrl);
  beforeScore = await readScore(baseUrl);
  records.push(scoreRecord("before", beforeScore));
  await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: 15000 });
  await runShardActions({ page, baseUrl, shard: leftShard, phase: "left", records });
  middleScore = await readScore(baseUrl);
  leftSolved = diffSolved(beforeScore, middleScore);
  matchedLeftTargets = matchTargets(leftShard, leftSolved);
  records.push(scoreRecord("after-left", middleScore, leftSolved));
  await runShardActions({ page, baseUrl, shard: rightShard, phase: "right", records });
  afterScore = await readScore(baseUrl);
  rightSolved = diffSolved(middleScore, afterScore);
  matchedRightTargets = matchTargets(rightShard, rightSolved);
  records.push(scoreRecord("after-right", afterScore, rightSolved));
  for (const item of consoleErrors) records.push({ type: "console", value: JSON.stringify({ text: item }), metadata: { bridgePhase: "console" } });
  for (const item of pageErrors) records.push({ type: "console", value: JSON.stringify({ pageError: item }), metadata: { bridgePhase: "pageerror" } });
  if (matchedLeftTargets.length === 0 && matchedRightTargets.length === 0) {
    status = leftSolved.length > 0 || rightSolved.length > 0 ? "off-target-score" : "no-new-score";
  }
  if (matchedLeftTargets.length > 0 || matchedRightTargets.length > 0) {
    records.push({
      type: "predicate",
      value: JSON.stringify({
        predicate: "juice-shop-bridge-target-solved",
        leftShard: leftShard.id,
        rightShard: rightShard.id,
        matchedLeftTargets,
        matchedRightTargets,
        leftSolved,
        rightSolved
      }),
      metadata: { predicate: "juice-shop-bridge-target-solved", leftShard: leftShard.id, rightShard: rightShard.id }
    });
  }
} catch (caught) {
  status = "harness-error";
  error = String(caught?.stack ?? caught?.message ?? caught);
  records.push({ type: "console", value: JSON.stringify({ harnessError: error }), metadata: { bridgePhase: "harness" } });
} finally {
  await page.screenshot({ path: path.join(outDir, "final-page.png"), fullPage: true }).catch(() => undefined);
  await browser.close().catch(() => undefined);
  if (startedContainer && !keepContainer) await removeContainer(containerName);
}

const evidence = buildEvidenceIndex(records);
const verdict = buildVerdict({ status, error, evidenceIndex: evidence.evidence_index });
const gate = evaluateFindingAdmission(verdict, evidence.evidence_index);
const result = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  benchmark: "juice-shop",
  executionMode: "bridge-left-right-single-container",
  leftShard: summarizeShard(leftShard),
  rightShard: summarizeShard(rightShard),
  baseUrl,
  status,
  error,
  scoreBefore: summarizeScore(beforeScore),
  scoreAfterLeft: summarizeScore(middleScore),
  scoreAfterRight: summarizeScore(afterScore),
  leftSolved,
  rightSolved,
  matchedLeftTargets,
  matchedRightTargets,
  ...evidence,
  verdict,
  gate,
  artifacts: {
    resultJson: relative(path.join(outDir, "result.json")),
    verdictJson: relative(path.join(outDir, "verdict.json")),
    gateJson: relative(path.join(outDir, "verdict-gate.json")),
    screenshot: relative(path.join(outDir, "final-page.png"))
  }
};
fs.writeFileSync(path.join(outDir, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "verdict.json"), `${JSON.stringify(verdict, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "verdict-gate.json"), `${JSON.stringify(gate, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "result.md"), formatMarkdown(result));
process.stdout.write(`JUICE_SHOP_BRIDGE_OK ${relative(path.join(outDir, "result.json"))}\n`);
process.stdout.write(`Bridge target matches: left=${matchedLeftTargets.length}, right=${matchedRightTargets.length}, gate=${gate.disposition}\n`);

async function runShardActions({ page, baseUrl, shard, phase, records }) {
  const actions = shard.offline_actions ?? [];
  for (let index = 0; index < actions.length; index += 1) {
    const action = actions[index];
    const before = await compactDom(page).catch((caught) => ({ error: String(caught?.message ?? caught) }));
    let actionResult = null;
    let actionError = "";
    try {
      actionResult = await performAction(page, baseUrl, action);
    } catch (caught) {
      actionError = String(caught?.stack ?? caught?.message ?? caught);
      if (action.required === true) throw caught;
    }
    const after = await compactDom(page).catch((caught) => ({ error: String(caught?.message ?? caught) }));
    records.push({
      type: action.type === "request" ? "network" : "action",
      value: JSON.stringify({ phase, shardId: shard.id, step: index + 1, action, result: actionResult, error: actionError, before, after }),
      metadata: { bridgePhase: phase, shardId: shard.id, step: index + 1, actionType: action.type }
    });
    if (actionError) records.push({ type: "console", value: JSON.stringify({ phase, shardId: shard.id, actionError }), metadata: { bridgePhase: phase, shardId: shard.id } });
  }
}

function buildVerdict({ status, error, evidenceIndex }) {
  const anyTarget = matchedLeftTargets.length > 0 || matchedRightTargets.length > 0;
  if (!anyTarget) {
    return buildNoFindingVerdict({
      namespace,
      shard: `${leftShard.id}+${rightShard.id}`,
      seed: "juice-shop-bridge",
      replaySummary: { status, error, leftSolved, rightSolved }
    });
  }
  const predicate = evidenceIndex.find((entry) => entry.type === "predicate");
  const citations = evidenceIndex
    .filter((entry) => ["action", "network", "storage"].includes(entry.type))
    .slice(0, 5)
    .map((entry) => ({ id: entry.id, type: entry.type, quote_hash: entry.sha256 }));
  return {
    version: 1,
    decision: "admit",
    title: "Juice Shop bridge sequence solved target native challenge(s)",
    severity: "P2",
    area: `juice-shop/bridge/${leftShard.area}+${rightShard.area}`,
    shard: `${leftShard.id}+${rightShard.id}`,
    source_run: namespace,
    primary_cause: predicate ? { id: predicate.id, type: "predicate", quote_hash: predicate.sha256 } : null,
    citations,
    expected: "A bridge run should preserve state across the left and right shard in one container/browser context.",
    actual: `Left target matches: ${matchedLeftTargets.map((item) => item.key).join(", ") || "none"}; right target matches: ${matchedRightTargets.map((item) => item.key).join(", ") || "none"}`,
    impact: "The bridge runner produced measurable benchmark-native evidence across two shard phases without resetting the target.",
    repro: [
      `Run left shard ${leftShard.id}: ${(leftShard.offline_actions ?? []).map((item) => JSON.stringify(item)).join(" ; ")}`,
      `Run right shard ${rightShard.id}: ${(rightShard.offline_actions ?? []).map((item) => JSON.stringify(item)).join(" ; ")}`
    ],
    notes: `Before ${summarizeScore(beforeScore).solved}/${summarizeScore(beforeScore).total}; after-left ${summarizeScore(middleScore).solved}/${summarizeScore(middleScore).total}; after-right ${summarizeScore(afterScore).solved}/${summarizeScore(afterScore).total}`
  };
}

async function startContainer({ containerName, image, port }) {
  await execFileAsync("docker", ["rm", "-f", containerName]).catch(() => undefined);
  await execFileAsync("docker", ["run", "--rm", "-d", "-p", `127.0.0.1:${port}:3000`, "--name", containerName, image]);
}

async function removeContainer(containerName) {
  await execFileAsync("docker", ["rm", "-f", containerName]).catch(() => undefined);
}

async function waitForReady(url) {
  const deadline = Date.now() + 60_000;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(`${url}/api/Challenges`);
      if (response.ok) return;
      lastError = new Error(`ready probe returned ${response.status}`);
    } catch (caught) {
      lastError = caught;
    }
    await sleep(1000);
  }
  throw lastError ?? new Error(`Juice Shop did not become ready: ${url}`);
}

async function readScore(url) {
  const response = await fetch(`${url}/api/Challenges`);
  if (!response.ok) throw new Error(`Score endpoint failed with ${response.status}`);
  const payload = await response.json();
  const challenges = Array.isArray(payload.data) ? payload.data : [];
  return challenges.map((item) => ({
    id: item.id,
    key: item.key,
    name: item.name,
    category: item.category,
    difficulty: item.difficulty,
    solved: Boolean(item.solved),
    disabledEnv: item.disabledEnv ?? null
  }));
}

async function performAction(page, baseUrl, action) {
  if (action.type === "goto") {
    const url = action.url ?? joinUrl(baseUrl, action.path ?? "/");
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: Number(action.timeout ?? 10000) });
    return { url: page.url() };
  }
  if (action.type === "wait") {
    await page.waitForTimeout(Number(action.ms ?? 1000));
    return { waitedMs: Number(action.ms ?? 1000) };
  }
  if (action.type === "request") {
    const response = await fetch(joinUrl(baseUrl, action.path ?? "/"), {
      method: String(action.method ?? "GET"),
      headers: {
        "Content-Type": "application/json",
        ...(action.headers && typeof action.headers === "object" ? action.headers : {})
      },
      body: action.json === undefined ? undefined : JSON.stringify(action.json)
    });
    const text = await response.text();
    return { status: response.status, body: text.slice(0, 500) };
  }
  if (action.type === "submitFeedback") {
    const captchaResponse = await fetch(joinUrl(baseUrl, "/rest/captcha"));
    if (!captchaResponse.ok) throw new Error(`Captcha request failed with ${captchaResponse.status}`);
    const captcha = await captchaResponse.json();
    const response = await fetch(joinUrl(baseUrl, "/api/Feedbacks"), {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        comment: String(action.comment ?? "puffer fuzz feedback"),
        rating: Number(action.rating ?? 0),
        captchaId: captcha.captchaId,
        captcha: String(captcha.answer)
      })
    });
    const text = await response.text();
    return { status: response.status, captchaId: captcha.captchaId, body: text.slice(0, 500) };
  }
  throw new Error(`Unsupported Juice Shop bridge action: ${action.type}`);
}

async function compactDom(page) {
  return page.evaluate(() => ({
    title: document.title,
    url: location.href,
    text: document.body.innerText.replace(/\s+/g, " ").trim().slice(0, 1200),
    controls: Array.from(document.querySelectorAll("button,input,select,textarea,a")).slice(0, 40).map((element) => ({
      tag: element.tagName.toLowerCase(),
      id: element.id || "",
      text: (element.innerText || element.getAttribute("aria-label") || element.getAttribute("placeholder") || "").trim(),
      value: "value" in element ? String(element.value) : ""
    }))
  }));
}

function scoreRecord(phase, score, solved = []) {
  return {
    type: "storage",
    value: JSON.stringify({ phase, score: summarizeScore(score), solved }),
    metadata: { bridgePhase: phase }
  };
}

function summarizeShard(shard) {
  return {
    id: shard.id,
    area: shard.area,
    targetChallenges: shard.target_challenges ?? []
  };
}

function summarizeScore(score) {
  const list = Array.isArray(score) ? score : [];
  return {
    total: list.length,
    solved: list.filter((item) => item.solved).length,
    solvedKeys: list.filter((item) => item.solved).map((item) => item.key)
  };
}

function diffSolved(before, after) {
  const beforeSolved = new Set((before ?? []).filter((item) => item.solved).map((item) => item.key));
  return (after ?? []).filter((item) => item.solved && !beforeSolved.has(item.key));
}

function matchTargets(shard, solved) {
  const targets = new Set(shard.target_challenges ?? []);
  return solved.filter((item) => targets.has(item.key));
}

function formatMarkdown(result) {
  return [
    "# Juice Shop Bridge Fuzz Result",
    "",
    `- Namespace: ${result.namespace}`,
    `- Execution mode: ${result.executionMode}`,
    `- Left shard: ${result.leftShard.id}`,
    `- Right shard: ${result.rightShard.id}`,
    `- Status: ${result.status}`,
    `- Gate: ${result.gate.disposition}`,
    `- Score before: ${result.scoreBefore.solved}/${result.scoreBefore.total}`,
    `- Score after left: ${result.scoreAfterLeft.solved}/${result.scoreAfterLeft.total}`,
    `- Score after right: ${result.scoreAfterRight.solved}/${result.scoreAfterRight.total}`,
    `- Left target matched: ${result.matchedLeftTargets.map((item) => item.key).join(", ") || "none"}`,
    `- Right target matched: ${result.matchedRightTargets.map((item) => item.key).join(", ") || "none"}`,
    `- Screenshot: ${result.artifacts.screenshot}`,
    ""
  ].join("\n");
}

function requireShard(suite, shardId) {
  const shard = (suite.shards ?? []).find((item) => item.id === shardId);
  if (!shard) throw new Error(`Unknown Juice Shop shard: ${shardId}`);
  return shard;
}

function joinUrl(baseUrl, pathValue) {
  const pathText = String(pathValue || "/");
  if (/^https?:\/\//.test(pathText)) return pathText;
  return `${baseUrl}${pathText.startsWith("/") ? pathText : `/${pathText}`}`;
}

function safeName(value) {
  return String(value).toLowerCase().replace(/[^a-z0-9_.-]+/g, "-").slice(0, 100);
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
