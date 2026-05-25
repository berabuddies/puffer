#!/usr/bin/env node
import { spawn } from "node:child_process";
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { evaluateFindingAdmission, buildNoFindingVerdict } from "../lib/admission-gate.mjs";
import { buildEvidenceIndex } from "../lib/evidence-index.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const desktopRequire = createRequire(path.join(repoRoot, "apps", "puffer-desktop", "package.json"));
const { chromium } = desktopRequire("playwright");
const args = parseArgs(process.argv.slice(2));
const guiflowRoot = path.resolve(String(args.root ?? path.join(repoRoot, "..", "guiflow-paper")));
const suitePath = path.resolve(String(args.suite ?? path.join(guiflowRoot, "benchmarks", "smoke_suite.json")));
const outDir = path.resolve(repoRoot, String(args.out ?? "apps/puffer-desktop/tests/fuzz/.runs/guiflow-smoke"));

const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
fs.mkdirSync(outDir, { recursive: true });
const results = [];
for (const testCase of suite.cases ?? []) {
  results.push(await runCase(testCase));
}
const summary = summarize(results);
const report = {
  version: 1,
  generatedAt: new Date().toISOString(),
  suite: suite.name,
  suitePath: relative(suitePath),
  guiflowRoot: relative(guiflowRoot),
  summary,
  results
};
fs.writeFileSync(path.join(outDir, "guiflow-smoke-report.json"), `${JSON.stringify(report, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "guiflow-smoke-report.md"), formatMarkdown(report));
process.stdout.write(`GUIFLOW_SMOKE_OK ${relative(path.join(outDir, "guiflow-smoke-report.json"))}\n`);
process.stdout.write(`Buggy admitted: ${summary.buggyAdmitted}, Fixed admitted: ${summary.fixedAdmitted}\n`);
if (summary.buggyAdmitted < 1 || summary.fixedAdmitted !== 0) process.exitCode = 2;

async function runCase(testCase) {
  const caseDir = path.join(outDir, testCase.id);
  fs.mkdirSync(caseDir, { recursive: true });
  const server = startServer(resolveServer(testCase.server));
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
  const expectationResults = [];
  try {
    await waitForUrl(page, testCase.url);
    records.push({
      type: "url",
      value: JSON.stringify({ caseId: testCase.id, url: page.url() }),
      metadata: { caseId: testCase.id }
    });
    for (let index = 0; index < (testCase.actions ?? []).length; index += 1) {
      const action = testCase.actions[index];
      const before = await compactDom(page);
      await perform(page, action);
      const after = await compactDom(page);
      records.push({
        type: "action",
        value: JSON.stringify({ caseId: testCase.id, step: index + 1, action, before, after }),
        metadata: { caseId: testCase.id, step: index + 1, actionType: action.type }
      });
    }
    for (const expectation of testCase.expectations ?? []) {
      const result = await checkExpectation(page, expectation);
      expectationResults.push(result);
      if (!result.passed) status = "failed";
    }
    for (const item of consoleErrors) {
      records.push({ type: "console", value: JSON.stringify({ caseId: testCase.id, text: item }), metadata: { caseId: testCase.id } });
    }
    for (const item of pageErrors) {
      records.push({ type: "console", value: JSON.stringify({ caseId: testCase.id, pageError: item }), metadata: { caseId: testCase.id } });
    }
    if (status === "failed") {
      records.push({
        type: "predicate",
        value: JSON.stringify({
          caseId: testCase.id,
          requirement: testCase.requirement,
          expectedBehavior: testCase.expected_behavior,
          expectationResults
        }),
        metadata: { caseId: testCase.id, predicate: "benchmark-expectation" }
      });
    }
  } catch (caught) {
    status = "harness-error";
    error = String(caught?.stack ?? caught?.message ?? caught);
    records.push({
      type: "console",
      value: JSON.stringify({ caseId: testCase.id, harnessError: error }),
      metadata: { caseId: testCase.id }
    });
  } finally {
    await browser.close().catch(() => undefined);
    if (server) await stopServer(server);
  }

  const evidence = buildEvidenceIndex(records);
  const verdict = buildBenchmarkVerdict(testCase, status, expectationResults, evidence.evidence_index, error);
  const gate = evaluateFindingAdmission(verdict, evidence.evidence_index);
  const result = {
    caseId: testCase.id,
    app: testCase.app,
    goldBug: Boolean(testCase.gold_bug),
    status,
    error,
    expectationResults,
    ...evidence,
    verdict,
    gate
  };
  fs.writeFileSync(path.join(caseDir, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
  fs.writeFileSync(path.join(caseDir, "verdict.json"), `${JSON.stringify(verdict, null, 2)}\n`);
  fs.writeFileSync(path.join(caseDir, "verdict-gate.json"), `${JSON.stringify(gate, null, 2)}\n`);
  return result;
}

function buildBenchmarkVerdict(testCase, status, expectationResults, evidenceIndex, error) {
  if (status !== "failed") {
    return buildNoFindingVerdict({
      namespace: "guiflow-smoke",
      shard: testCase.app,
      seed: testCase.id,
      replaySummary: { status, error }
    });
  }
  const predicate = evidenceIndex.find((entry) => entry.type === "predicate");
  const citations = evidenceIndex
    .filter((entry) => ["action", "url"].includes(entry.type))
    .slice(0, 3)
    .map((entry) => ({ id: entry.id, type: entry.type, quote_hash: entry.sha256 }));
  return {
    version: 1,
    decision: "admit",
    title: `${testCase.app} violates checkout expectation`,
    severity: "P1",
    area: "guiflow-smoke",
    shard: testCase.app,
    source_run: "guiflow-smoke",
    primary_cause: predicate ? { id: predicate.id, type: "predicate", quote_hash: predicate.sha256 } : null,
    citations,
    expected: testCase.expected_behavior ?? testCase.requirement ?? "",
    actual: expectationResults.map((item) => `${item.selector}: observed ${JSON.stringify(item.observed)}`).join("; "),
    impact: "The benchmark user flow completes but the visible business result is stale or incorrect.",
    repro: (testCase.actions ?? []).map((action) => `${action.type} ${action.selector ?? action.role ?? action.text ?? ""} ${action.value ?? ""}`.trim()),
    notes: `Benchmark case ${testCase.id}`
  };
}

function resolveServer(server) {
  if (!server) return null;
  const cwd = String(server.cwd ?? "");
  const marker = "benchmarks/smoke_apps/";
  const markerIndex = cwd.indexOf(marker);
  return {
    ...server,
    cwd: markerIndex >= 0 ? path.join(guiflowRoot, cwd.slice(markerIndex)) : cwd
  };
}

function startServer(server) {
  if (!server) return null;
  return spawn(server.command, server.args ?? [], {
    cwd: server.cwd,
    stdio: ["ignore", "pipe", "pipe"]
  });
}

async function stopServer(child) {
  child.kill("SIGTERM");
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, 1000);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function waitForUrl(page, url, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let lastError = null;
  while (Date.now() < deadline) {
    try {
      await page.goto(url, { waitUntil: "domcontentloaded", timeout: 3000 });
      return;
    } catch (error) {
      lastError = error;
      await new Promise((resolve) => setTimeout(resolve, 250));
    }
  }
  throw lastError ?? new Error(`Timed out waiting for ${url}`);
}

async function compactDom(page) {
  return page.evaluate(() => ({
    title: document.title,
    url: location.href,
    text: document.body.innerText.replace(/\s+/g, " ").trim().slice(0, 2000),
    controls: Array.from(document.querySelectorAll("button,input,select,textarea,a")).map((element) => ({
      tag: element.tagName.toLowerCase(),
      id: element.id || "",
      text: (element.innerText || element.getAttribute("aria-label") || element.getAttribute("placeholder") || "").trim(),
      value: "value" in element ? String(element.value) : ""
    }))
  }));
}

async function perform(page, action) {
  if (action.type === "fill") {
    await page.locator(action.selector).first().fill(String(action.value), { timeout: 5000 });
    return;
  }
  if (action.type === "click") {
    await page.locator(action.selector).first().click({ timeout: 5000 });
    return;
  }
  throw new Error(`Unsupported smoke action: ${action.type}`);
}

async function checkExpectation(page, expectation) {
  if (expectation.type !== "textIncludes") throw new Error(`Unsupported smoke expectation: ${expectation.type}`);
  const observed = await page.locator(expectation.selector).first().innerText({ timeout: 3000 });
  return {
    ...expectation,
    observed,
    passed: observed.includes(String(expectation.value))
  };
}

function summarize(results) {
  const admitted = results.filter((item) => item.gate?.disposition === "admitted");
  return {
    total: results.length,
    admitted: admitted.length,
    candidates: results.filter((item) => item.gate?.disposition === "candidate").length,
    dismissed: results.filter((item) => item.gate?.disposition === "dismissed").length,
    buggyAdmitted: admitted.filter((item) => item.goldBug).length,
    fixedAdmitted: admitted.filter((item) => !item.goldBug).length
  };
}

function formatMarkdown(report) {
  const lines = [
    "# GUIFlow Smoke Benchmark Through Puffer Fuzz Gate",
    "",
    `Generated: ${report.generatedAt}`,
    `Suite: ${report.suite}`,
    "",
    "## Summary",
    "",
    `- Total cases: ${report.summary.total}`,
    `- Admitted: ${report.summary.admitted}`,
    `- Candidates: ${report.summary.candidates}`,
    `- Dismissed: ${report.summary.dismissed}`,
    `- Buggy admitted: ${report.summary.buggyAdmitted}`,
    `- Fixed admitted: ${report.summary.fixedAdmitted}`,
    "",
    "## Cases",
    ""
  ];
  for (const result of report.results) {
    lines.push(`### ${result.caseId}`);
    lines.push("");
    lines.push(`- App: ${result.app}`);
    lines.push(`- Gold bug: ${result.goldBug ? "yes" : "no"}`);
    lines.push(`- Status: ${result.status}`);
    lines.push(`- Gate disposition: ${result.gate.disposition}`);
    lines.push(`- Evidence entries: ${result.evidence_index.length}`);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
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

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
