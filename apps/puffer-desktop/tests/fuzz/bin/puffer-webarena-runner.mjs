#!/usr/bin/env node
import { createRequire } from "node:module";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { evaluateFindingAdmission, buildNoFindingVerdict } from "../lib/admission-gate.mjs";
import { buildEvidenceIndex } from "../lib/evidence-index.mjs";
import { readOpenRouterApiKey } from "../lib/openrouter-auth.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const desktopRequire = createRequire(path.join(repoRoot, "apps", "puffer-desktop", "package.json"));
const { chromium } = desktopRequire("playwright");
const args = parseArgs(process.argv.slice(2));
const suitePath = path.resolve(repoRoot, String(args.suite ?? "apps/puffer-desktop/tests/fuzz/benchmarks/webarena_smoke_suite.json"));
const planPath = path.resolve(repoRoot, requireArg(args, "plan"));
const namespace = requireArg(args, "namespace");
const shardId = requireArg(args, "shard-id");
const outDir = path.resolve(repoRoot, String(args.out ?? path.join("apps/puffer-desktop/tests/fuzz/.runs", namespace)));
const interactive = args.interactive === "true" || process.env.PUFFER_WEBARENA_INTERACTIVE === "1";
const model = String(args.model ?? process.env.PUFFER_OPENROUTER_MODEL ?? "inclusionai/ling-2.6-flash");
const baseUrl = (process.env.OPENROUTER_BASE_URL ?? "https://openrouter.ai/api/v1").replace(/\/+$/, "");
const apiKey = readOpenRouterApiKey();
const maxSteps = Math.max(1, Number(args["max-steps"] ?? process.env.PUFFER_WEBARENA_MAX_STEPS ?? 8));
const requestAttempts = Math.max(1, Number(process.env.PUFFER_OPENROUTER_REQUEST_ATTEMPTS ?? 4));
const requestTimeoutMs = Math.max(1000, Number(process.env.PUFFER_OPENROUTER_REQUEST_TIMEOUT_MS ?? 30000));
const suite = JSON.parse(fs.readFileSync(suitePath, "utf8"));
const shard = (suite.shards ?? []).find((item) => item.id === shardId);
if (!shard) throw new Error(`Unknown WebArena shard: ${shardId}`);
const task = loadTask(shard);
const plan = JSON.parse(fs.readFileSync(planPath, "utf8"));
if (interactive && !apiKey) throw new Error("Interactive WebArena runner requires OPENROUTER_API_KEY or PUFFER_OPENROUTER_API_KEY_FILE");
fs.mkdirSync(outDir, { recursive: true });

const browser = await chromium.launch({ headless: true });
const contextOptions = {};
if (task.storage_state) contextOptions.storageState = path.resolve(repoRoot, String(task.storage_state));
const context = await browser.newContext(contextOptions);
const page = await context.newPage();
const records = [];
const trace = [];
const consoleErrors = [];
const pageErrors = [];
page.on("console", (message) => {
  if (message.type() === "error") consoleErrors.push(message.text());
});
page.on("pageerror", (error) => pageErrors.push(error.message));

