#!/usr/bin/env node
import { execFile, spawn } from "node:child_process";
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
const planPath = path.resolve(repoRoot, requireArg(args, "plan"));
const namespace = requireArg(args, "namespace");
const shardId = requireArg(args, "shard-id");
const outDir = path.resolve(repoRoot, String(args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace)));
const image = String(args.image ?? process.env.PUFFER_JUICE_SHOP_IMAGE ?? "bkimminich/juice-shop");
const port = Number(args.port ?? process.env.PUFFER_JUICE_SHOP_PORT ?? 13000);
const keepContainer = args["keep-container"] === "true" || process.env.PUFFER_JUICE_SHOP_KEEP_CONTAINER === "1";
const externalBaseUrl = String(args["base-url"] ?? "").replace(/\/+$/, "");
const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
const shard = (suite.shards ?? []).find((item) => item.id === shardId);
if (!shard) throw new Error(`Unknown Juice Shop shard: ${shardId}`);
const plan = JSON.parse(fs.readFileSync(planPath, "utf8"));
fs.mkdirSync(outDir, { recursive: true });

const containerName = safeName(`puffer-juice-${namespace}`);
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
let afterScore = null;
let newSolved = [];
try {
  if (!baseUrl) {
    baseUrl = `http://127.0.0.1:${port}`;
    await startContainer({ containerName, image, port });
    startedContainer = true;
  }
  await waitForReady(baseUrl);
  beforeScore = await readScore(baseUrl);
  records.push({
    type: "storage",
    value: JSON.stringify({ phase: "before", score: summarizeScore(beforeScore) }),
    metadata: { shardId, phase: "before-score" }
  });
  await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: 15000 });
  for (let index = 0; index < (plan.actions ?? []).length; index += 1) {
    const action = plan.actions[index];
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
      value: JSON.stringify({ shardId, step: index + 1, action, result: actionResult, error: actionError, before, after }),
      metadata: { shardId, step: index + 1, actionType: action.type }
    });
    if (actionError) {
      records.push({
        type: "console",
        value: JSON.stringify({ shardId, step: index + 1, actionType: action.type, actionError }),
        metadata: { shardId, step: index + 1, actionType: action.type }
      });
    }
  }
  for (const item of consoleErrors) {
    records.push({ type: "console", value: JSON.stringify({ shardId, text: item }), metadata: { shardId } });
  }
  for (const item of pageErrors) {
    records.push({ type: "console", value: JSON.stringify({ shardId, pageError: item }), metadata: { shardId } });
  }
  afterScore = await readScore(baseUrl);
  newSolved = diffSolved(beforeScore, afterScore);
  records.push({
    type: "storage",
    value: JSON.stringify({ phase: "after", score: summarizeScore(afterScore), newSolved }),
    metadata: { shardId, phase: "after-score" }
  });
  const expectedSolved = new Set(shard.target_challenges ?? []);
  const matchedTargets = newSolved.filter((item) => expectedSolved.has(item.key));
  if (newSolved.length > 0) {
    records.push({
      type: "predicate",
      value: JSON.stringify({
        shardId,
        predicate: "native-juice-shop-challenge-solved",
        targetChallenges: shard.target_challenges ?? [],
        matchedTargets,
        newSolved
      }),
      metadata: { shardId, predicate: "native-score-diff" }
    });
  } else {
    status = "no-new-score";
  }
} catch (caught) {
  status = "harness-error";
  error = String(caught?.stack ?? caught?.message ?? caught);
  records.push({ type: "console", value: JSON.stringify({ shardId, harnessError: error }), metadata: { shardId } });
} finally {
  await page.screenshot({ path: path.join(outDir, "final-page.png"), fullPage: true }).catch(() => undefined);
  await browser.close().catch(() => undefined);
  if (startedContainer && !keepContainer) await removeContainer(containerName);
}

const evidence = buildEvidenceIndex(records);
const verdict = buildVerdict({ status, error, shard, plan, evidenceIndex: evidence.evidence_index, beforeScore, afterScore, newSolved });
const gate = evaluateFindingAdmission(verdict, evidence.evidence_index);
const result = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  shardId,
  baseUrl,
  status,
  error,
  scoreBefore: summarizeScore(beforeScore),
  scoreAfter: summarizeScore(afterScore),
  newSolved,
  targetChallenges: shard.target_challenges ?? [],
  plan,
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
process.stdout.write(`JUICE_SHOP_RUNNER_OK ${relative(path.join(outDir, "result.json"))}\n`);
process.stdout.write(`Native score: ${result.scoreBefore.solved}/${result.scoreBefore.total} -> ${result.scoreAfter.solved}/${result.scoreAfter.total}; new=${newSolved.length}; gate=${gate.disposition}\n`);

