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
const maxCases = args["max-cases"] === undefined ? null : Number(args["max-cases"]);
const caseIdFilter = String(args["case-id"] ?? "");
const assertGold = args["no-gold-assert"] !== "true";

const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
fs.mkdirSync(outDir, { recursive: true });
const results = [];
for (const testCase of selectedCases(suite.cases ?? [], { maxCases, caseId: caseIdFilter })) {
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
if (assertGold && (summary.buggyAdmitted < 1 || summary.fixedAdmitted !== 0)) process.exitCode = 2;

async function runCase(testCase) {
  const caseDir = path.join(outDir, testCase.id);
  fs.mkdirSync(caseDir, { recursive: true });
  const screenshotPath = path.join(caseDir, "final-page.png");
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
    await page.screenshot({ path: screenshotPath, fullPage: true }).catch(() => undefined);
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
    gate,
    artifacts: {
      resultJson: relative(path.join(caseDir, "result.json")),
      verdictJson: relative(path.join(caseDir, "verdict.json")),
      gateJson: relative(path.join(caseDir, "verdict-gate.json")),
      screenshot: fs.existsSync(screenshotPath) ? relative(screenshotPath) : ""
    }
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
    repro: buildReproSteps(testCase),
    notes: `Benchmark case ${testCase.id}`
  };
}

function resolveServer(server) {
  if (!server) return null;
  const cwd = String(server.cwd ?? "");
  const markers = ["benchmarks/smoke_apps/", "data/WebTestBench/"];
  const marker = markers.find((item) => cwd.includes(item));
  const markerIndex = marker ? cwd.indexOf(marker) : -1;
  return {
    ...server,
    cwd: markerIndex >= 0 && marker ? path.join(guiflowRoot, cwd.slice(markerIndex)) : cwd
  };
}

function startServer(server) {
  if (!server) return null;
  return spawn(server.command, server.args ?? [], {
    cwd: server.cwd,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"]
  });
}

async function stopServer(child) {
  let exited = false;
  try {
    process.kill(-child.pid, "SIGTERM");
  } catch {
    child.kill("SIGTERM");
  }
  await new Promise((resolve) => {
    const timer = setTimeout(resolve, 1000);
    child.once("exit", () => {
      exited = true;
      clearTimeout(timer);
      resolve();
    });
  });
  if (!exited) {
    try {
      process.kill(-child.pid, "SIGKILL");
    } catch {
      child.kill("SIGKILL");
    }
  }
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
  const repeat = Number(action.repeat ?? 1);
  for (let index = 0; index < repeat; index += 1) {
    await performOnce(page, action);
    if (action.afterEachMs) {
      await page.waitForTimeout(Number(action.afterEachMs));
    }
  }
}

async function performOnce(page, action) {
  if (action.type === "goto") {
    await page.goto(String(action.url), { waitUntil: "domcontentloaded", timeout: Number(action.timeout ?? 5000) });
    return;
  }
  if (action.type === "wait") {
    await page.waitForTimeout(Number(action.ms ?? 1000));
    return;
  }
  if (action.type === "fill") {
    await locatorForAction(page, action).fill(String(action.value), { timeout: Number(action.timeout ?? 5000) });
    return;
  }
  if (action.type === "click") {
    await locatorForAction(page, action).click({ timeout: Number(action.timeout ?? 5000) });
    return;
  }
  throw new Error(`Unsupported benchmark action: ${action.type}`);
}

async function checkExpectation(page, expectation) {
  if (expectation.type === "textIncludes") {
    const observed = await nthLocator(page.locator(expectation.selector), expectation).innerText({ timeout: 3000 });
    return { ...expectation, observed, passed: observed.includes(String(expectation.value)) };
  }
  if (expectation.type === "pageTextIncludes") {
    const observed = await page.locator("body").innerText({ timeout: 3000 });
    return { ...expectation, observed: truncate(observed), passed: observed.includes(String(expectation.value)) };
  }
  if (expectation.type === "pageTextOccurrenceAtMost") {
    const observed = await page.locator("body").innerText({ timeout: 3000 });
    const count = occurrenceCount(observed, String(expectation.value));
    return { ...expectation, observed: count, passed: count <= Number(expectation.max) };
  }
  if (expectation.type === "pageTextOccurrenceAtLeast") {
    const observed = await page.locator("body").innerText({ timeout: 3000 });
    const count = occurrenceCount(observed, String(expectation.value));
    return { ...expectation, observed: count, passed: count >= Number(expectation.min) };
  }
  if (expectation.type === "attributeIncludes" || expectation.type === "attributeNotIncludes") {
    const observed = await nthLocator(page.locator(expectation.selector), expectation)
      .getAttribute(String(expectation.attribute), { timeout: 3000 });
    const includes = String(observed ?? "").includes(String(expectation.value));
    return {
      ...expectation,
      observed,
      passed: expectation.type === "attributeIncludes" ? includes : !includes
    };
  }
  if (expectation.type === "disabled") {
    const observed = await nthLocator(page.locator(expectation.selector), expectation).isDisabled({ timeout: 3000 });
    return { ...expectation, observed, passed: observed === Boolean(expectation.value) };
  }
  if (expectation.type === "locatorCountAtLeast") {
    const observed = await page.locator(expectation.selector).count();
    return { ...expectation, observed, passed: observed >= Number(expectation.value) };
  }
  throw new Error(`Unsupported benchmark expectation: ${expectation.type}`);
}

function locatorForAction(page, action) {
  if (action.selector) return nthLocator(page.locator(action.selector), action);
  if (action.label) return page.getByLabel(String(action.label)).first();
  if (action.role) return page.getByRole(String(action.role), { name: action.name }).first();
  throw new Error(`Action is missing selector, label, or role: ${JSON.stringify(action)}`);
}

function nthLocator(locator, spec) {
  return spec.nth === undefined ? locator.first() : locator.nth(Number(spec.nth));
}

function occurrenceCount(text, needle) {
  if (!needle) return 0;
  return text.split(needle).length - 1;
}

function truncate(text, limit = 2000) {
  return String(text).replace(/\s+/g, " ").trim().slice(0, limit);
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
    lines.push(`- Screenshot: ${result.artifacts?.screenshot || "missing"}`);
    lines.push(`- Verdict: ${result.artifacts?.verdictJson || "missing"}`);
    lines.push(`- Citation gate: ${result.artifacts?.gateJson || "missing"}`);
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

function selectedCases(cases, { maxCases, caseId }) {
  const filtered = caseId ? cases.filter((item) => item.id === caseId) : cases;
  if (caseId && filtered.length === 0) throw new Error(`Unknown benchmark case: ${caseId}`);
  if (maxCases === null || Number.isNaN(maxCases) || maxCases <= 0) return filtered;
  return filtered.slice(0, maxCases);
}

function buildReproSteps(testCase) {
  const steps = [`goto ${testCase.url}`];
  for (const action of testCase.actions ?? []) {
    steps.push(`${action.type} ${action.selector ?? action.label ?? action.role ?? action.text ?? ""} ${action.value ?? ""}`.trim());
  }
  if ((testCase.actions ?? []).length === 0) {
    steps.push("evaluate benchmark expectations");
  }
  return steps;
}