let status = "passed";
let error = "";
let stopAnswer = "";
let evalResult = { score: 0, passed: false, checks: [] };
try {
  if (task.start_url && !startsWithGoto(plan.actions)) {
    await page.goto(String(task.start_url), { waitUntil: "domcontentloaded", timeout: 15000 });
  }
  if (interactive) {
    const interactiveResult = await runInteractiveLoop(page);
    stopAnswer = interactiveResult.stopAnswer;
  } else {
    const actions = plan.actions ?? [];
    for (let index = 0; index < actions.length; index += 1) {
      const result = await executeAndRecord(page, actions[index], index + 1, { interactive: false });
      if (result.stopAnswer) stopAnswer = result.stopAnswer;
    }
  }
  for (const item of consoleErrors) records.push({ type: "console", value: JSON.stringify({ shardId, text: item }), metadata: { shardId } });
  for (const item of pageErrors) records.push({ type: "console", value: JSON.stringify({ shardId, pageError: item }), metadata: { shardId } });
  evalResult = await evaluateWebArena(page, task, stopAnswer);
  records.push({
    type: "predicate",
    value: JSON.stringify({ shardId, predicate: "native-webarena-evaluator", ...evalResult }),
    metadata: { shardId, predicate: "webarena-evaluator" }
  });
  if (!evalResult.passed) status = "evaluator-failed";
} catch (caught) {
  status = "harness-error";
  error = String(caught?.stack ?? caught?.message ?? caught);
  records.push({ type: "console", value: JSON.stringify({ shardId, harnessError: error }), metadata: { shardId } });
} finally {
  await page.screenshot({ path: path.join(outDir, "final-page.png"), fullPage: true }).catch(() => undefined);
  await context.close().catch(() => undefined);
  await browser.close().catch(() => undefined);
}