async function startContainer({ containerName, image, port }) {
  await execFileAsync("docker", ["rm", "-f", containerName]).catch(() => undefined);
  await execFileAsync("docker", [
    "run",
    "--rm",
    "-d",
    "-p",
    `127.0.0.1:${port}:3000`,
    "--name",
    containerName,
    image
  ]);
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
  if (action.type === "fill") {
    await page.locator(String(action.selector)).first().fill(String(action.value ?? ""), { timeout: Number(action.timeout ?? 5000) });
    return { selector: action.selector };
  }
  if (action.type === "click") {
    await page.locator(String(action.selector)).first().click({ timeout: Number(action.timeout ?? 5000) });
    return { selector: action.selector };
  }
  if (action.type === "press") {
    await page.locator(String(action.selector ?? "body")).first().press(String(action.key), { timeout: Number(action.timeout ?? 5000) });
    return { selector: action.selector ?? "body", key: action.key };
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
  throw new Error(`Unsupported Juice Shop action: ${action.type}`);
}

async function compactDom(page) {
  return page.evaluate(() => ({
    title: document.title,
    url: location.href,
    text: document.body.innerText.replace(/\s+/g, " ").trim().slice(0, 1600),
    controls: Array.from(document.querySelectorAll("button,input,select,textarea,a")).slice(0, 80).map((element) => ({
      tag: element.tagName.toLowerCase(),
      id: element.id || "",
      text: (element.innerText || element.getAttribute("aria-label") || element.getAttribute("placeholder") || "").trim(),
      value: "value" in element ? String(element.value) : ""
    }))
  }));
}

function buildVerdict({ status, error, shard, plan, evidenceIndex, beforeScore, afterScore, newSolved }) {
  if (newSolved.length === 0) {
    return buildNoFindingVerdict({
      namespace,
      shard: shard.id,
      seed: plan.shard_id,
      replaySummary: { status, error, before: summarizeScore(beforeScore), after: summarizeScore(afterScore) }
    });
  }
  const predicate = evidenceIndex.find((entry) => entry.type === "predicate");
  const citations = evidenceIndex
    .filter((entry) => ["action", "network", "url", "storage"].includes(entry.type))
    .slice(0, 4)
    .map((entry) => ({ id: entry.id, type: entry.type, quote_hash: entry.sha256 }));
  return {
    version: 1,
    decision: "admit",
    title: `Juice Shop shard solved ${newSolved.length} native challenge(s)`,
    severity: "P2",
    area: `juice-shop/${shard.area}`,
    shard: shard.id,
    source_run: namespace,
    primary_cause: predicate ? { id: predicate.id, type: "predicate", quote_hash: predicate.sha256 } : null,
    citations,
    expected: `Shard should make progress on native Juice Shop target(s): ${(shard.target_challenges ?? []).join(", ")}`,
    actual: `New solved challenge(s): ${newSolved.map((item) => item.key).join(", ")}`,
    impact: "The benchmark-native score changed, so the shard produced a measurable Juice Shop result.",
    repro: (plan.actions ?? []).map((item) => JSON.stringify(item)),
    notes: `Before ${summarizeScore(beforeScore).solved}/${summarizeScore(beforeScore).total}; after ${summarizeScore(afterScore).solved}/${summarizeScore(afterScore).total}`
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

function formatMarkdown(result) {
  return [
    "# Juice Shop Fuzz Shard Result",
    "",
    `- Namespace: ${result.namespace}`,
    `- Shard: ${result.shardId}`,
    `- Status: ${result.status}`,
    `- Gate: ${result.gate.disposition}`,
    `- Score before: ${result.scoreBefore.solved}/${result.scoreBefore.total}`,
    `- Score after: ${result.scoreAfter.solved}/${result.scoreAfter.total}`,
    `- New solved: ${result.newSolved.map((item) => item.key).join(", ") || "none"}`,
    `- Screenshot: ${result.artifacts.screenshot}`,
    ""
  ].join("\n");
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