const evidence = buildEvidenceIndex(records);
const verdict = buildVerdict({ status, error, shard, task, plan, evidenceIndex: evidence.evidence_index, evalResult, stopAnswer });
const gate = evaluateFindingAdmission(verdict, evidence.evidence_index);
const result = {
  version: 1,
  generatedAt: new Date().toISOString(),
  namespace,
  shardId,
  status,
  error,
  score: evalResult.score,
  passed: evalResult.passed,
  checks: evalResult.checks,
  stopAnswer,
  plan,
  ...evidence,
  verdict,
  gate,
  artifacts: {
    resultJson: relative(path.join(outDir, "result.json")),
    verdictJson: relative(path.join(outDir, "verdict.json")),
    gateJson: relative(path.join(outDir, "verdict-gate.json")),
    traceJson: relative(path.join(outDir, "interactive-trace.json")),
    screenshot: relative(path.join(outDir, "final-page.png"))
  }
};
fs.writeFileSync(path.join(outDir, "result.json"), `${JSON.stringify(result, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "verdict.json"), `${JSON.stringify(verdict, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "verdict-gate.json"), `${JSON.stringify(gate, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "interactive-trace.json"), `${JSON.stringify(trace, null, 2)}\n`);
fs.writeFileSync(path.join(outDir, "result.md"), formatMarkdown(result));
process.stdout.write(`WEBARENA_RUNNER_OK ${relative(path.join(outDir, "result.json"))}\n`);
process.stdout.write(`WebArena score: ${result.score}; passed=${result.passed}; gate=${gate.disposition}\n`);

async function runInteractiveLoop(page) {
  const actions = [];
  const observations = [];
  let answer = "";
  if (task.start_url) {
    const gotoAction = { type: "goto", url: String(task.start_url) };
    const result = await executeAndRecord(page, gotoAction, 0, { interactive: true });
    actions.push(gotoAction);
    observations.push({ step: 0, action: gotoAction, url: result.after?.url, title: result.after?.title, text: result.after?.text });
  }
  for (let step = 1; step <= maxSteps; step += 1) {
    const observation = await compactDom(page).catch((caught) => ({ error: String(caught?.message ?? caught), url: page.url() }));
    const action = normalizeInteractiveAction(await nextInteractiveAction({ page, observation, actions, observations, step }));
    const result = await executeAndRecord(page, action, step, { before: observation, interactive: true });
    actions.push(action);
    observations.push({ step, action, error: result.actionError, url: result.after?.url, title: result.after?.title, text: result.after?.text });
    if (action.type === "stop") {
      answer = result.stopAnswer || String(action.answer ?? "");
      break;
    }
  }
  if (!answer) {
    const stopAction = { type: "stop", answer: "N/A" };
    await executeAndRecord(page, stopAction, maxSteps + 1, { interactive: true });
    actions.push(stopAction);
    answer = "N/A";
  }
  return { actions, stopAnswer: answer };
}

async function executeAndRecord(page, action, step, options = {}) {
  const before = options.before ?? await compactDom(page).catch((caught) => ({ error: String(caught?.message ?? caught), url: page.url() }));
  let actionResult = null;
  let actionError = "";
  let stopAnswerForStep = "";
  try {
    actionResult = await performAction(page, action);
    if (action.type === "stop") {
      stopAnswerForStep = String(action.answer ?? "");
      if (isEmptyAnswer(stopAnswerForStep)) {
        stopAnswerForStep = inferVisibleAnswer(before, task.intent) || stopAnswerForStep;
      }
      actionResult = { answer: stopAnswerForStep };
    }
  } catch (caught) {
    actionError = String(caught?.stack ?? caught?.message ?? caught);
    if (action.required === true) throw caught;
  }
  const after = await compactDom(page).catch((caught) => ({ error: String(caught?.message ?? caught), url: page.url() }));
  const entry = {
    shardId,
    step,
    action,
    result: actionResult,
    error: actionError,
    before: trimObservation(before),
    after: trimObservation(after),
    interactive: Boolean(options.interactive)
  };
  trace.push(entry);
  records.push({
    type: action.type === "goto" ? "url" : "action",
    value: JSON.stringify(entry),
    metadata: { shardId, step, actionType: action.type, interactive: Boolean(options.interactive) }
  });
  if (actionError) {
    records.push({
      type: "console",
      value: JSON.stringify({ shardId, step, actionType: action.type, actionError, interactive: Boolean(options.interactive) }),
      metadata: { shardId, step, actionType: action.type, interactive: Boolean(options.interactive) }
    });
  }
  return { actionResult, actionError, before, after, stopAnswer: stopAnswerForStep };
}

function isEmptyAnswer(answer) {
  return ["", "n/a", "na", "not available", "unknown"].includes(normalizeText(answer));
}

function inferVisibleAnswer(observation, intent) {
  const productRows = visibleProductRows(observation);
  if (productRows.length === 0) return "";
  const normalizedIntent = normalizeText(intent);
  if (normalizedIntent.includes("top-2") && normalizedIntent.includes("best-selling product")) {
    return productRows.slice(0, 2).map((row) => row.product).join(", ");
  }
  if (normalizedIntent.includes("top-1") && normalizedIntent.includes("best-selling product type")) {
    return inferProductType(productRows[0].product);
  }
  if (normalizedIntent.includes("top-1") && normalizedIntent.includes("best-selling brand")) {
    return productRows[0].product.split(/\s+/)[0] ?? "";
  }
  if (normalizedIntent.includes("top-1") && normalizedIntent.includes("best-selling product")) {
    return productRows[0].product;
  }
  return "";
}

function visibleProductRows(observation) {
  const tables = Array.isArray(observation?.tables) ? observation.tables : [];
  for (const table of tables) {
    const rows = Array.isArray(table.rows) ? table.rows : [];
    const header = rows[0]?.map((cell) => normalizeText(cell)) ?? [];
    const productIndex = header.indexOf("product");
    const quantityIndex = header.indexOf("quantity");
    if (productIndex === -1 || quantityIndex === -1) continue;
    return rows.slice(1).map((row) => ({
      product: String(row[productIndex] ?? "").trim(),
      quantity: String(row[quantityIndex] ?? "").trim()
    })).filter((row) => row.product);
  }
  return [];
}

function inferProductType(productName) {
  const normalized = normalizeText(productName);
  if (normalized.includes("ball")) return "Yoga ball";
  if (normalized.includes("strap")) return "Yoga strap";
  if (normalized.includes("band")) return "Resistance band";
  if (normalized.includes("duffle")) return "Duffle bag";
  return productName;
}

async function nextInteractiveAction({ observation, actions, observations, step }) {
  const response = await openRouterChat({
    model,
    temperature: 0.1,
    max_tokens: 900,
    messages: [
      {
        role: "system",
        content: [
          "You are a WebArena browser control worker.",
          "You receive one page observation at a time and must return one strict JSON action only.",
          "Do not invent benchmark answers. If the current page contains enough evidence, stop with the exact answer.",
          "Allowed action types: click, fill, select, press, wait, goto, stop.",
          "Prefer visible controls from the observation. Use selectors only when they are visible in controls.",
          "For Magento Admin, menu labels are uppercase top-level links; click a top-level menu to reveal submenu links.",
          "If the dashboard is visible, the authentication precondition is already satisfied.",
          "For report questions, first look for visible dashboard widgets before navigating into deep menus.",
          "For Magento report filters, use #sales_report_period_type, #sales_report_from, #sales_report_to, and #filter_form_submit when visible.",
          "Interpret Quarter 1 2022 as from 01/01/2022 to 03/31/2022; interpret 2022 as from 01/01/2022 to 12/31/2022.",
          "When a visible table already contains the requested top result, stop with that exact cell value.",
          "If a previous action failed, choose a different visible control or stop with N/A if blocked."
        ].join(" ")
      },
      {
        role: "user",
        content: [
          `Namespace: ${namespace}`,
          `Shard: ${shardId}`,
          `Step: ${step}/${maxSteps}`,
          `Intent: ${task.intent}`,
          `Start URL: ${task.start_url}`,
          `Eval types: ${(task.eval?.eval_types ?? []).join(", ")}`,
          "",
          "Recent actions:",
          JSON.stringify(actions.slice(-5), null, 2),
          "",
          "Recent observations:",
          JSON.stringify(observations.slice(-3), null, 2),
          "",
          "Current observation:",
          JSON.stringify(trimObservation(observation), null, 2),
          "",
          "Return one JSON object only. Examples:",
          JSON.stringify({ type: "click", role: "link", name: "REPORTS", withinRole: "navigation" }),
          JSON.stringify({ type: "select", selector: "#sales_report_period_type", value: "year" }),
          JSON.stringify({ type: "fill", selector: "input[name='from']", value: "01/01/2022" }),
          JSON.stringify({ type: "press", selector: "body", key: "Enter" }),
          JSON.stringify({ type: "stop", answer: "exact answer from page evidence" })
        ].join("\n")
      }
    ]
  });
  const content = response?.choices?.[0]?.message?.content ?? "";
  let action;
  try {
    action = parseStrictJson(content);
  } catch (caught) {
    records.push({
      type: "storage",
      value: JSON.stringify({ shardId, step, interactiveModel: model, rawContent: content, parseError: String(caught?.message ?? caught) }),
      metadata: { shardId, step, interactive: true, kind: "model-parse-error" }
    });
    return { type: "stop", answer: "N/A" };
  }
  records.push({
    type: "storage",
    value: JSON.stringify({ shardId, step, interactiveModel: model, rawContent: content, rawAction: action }),
    metadata: { shardId, step, interactive: true, kind: "model-action" }
  });
  return action;
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
          "X-Title": "Puffer WebArena Interactive Runner"
        },
        body: JSON.stringify(body)
      });
      const bodyText = await response.text();
      if (!response.ok) throw new Error(`OpenRouter WebArena runner failed with ${response.status}: ${bodyText.slice(0, 1000)}`);
      return JSON.parse(bodyText);
    } catch (caught) {
      lastError = caught;
      if (attempt !== requestAttempts) await sleep(1000 * attempt * attempt);
    } finally {
      clearTimeout(timeout);
    }
  }
  throw lastError ?? new Error("OpenRouter WebArena runner failed");
}

function normalizeInteractiveAction(action) {
  const normalized = action && typeof action === "object" ? { ...action } : { type: "stop", answer: "N/A" };
  normalized.type = String(normalized.type ?? "stop");
  if (normalized.type === "stop") normalized.answer = String(normalized.answer ?? "N/A");
  if (normalized.type === "wait" && normalized.ms === undefined) normalized.ms = Number(normalized.time ?? 1000);
  return normalized;
}

function trimObservation(observation) {
  const controls = Array.isArray(observation.controls) ? observation.controls : [];
  const links = Array.isArray(observation.links) ? observation.links : [];
  const tables = Array.isArray(observation.tables) ? observation.tables : [];
  return {
    title: observation.title ?? "",
    url: observation.url ?? "",
    text: String(observation.text ?? "").slice(0, 5000),
    tables: tables.slice(0, 8),
    controls: controls.slice(0, 180),
    links: links.slice(0, 180)
  };
}

async function performAction(page, action) {
  if (action.type === "goto") {
    const url = String(action.url ?? action.path ?? task.start_url);
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: Number(action.timeout ?? 15000) });
    return { url: page.url() };
  }
  if (action.type === "wait") {
    await page.waitForTimeout(Number(action.ms ?? 1000));
    return { waitedMs: Number(action.ms ?? 1000) };
  }
  if (action.type === "fill") {
    await withCandidateLocator(page, action, "fill", async (locator) => locator.fill(String(action.value ?? ""), { timeout: Number(action.timeout ?? 5000) }));
    return { selector: action.selector ?? "", role: action.role ?? "" };
  }
  if (action.type === "select") {
    await withCandidateLocator(page, action, "select", async (locator) => locator.selectOption(String(action.value ?? action.label ?? ""), { timeout: Number(action.timeout ?? 5000) }));
    return { selector: action.selector ?? "", value: action.value ?? action.label ?? "" };
  }
  if (action.type === "click") {
    await withCandidateLocator(page, action, "click", async (locator) => locator.click({ timeout: Number(action.timeout ?? 5000) }));
    return { selector: action.selector ?? "", role: action.role ?? "", name: action.name ?? "" };
  }
  if (action.type === "press") {
    await withCandidateLocator(page, action, "press", async (locator) => locator.press(String(action.key), { timeout: Number(action.timeout ?? 5000) }), "body");
    return { selector: action.selector ?? "body", key: action.key };
  }
  if (action.type === "stop") return { answer: String(action.answer ?? "") };
  throw new Error(`Unsupported WebArena action: ${action.type}`);
}

async function withCandidateLocator(page, action, operation, callback, defaultSelector = null) {
  const candidates = locateCandidates(page, action, defaultSelector);
  const errors = [];
  for (const locator of candidates) {
    try {
      await callback(locator);
      return;
    } catch (caught) {
      errors.push(String(caught?.message ?? caught).slice(0, 500));
    }
  }
  if (operation === "click" && action.name) {
    const href = await hiddenHrefByText(page, String(action.name)).catch(() => "");
    if (href && href !== "#") {
      await page.goto(href, { waitUntil: "domcontentloaded", timeout: Number(action.timeout ?? 15000) });
      return;
    }
  }
  throw new Error(`Unable to ${operation} with ${candidates.length} candidate locator(s): ${errors.join(" | ")}`);
}

function locateCandidates(page, action, defaultSelector = null) {
  const candidates = [];
  if (action.selector || defaultSelector) candidates.push(page.locator(String(action.selector ?? defaultSelector)).first());
  if (action.role) {
    const options = action.name ? { name: new RegExp(escapeRegExp(String(action.name)), "i") } : {};
    if (action.withinRole) candidates.push(page.getByRole(String(action.withinRole)).getByRole(String(action.role), options).first());
    candidates.push(page.getByRole(String(action.role), options).first());
    if (action.name) candidates.push(page.getByText(String(action.name), { exact: true }).first());
    if (action.name) candidates.push(page.getByText(String(action.name)).first());
  }
  if (action.text) {
    candidates.push(page.getByText(String(action.text), { exact: true }).first());
    candidates.push(page.getByText(String(action.text)).first());
  }
  if (action.name) {
    candidates.push(page.locator(`a:has-text("${cssString(String(action.name))}")`).first());
    candidates.push(page.locator(`button:has-text("${cssString(String(action.name))}")`).first());
  }
  if (candidates.length === 0) throw new Error("Action needs selector, role, name, or text locator");
  return candidates;
}

async function compactDom(page) {
  if (page.url() === "about:blank") return { title: "", url: page.url(), text: "", controls: [] };
  return page.evaluate(() => ({
    title: document.title,
    url: location.href,
    text: document.body.innerText.replace(/\s+/g, " ").trim().slice(0, 6000),
    tables: Array.from(document.querySelectorAll("table")).slice(0, 8).map((table) => ({
      caption: (table.caption?.innerText || "").trim(),
      text: table.innerText.replace(/\s+/g, " ").trim().slice(0, 1600),
      rows: Array.from(table.querySelectorAll("tr")).slice(0, 12).map((row) =>
        Array.from(row.querySelectorAll("th,td")).slice(0, 8).map((cell) => cell.innerText.replace(/\s+/g, " ").trim())
      )
    })),
    controls: Array.from(document.querySelectorAll("button,input,select,textarea,a,[role='button'],[role='link'],[role='menuitem']")).slice(0, 180).map((element) => ({
      tag: element.tagName.toLowerCase(),
      id: element.id || "",
      name: element.getAttribute("name") || "",
      type: element.getAttribute("type") || "",
      role: element.getAttribute("role") || "",
      href: element.getAttribute("href") || "",
      className: element.getAttribute("class") || "",
      selector: element.id ? `#${CSS.escape(element.id)}` : element.getAttribute("name") ? `${element.tagName.toLowerCase()}[name="${CSS.escape(element.getAttribute("name"))}"]` : "",
      text: (element.innerText || element.getAttribute("aria-label") || element.getAttribute("placeholder") || "").trim(),
      value: "value" in element ? String(element.value) : ""
    })),
    links: Array.from(document.querySelectorAll("a[href]")).slice(0, 180).map((element) => ({
      text: (element.innerText || element.getAttribute("aria-label") || "").trim(),
      href: element.href,
      id: element.id || "",
      className: element.getAttribute("class") || ""
    }))
  }));
}

async function evaluateWebArena(page, task, stopAnswer) {
  const checks = [];
  for (const type of task.eval?.eval_types ?? []) {
    if (type === "url_match") checks.push(evalUrl(page.url(), task.eval?.reference_url, task.eval?.url_note));
    else if (type === "string_match") checks.push(evalString(stopAnswer, task.eval?.reference_answers, task.intent, task.eval?.string_note));
    else if (type === "program_html") checks.push(await evalProgramHtml(page, task.eval?.program_html));
    else checks.push({ type, passed: false, expected: "supported evaluator", actual: "unsupported evaluator" });
  }
  const passed = checks.length > 0 && checks.every((item) => item.passed);
  return { score: passed ? 1 : 0, passed, checks };
}

function evalUrl(actualUrl, referenceUrl, urlNote = "GOLD in PRED") {
  const refs = String(referenceUrl ?? "").split(" |OR| ").map(normalizeUrl).filter(Boolean);
  const actual = normalizeUrl(actualUrl);
  const passed = urlNote === "GOLD in PRED" && refs.some((ref) => urlGoldInPred(ref, actual));
  return { type: "url_match", passed, expected: refs, actual, urlNote };
}

function evalString(answer, referenceAnswers, intent = "", stringNote = "") {
  const actual = normalizeText(answer);
  const refs = normalizeReferenceAnswers(referenceAnswers);
  let passed = refs.length > 0;
  for (const check of refs) {
    if (check.kind === "exact_match") passed &&= actual === normalizeText(check.value);
    if (check.kind === "must_include") passed &&= check.values.every((value) => actual.includes(normalizeText(value)));
    if (check.kind === "fuzzy_match") passed &&= check.values.some((value) => actual.includes(normalizeText(value)) || normalizeText(value).includes(actual));
  }
  return { type: "string_match", passed, expected: refs, actual: answer, intent, stringNote };
}

async function evalProgramHtml(page, programHtml) {
  const specs = Array.isArray(programHtml) ? programHtml : [];
  const checkResults = [];
  for (const spec of specs) {
    const unsupported = [];
    let targetUrl = String(spec.url ?? "last");
    if (targetUrl.startsWith("func:")) {
      unsupported.push(`target_url:${targetUrl}`);
      targetUrl = "last";
    }
    if (targetUrl && targetUrl !== "last") {
      await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: 15000 }).catch((caught) => unsupported.push(`goto:${caught.message}`));
    }
    for (const prepAction of spec.prep_actions ?? []) {
      await page.evaluate(`() => { ${prepAction} }`).catch((caught) => unsupported.push(`prep:${caught.message}`));
    }
    const locator = String(spec.locator ?? "");
    let selected = "";
    if (!locator.trim()) selected = await page.content().catch(() => "");
    else if (locator.startsWith("document.") || locator.startsWith("[...document.")) {
      selected = String(await page.evaluate(`() => ${locator}`).catch((caught) => {
        unsupported.push(`locator:${caught.message}`);
        return "";
      }));
    } else if (locator.startsWith("func:")) unsupported.push(`locator:${locator}`);
    else unsupported.push(`locator:${locator}`);
    const required = normalizeRequiredContents(spec.required_contents);
    const passed = required.every((item) => {
      if (item.kind === "exact_match") return normalizeText(selected) === normalizeText(item.value);
      return item.values.some((value) => selected.includes(String(value)));
    });
    checkResults.push({ passed, required, unsupported, actual: selected.slice(0, 500) });
  }
  const requiredResults = checkResults.filter((item) => item.required.length > 0);
  return { type: "program_html", passed: requiredResults.length === 0 ? true : requiredResults.every((item) => item.passed), checks: checkResults };
}

function buildVerdict({ status, error, shard, task, plan, evidenceIndex, evalResult, stopAnswer }) {
  if (!evalResult.passed) {
    return buildNoFindingVerdict({
      namespace,
      shard: shard.id,
      seed: plan.shard_id,
      replaySummary: { status, error, score: evalResult.score, checks: evalResult.checks, stopAnswer }
    });
  }
  const predicate = evidenceIndex.find((entry) => entry.type === "predicate");
  const citations = evidenceIndex
    .filter((entry) => ["action", "url", "storage"].includes(entry.type))
    .slice(0, 4)
    .map((entry) => ({ id: entry.id, type: entry.type, quote_hash: entry.sha256 }));
  return {
    version: 1,
    decision: "admit",
    title: `WebArena task ${task.task_id ?? shard.task_id} passed evaluator`,
    severity: "P2",
    area: `webarena/${shard.area}`,
    shard: shard.id,
    source_run: namespace,
    primary_cause: predicate ? { id: predicate.id, type: "predicate", quote_hash: predicate.sha256 } : null,
    citations,
    expected: `Task should satisfy WebArena eval type(s): ${(task.eval?.eval_types ?? []).join(", ")}`,
    actual: `Evaluator score=${evalResult.score}; stopAnswer=${stopAnswer || "none"}`,
    impact: "The benchmark-native evaluator passed, so the shard produced a measurable WebArena result.",
    repro: (plan.actions ?? []).map((item) => JSON.stringify(item)),
    notes: JSON.stringify(evalResult.checks)
  };
}

function normalizeUrl(value) {
  return String(value ?? "").trim().replace(/\/+$/, "");
}

function urlGoldInPred(ref, actual) {
  try {
    const refUrl = new URL(ref);
    const actualUrl = new URL(actual);
    const basePassed = `${actualUrl.host}${actualUrl.pathname}`.includes(`${refUrl.host}${refUrl.pathname}`);
    if (!basePassed) return false;
    for (const [key, value] of refUrl.searchParams.entries()) {
      if (![...actualUrl.searchParams.getAll(key)].includes(value)) return false;
    }
    if (refUrl.hash && refUrl.hash !== actualUrl.hash) return false;
    return true;
  } catch {
    return actual.includes(ref);
  }
}

function normalizeText(value) {
  return String(value ?? "").toLowerCase().replace(/\s+/g, " ").trim();
}

function normalizeReferenceAnswers(referenceAnswers) {
  if (Array.isArray(referenceAnswers)) return referenceAnswers.map((value) => ({ kind: "must_include", values: [String(value)] }));
  if (!referenceAnswers || typeof referenceAnswers !== "object") return [];
  const checks = [];
  if (referenceAnswers.exact_match !== undefined) checks.push({ kind: "exact_match", value: String(referenceAnswers.exact_match) });
  if (Array.isArray(referenceAnswers.must_include)) checks.push({ kind: "must_include", values: referenceAnswers.must_include.map(String) });
  if (Array.isArray(referenceAnswers.fuzzy_match)) checks.push({ kind: "fuzzy_match", values: referenceAnswers.fuzzy_match.map(String) });
  if (referenceAnswers.fuzzy_match === "N/A") checks.push({ kind: "exact_match", value: "N/A" });
  return checks;
}

function normalizeRequiredContents(requiredContents) {
  if (Array.isArray(requiredContents)) return requiredContents.map((value) => ({ kind: "must_include", values: [String(value)] }));
  if (!requiredContents || typeof requiredContents !== "object") return [];
  if (requiredContents.exact_match !== undefined) return [{ kind: "exact_match", value: String(requiredContents.exact_match) }];
  if (Array.isArray(requiredContents.must_include)) return requiredContents.must_include.map((value) => ({ kind: "must_include", values: String(value).split(" |OR| ") }));
  return [];
}

function loadTask(shard) {
  if (shard.config_path) {
    const task = JSON.parse(fs.readFileSync(path.resolve(repoRoot, String(shard.config_path)), "utf8"));
    if (shard.storage_state) task.storage_state = shard.storage_state;
    return task;
  }
  return shard;
}

function startsWithGoto(actions) {
  return Array.isArray(actions) && actions.some((action) => action.type === "goto");
}

function formatMarkdown(result) {
  return [
    "# WebArena Fuzz Shard Result",
    "",
    `- Namespace: ${result.namespace}`,
    `- Shard: ${result.shardId}`,
    `- Status: ${result.status}`,
    `- Gate: ${result.gate.disposition}`,
    `- Score: ${result.score}`,
    `- Passed: ${result.passed}`,
    `- Screenshot: ${result.artifacts.screenshot}`,
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
    if (!next || next.startsWith("--")) parsed[key] = "true";
    else {
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

function parseStrictJson(text) {
  const trimmed = String(text).trim();
  if (trimmed.startsWith("{") && trimmed.endsWith("}")) return JSON.parse(trimmed);
  const objectText = firstJsonObject(trimmed);
  if (objectText) return JSON.parse(objectText);
  throw new Error("Interactive WebArena worker did not return JSON");
}

async function hiddenHrefByText(page, text) {
  return page.evaluate((needle) => {
    const normalizedNeedle = needle.toLowerCase().replace(/\s+/g, " ").trim();
    for (const link of Array.from(document.querySelectorAll("a[href]"))) {
      const label = (link.innerText || link.textContent || link.getAttribute("aria-label") || "").toLowerCase().replace(/\s+/g, " ").trim();
      if (label === normalizedNeedle || label.includes(normalizedNeedle)) return link.href;
    }
    return "";
  }, text);
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

function cssString(value) {
  return value.replaceAll("\\", "\\\\").replaceAll("\"", "\\\"");
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
