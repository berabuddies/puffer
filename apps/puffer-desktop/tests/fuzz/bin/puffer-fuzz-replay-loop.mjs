#!/usr/bin/env node
import { lstat, mkdir, readFile, symlink, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const desktopRoot = path.resolve(fuzzRoot, "..", "..");
const fuzzCli = path.join(fuzzRoot, "bin", "puffer-fuzz.mjs");
const defaultSeeds = [
  "chat-turn-race",
  "workspace-session-race",
  "provider-auth-model-race",
  "modal-focus-race",
  "browser-tab-race",
  "files-terminal-race",
  "settings-mcp-permission-race",
  "pipelines-draft-race"
];

const seedDefaults = {
  "chat-turn-race": { iterations: 12, steps: 20 },
  "workspace-session-race": { iterations: 12, steps: 18 },
  "provider-auth-model-race": { iterations: 12, steps: 18 },
  "modal-focus-race": { iterations: 12, steps: 3 },
  "browser-tab-race": { iterations: 10, steps: 16 },
  "files-terminal-race": { iterations: 8, steps: 16 },
  "settings-mcp-permission-race": { iterations: 8, steps: 14 },
  "pipelines-draft-race": { iterations: 8, steps: 14 }
};

function parseArgs(argv) {
  const args = { _: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith("--")) {
      args._.push(item);
      continue;
    }
    const key = item.slice(2);
    const next = argv[index + 1];
    if (!next || next.startsWith("--")) {
      args[key] = true;
    } else {
      args[key] = next;
      index += 1;
    }
  }
  return args;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const seeds = String(args.seeds ?? process.env.PUFFER_REPLAY_SEEDS ?? defaultSeeds.join(","))
    .split(",")
    .map((item) => item.trim())
    .filter(Boolean);
  const topLimit = Number(args.limit ?? process.env.PUFFER_REPLAY_LIMIT ?? 1);
  const attempts = Number(args.attempts ?? process.env.PUFFER_REPLAY_ATTEMPTS ?? 1);
  const timeoutSeconds = Number(args.timeout ?? process.env.PUFFER_REPLAY_TIMEOUT_SECONDS ?? 120);
  const playwrightTimeoutMs = Math.max(
    10_000,
    Number(args["playwright-timeout-ms"] ?? Math.max(15, timeoutSeconds - 20) * 1000)
  );
  const shellTimeoutSeconds = Math.max(timeoutSeconds, Math.ceil(playwrightTimeoutMs / 1000) + 15);
  const rngNamespace = String(args["rng-seed"] ?? process.env.PUFFER_REPLAY_RNG_SEED ?? "bounded-replay");
  const namespace = sanitizeNamespace(String(args.namespace ?? process.env.PUFFER_REPLAY_NAMESPACE ?? `${rngNamespace}-${Date.now()}`));
  const port = Number(args.port ?? process.env.PUFFER_REPLAY_PORT ?? (15_000 + (hashString(namespace) % 1_000)));
  const tmpDir = path.resolve(String(args["tmp-dir"] ?? path.join(fuzzRoot, ".runs", namespace)));
  const out = path.resolve(String(args.out ?? path.join(tmpDir, "bounded-replay-report.md")));
  const jsonOut = path.resolve(String(args["json-out"] ?? path.join(tmpDir, "bounded-replay-report.json")));
  const ledgerPath = path.resolve(String(args.ledger ?? path.join(fuzzRoot, "coverage-ledger.json")));
  const specDir = path.join(tmpDir, "tests");
  const logDir = path.join(tmpDir, "logs");
  const playwrightOutputDir = path.join(tmpDir, "playwright-output");
  const reuseExistingServer = args["no-reuse-server"] ? false : true;
  const ledger = await readJsonIfExists(ledgerPath);
  const knownBugSignatures = ledger.knownBugSignatures ?? [];

  await mkdir(specDir, { recursive: true });
  await mkdir(logDir, { recursive: true });
  await mkdir(playwrightOutputDir, { recursive: true });
  await ensureNodeModulesLink(specDir, path.join(desktopRoot, "node_modules"));
  const playwrightConfigPath = path.join(tmpDir, "playwright.config.mjs");
  await writePlaywrightConfig(playwrightConfigPath, {
    desktopRoot,
    specDir,
    port,
    reuseExistingServer
  });

  const startedAt = new Date().toISOString();
  const results = [];
  for (const seed of seeds) {
    const defaults = seedDefaults[seed] ?? { iterations: 8, steps: 12 };
    const runPath = path.join(tmpDir, `${seed}.json`);
    const reportPath = path.join(tmpDir, `${seed}.md`);
    const topPath = path.join(tmpDir, `${seed}-top.json`);
    const topReportPath = path.join(tmpDir, `${seed}-top.md`);
    const rngSeed = `${rngNamespace}-${seed}`;

    await runCommand("node", [
      fuzzCli,
      "run",
      "--seed",
      seed,
      "--iterations",
      String(defaults.iterations),
      "--steps",
      String(defaults.steps),
      "--rng-seed",
      rngSeed,
      "--out",
      runPath
    ], { cwd: repoRoot, timeoutSeconds: 60 });

    await runCommand("node", [
      fuzzCli,
      "report",
      "--input",
      runPath,
      "--out",
      reportPath
    ], { cwd: repoRoot, timeoutSeconds: 60, quiet: true });

    await runCommand("node", [
      fuzzCli,
      "top-cases",
      "--input",
      runPath,
      "--limit",
      String(topLimit),
      "--out",
      topPath,
      "--report-out",
      topReportPath
    ], { cwd: repoRoot, timeoutSeconds: 60, quiet: true });

    const top = JSON.parse(await readFile(topPath, "utf8"));
    for (const item of top.cases ?? []) {
      const specName = `${item.caseId}.bounded.tmp.spec.ts`;
      const specPath = path.join(specDir, specName);
      const replay = {
        seed,
        caseId: item.caseId,
        score: item.score,
        diversityKey: item.diversityKey,
        coverage: item.coverage,
        steps: item.steps?.map((step) => step.action) ?? [],
        specPath,
        attempts: []
      };

      await runCommand("node", [
        fuzzCli,
        "replay",
        "--input",
        runPath,
        "--case-id",
        item.caseId,
        "--out",
        specPath
      ], { cwd: repoRoot, timeoutSeconds: 60, quiet: true });

      for (let attempt = 1; attempt <= attempts; attempt += 1) {
        const logPath = path.join(logDir, `${item.caseId}-attempt-${attempt}.log`);
        const result = await runCommand("timeout", [
          `${shellTimeoutSeconds}s`,
          "npx",
          "playwright",
          "test",
          specPath,
          "--config",
          playwrightConfigPath,
          "--workers=1",
          "--reporter=list",
          "--timeout",
          String(playwrightTimeoutMs),
          `--output=${path.join(playwrightOutputDir, `${item.caseId}-attempt-${attempt}`)}`
        ], {
          cwd: desktopRoot,
          timeoutSeconds: shellTimeoutSeconds + 20,
          env: { ...process.env, CODEX_CI: reuseExistingServer ? "" : "1" },
          allowFailure: true
        });
        await writeFile(logPath, result.output);
        replay.attempts.push({
          attempt,
          status: result.exitCode === 0 ? "passed" : result.exitCode === 124 ? "timeout" : "failed",
          exitCode: result.exitCode,
          logPath,
          excerpt: excerptFailure(result.output),
          failureSignature: failureSignature(result.output)
        });
      }
      Object.assign(replay, classifyReplay(replay));
      replay.knownDuplicate = knownBugMatch(replay.failureSignature ?? "", knownBugSignatures);
      results.push(replay);
    }
  }

  const finishedAt = new Date().toISOString();
  const summary = summarize(results);
  const findings = collectFindings(results, knownBugSignatures);
  const payload = {
    version: 1,
    startedAt,
    finishedAt,
    seeds,
    topLimit,
    attempts,
    timeoutSeconds,
    playwrightTimeoutMs,
    shellTimeoutSeconds,
    port,
    reuseExistingServer,
    namespace,
    artifactDir: tmpDir,
    playwrightConfigPath,
    ledgerPath,
    knownBugSignatures,
    summary,
    findings,
    results
  };
  await writeFile(jsonOut, `${JSON.stringify(payload, null, 2)}\n`);
  await writeFile(out, formatMarkdown(payload));
  process.stdout.write(`Report: ${out}\nJSON: ${jsonOut}\n`);
  process.stdout.write(`Passed: ${summary.passed}, Stable failed: ${summary.stableFailed}, Flaky: ${summary.flaky}, Timeout: ${summary.timeout}, Actionable failures: ${summary.actionableFailures}\n`);
  if ((summary.stableFailed > 0 || summary.flaky > 0 || summary.timeout > 0) && args["fail-on-finding"]) process.exitCode = 2;
  if (summary.actionableFailures > 0 && args["fail-on-new-finding"]) process.exitCode = 2;
}

async function readJsonIfExists(filePath) {
  try {
    return JSON.parse(await readFile(filePath, "utf8"));
  } catch (error) {
    if (error && error.code === "ENOENT") return {};
    throw error;
  }
}

function runCommand(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: ["ignore", "pipe", "pipe"]
    });
    let output = "";
    let settled = false;
    const timer = setTimeout(() => {
      if (settled) return;
      child.kill("SIGTERM");
      setTimeout(() => child.kill("SIGKILL"), 2_000).unref();
    }, Number(options.timeoutSeconds ?? 120) * 1000);
    child.stdout.on("data", (chunk) => {
      output += chunk.toString();
      if (!options.quiet) process.stdout.write(chunk);
    });
    child.stderr.on("data", (chunk) => {
      output += chunk.toString();
      if (!options.quiet) process.stderr.write(chunk);
    });
    child.on("error", (error) => {
      clearTimeout(timer);
      settled = true;
      reject(error);
    });
    child.on("close", (exitCode) => {
      clearTimeout(timer);
      settled = true;
      const result = { exitCode: exitCode ?? 1, output };
      if (result.exitCode !== 0 && !options.allowFailure) {
        const error = new Error(`${command} ${args.join(" ")} exited ${result.exitCode}`);
        error.result = result;
        reject(error);
        return;
      }
      resolve(result);
    });
  });
}

function excerptFailure(output) {
  const cleaned = output.replace(/\x1b\[[0-9;]*[A-Za-z]/g, "");
  const lines = cleaned.split(/\r?\n/);
  const interesting = lines.filter((line) => /Error|Timeout|expect|failed|passed|\u2718|\u2713|locator|Timed out|Test timeout|TimeoutError/i.test(line));
  return interesting.slice(-20).join("\n").slice(0, 4000);
}

function summarize(results) {
  let passed = 0;
  let stableFailed = 0;
  let flaky = 0;
  let timeout = 0;
  let productCandidateFindings = 0;
  let knownDuplicateFindings = 0;
  let newCandidateFindings = 0;
  let knownDuplicateFailures = 0;
  let actionableFailures = 0;
  const byClassification = {};
  for (const item of results) {
    const failed = item.status !== "passed";
    if (item.status === "passed") passed += 1;
    else if (item.status === "flaky") flaky += 1;
    else if (item.status === "timeout" || item.status === "unstable-timeout") timeout += 1;
    else stableFailed += 1;
    const classification = item.classification ?? "unknown";
    byClassification[classification] = (byClassification[classification] ?? 0) + 1;
    if (failed && item.knownDuplicate) knownDuplicateFailures += 1;
    if (failed && !item.knownDuplicate) actionableFailures += 1;
    if (failed && classification.startsWith("product-candidate:")) {
      productCandidateFindings += 1;
      if (item.knownDuplicate) knownDuplicateFindings += 1;
      else newCandidateFindings += 1;
    }
  }
  return {
    total: results.length,
    passed,
    stableFailed,
    flaky,
    timeout,
    productCandidateFindings,
    newCandidateFindings,
    knownDuplicateFindings,
    knownDuplicateFailures,
    actionableFailures,
    byClassification
  };
}

function collectFindings(results, knownBugSignatures = []) {
  const findings = [];
  for (const item of results) {
    const classification = item.classification ?? "unknown";
    if (item.status === "passed" || !classification.startsWith("product-candidate:")) continue;
    const last = item.attempts.at(-1) ?? { logPath: "", excerpt: "", exitCode: null };
    findings.push({
      id: `${item.caseId}:${classification}`,
      title: findingTitle(classification),
      classification,
      status: item.status,
      seed: item.seed,
      caseId: item.caseId,
      replayScore: item.score,
      diversityKey: item.diversityKey ?? "",
      failureSignature: item.failureSignature ?? last.failureSignature ?? "",
      knownDuplicate: knownBugMatch(item.failureSignature ?? last.failureSignature ?? "", knownBugSignatures),
      specPath: relativeRepoPath(item.specPath),
      logPath: last.logPath,
      attempts: item.attempts.map((attempt) => ({
        attempt: attempt.attempt,
        status: attempt.status,
        exitCode: attempt.exitCode,
        logPath: attempt.logPath
      })),
      coverage: item.coverage,
      steps: item.steps,
      excerpt: last.excerpt ?? ""
    });
  }
  return findings;
}

function knownBugMatch(signature, knownBugSignatures = []) {
  if (!signature) return false;
  return knownBugSignatures.some((known) => {
    if (!known) return false;
    return signature === known || signature.includes(known) || known.includes(signature);
  });
}

function findingTitle(classification) {
  const titles = {
    "product-candidate:connection-banner-blocks-navigation": "Connection banner blocks primary navigation recovery",
    "product-candidate:draft-recovery": "Draft or typed browser state is not preserved after failure",
    "product-candidate:duplicate-intent": "A single user intent can submit duplicate backend requests",
    "product-candidate:modal-focus": "Modal focus management can trap or lose keyboard focus",
    "product-candidate:stale-browser-tab-state": "Stale browser tab state can overwrite the active address",
    "product-candidate:unclassified": "Unclassified product candidate"
  };
  return titles[classification] ?? classification.replace(/^product-candidate:/, "").replaceAll("-", " ");
}

function formatMarkdown(payload) {
  const lines = [
    "# Puffer Bounded UI/UX Replay Report",
    "",
    `Started: ${payload.startedAt}`,
    `Finished: ${payload.finishedAt}`,
    `Seeds: ${payload.seeds.join(", ")}`,
    `Top cases per seed: ${payload.topLimit}`,
    `Attempts per case: ${payload.attempts}`,
    `Timeout per attempt: ${payload.timeoutSeconds}s`,
    `Playwright timeout: ${payload.playwrightTimeoutMs}ms`,
    `Shell timeout: ${payload.shellTimeoutSeconds}s`,
    `Port: ${payload.port}`,
    `Reuse existing server: ${payload.reuseExistingServer ? "yes" : "no"}`,
    `Namespace: ${payload.namespace}`,
    `Artifact dir: ${relativeRepoPath(payload.artifactDir)}`,
    `Playwright config: ${relativeRepoPath(payload.playwrightConfigPath)}`,
    `Known-signature ledger: ${relativeRepoPath(payload.ledgerPath)}`,
    "",
    "## Summary",
    "",
    `- Total replay cases: ${payload.summary.total}`,
    `- Passed: ${payload.summary.passed}`,
    `- Stable failed: ${payload.summary.stableFailed}`,
    `- Flaky: ${payload.summary.flaky}`,
    `- Timed out: ${payload.summary.timeout}`,
    `- Product-candidate findings: ${payload.summary.productCandidateFindings ?? 0}`,
    `- New product-candidate findings: ${payload.summary.newCandidateFindings ?? 0}`,
    `- Known duplicate findings: ${payload.summary.knownDuplicateFindings ?? 0}`,
    `- Known duplicate failures: ${payload.summary.knownDuplicateFailures ?? 0}`,
    `- Actionable failures: ${payload.summary.actionableFailures ?? 0}`,
    "",
    "## Classification",
    ""
  ];
  for (const [classification, count] of Object.entries(payload.summary.byClassification ?? {}).sort()) {
    lines.push(`- ${classification}: ${count}`);
  }
  lines.push(
    "",
    "## Primary Replayed Route Coverage",
    ""
  );
  const routeCounts = replayedRouteCounts(payload.results ?? []);
  if (routeCounts.length === 0) {
    lines.push("- None");
  } else {
    for (const [route, count] of routeCounts) lines.push(`- ${route}: ${count}`);
  }
  lines.push(
    "",
    "## All Replayed Route Tags",
    ""
  );
  const allRouteCounts = replayedAllRouteCounts(payload.results ?? []);
  if (allRouteCounts.length === 0) {
    lines.push("- None");
  } else {
    for (const [route, count] of allRouteCounts) lines.push(`- ${route}: ${count}`);
  }
  lines.push(
    "",
    "## Candidate Findings",
    ""
  );
  const findings = payload.findings ?? [];
  const newFindings = findings.filter((finding) => !finding.knownDuplicate);
  const knownFindings = findings.filter((finding) => finding.knownDuplicate);
  if (findings.length === 0) {
    lines.push("- None");
  } else {
    appendFindingGroup(lines, "New Product-Candidate Findings", newFindings);
    appendFindingGroup(lines, "Known Duplicate Findings", knownFindings);
  }
  lines.push(
    "",
    "## Replay Positions",
    ""
  );
  for (const item of payload.results) {
    const last = item.attempts.at(-1) ?? { status: "not-run", exitCode: null, logPath: "", failureSignature: "" };
    lines.push(`### ${item.caseId}`);
    lines.push("");
    lines.push(`- Seed: ${item.seed}`);
    lines.push(`- Status: ${item.status ?? last.status}`);
    lines.push(`- Classification: ${item.classification ?? "unknown"}`);
    if (item.status !== "passed") lines.push(`- Known duplicate: ${item.knownDuplicate ? "yes" : "no"}`);
    lines.push(`- Failure signature: ${item.failureSignature ?? last.failureSignature ?? ""}`);
    lines.push(`- Exit code: ${last.exitCode}`);
    lines.push(`- Replay score: ${item.score}`);
    if (item.diversityKey) lines.push(`- Diversity key: ${item.diversityKey}`);
    lines.push(`- Spec path: ${relativeRepoPath(item.specPath)}`);
    lines.push(`- Log path: ${last.logPath}`);
    lines.push(`- Coverage: ${item.coverage.join(", ")}`);
    lines.push(`- Steps: ${item.steps.join(" -> ")}`);
    if (last.excerpt) {
      lines.push("", "```text", last.excerpt, "```");
    }
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

function replayedRouteCounts(results) {
  const counts = new Map();
  for (const item of results) {
    const route = replayPrimaryRoute(item.coverage ?? []);
    counts.set(route, (counts.get(route) ?? 0) + 1);
  }
  return [...counts.entries()].sort((left, right) => left[0].localeCompare(right[0]));
}

function replayedAllRouteCounts(results) {
  const counts = new Map();
  for (const item of results) {
    const routes = new Set((item.coverage ?? []).filter((tag) => String(tag).startsWith("route:")));
    for (const route of routes) counts.set(route, (counts.get(route) ?? 0) + 1);
  }
  return [...counts.entries()].sort((left, right) => left[0].localeCompare(right[0]));
}

function replayPrimaryRoute(coverage) {
  const routes = coverage
    .filter((tag) => String(tag).startsWith("route:"))
    .sort();
  return routes.find((tag) => !["route:workspace", "route:agent-detail"].includes(tag)) ?? routes[0] ?? "route:none";
}

function appendFindingGroup(lines, heading, findings) {
  lines.push(`### ${heading}`, "");
  if (findings.length === 0) {
    lines.push("- None", "");
    return;
  }
  for (const finding of findings) {
    lines.push(`#### ${finding.title}`);
    lines.push("");
    lines.push(`- ID: ${finding.id}`);
    lines.push(`- Classification: ${finding.classification}`);
    lines.push(`- Status: ${finding.status}`);
    lines.push(`- Known duplicate: ${finding.knownDuplicate ? "yes" : "no"}`);
    lines.push(`- Seed/case: ${finding.seed} / ${finding.caseId}`);
    lines.push(`- Failure signature: ${finding.failureSignature}`);
    lines.push(`- Spec path: ${finding.specPath}`);
    lines.push(`- Last log path: ${finding.logPath}`);
    lines.push(`- Coverage: ${finding.coverage.join(", ")}`);
    lines.push(`- Trigger steps: ${finding.steps.join(" -> ")}`);
    if (finding.excerpt) {
      lines.push("", "```text", finding.excerpt, "```");
    }
    lines.push("");
  }
}

function relativeRepoPath(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}

function sanitizeNamespace(value) {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 80) || "bounded-replay";
}

function hashString(value) {
  let hash = 0;
  for (const char of value) hash = ((hash << 5) - hash + char.charCodeAt(0)) >>> 0;
  return hash;
}

async function writePlaywrightConfig(configPath, options) {
  const contents = `import { createRequire } from "node:module";

const require = createRequire(${JSON.stringify(path.join(options.desktopRoot, "package.json"))});
const { defineConfig } = require("@playwright/test");
const nodeExecutable = JSON.stringify(process.execPath);
const viteExecutable = ${JSON.stringify(path.join(options.desktopRoot, "node_modules", "vite", "bin", "vite.js"))};

export default defineConfig({
  testDir: ${JSON.stringify(options.specDir)},
  timeout: 120_000,
  expect: {
    timeout: 10_000
  },
  webServer: {
    command: \`\${nodeExecutable} \${JSON.stringify(viteExecutable)} --host 127.0.0.1 --port ${options.port}\`,
    cwd: ${JSON.stringify(options.desktopRoot)},
    url: "http://127.0.0.1:${options.port}/?skipOnboarding",
    reuseExistingServer: ${options.reuseExistingServer ? "true" : "false"},
    timeout: 120_000
  },
  use: {
    baseURL: "http://127.0.0.1:${options.port}",
    headless: true
  }
});
`;
  await writeFile(configPath, contents);
}

async function ensureNodeModulesLink(specDir, desktopNodeModules) {
  const linkPath = path.join(specDir, "node_modules");
  try {
    const stat = await lstat(linkPath);
    if (stat.isSymbolicLink() || stat.isDirectory()) return;
  } catch (error) {
    if (!error || error.code !== "ENOENT") throw error;
  }
  await symlink(desktopNodeModules, linkPath, "dir");
}

function failureSignature(output) {
  const excerpt = excerptFailure(output);
  const normalized = excerpt
    .replace(/\d{2,}/g, "#")
    .replace(/@[a-f0-9-]{8,}/gi, "@id")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 240);
  if (!normalized) return "";
  if (/one request per intent|one chat turn request per unique prompt intent|one browser navigate request per (?:unique URL|URL submit) intent|toHaveLength\(1\)|Expected length: 1|Received length: [2-9]/i.test(normalized)) {
    return "duplicate-request-per-intent";
  }
  if (/draft|typed url|typed message|preserved|toContainText|toHaveValue/i.test(normalized)) {
    return "draft-or-browser-state-not-preserved";
  }
  if (/locator|Timed out|waiting for|getByRole|getByLabel/i.test(normalized)) {
    return `locator-or-precondition:${normalized}`;
  }
  if (/Test timeout|TimeoutError|timeout of #ms exceeded|timeout \d+ms exceeded/i.test(normalized)) {
    return `test-timeout:${normalized}`;
  }
  if (/dialog|focus|activeElement|toBeFocused|focus trap/i.test(normalized)) {
    return "modal-focus-management";
  }
  if (/already used|reuseExistingServer|EADDRINUSE|address already in use|Port # is already in use|Port \d+ is already in use/i.test(normalized)) {
    return "harness-port-conflict";
  }
  if (/No tests found|did not match any files|outside of the testDir/i.test(normalized)) {
    return "harness-spec-discovery";
  }
  if (/Cannot find module .*node_modules\/vite|Process from config\.webServer was not able to start/i.test(normalized)) {
    return "harness-webserver-start";
  }
  if (/Cannot find package '@playwright\/test'|Cannot find module '@playwright\/test'/i.test(normalized)) {
    return "harness-dependency-resolution";
  }
  return normalized;
}

function classifyReplay(replay) {
  const statuses = replay.attempts.map((attempt) => attempt.status);
  const signatures = replay.attempts.map((attempt) => attempt.failureSignature).filter(Boolean);
  const uniqueStatuses = new Set(statuses);
  const uniqueSignatures = new Set(signatures);
  const failureSignatureValue = signatures.at(-1) ?? "";
  if (statuses.length === 0) {
    return { status: "not-run", classification: "harness-precondition", failureSignature: "" };
  }
  if (statuses.every((status) => status === "passed")) {
    return { status: "passed", classification: "no-finding", failureSignature: "" };
  }
  if (uniqueStatuses.has("passed")) {
    return { status: "flaky", classification: "flaky-environment", failureSignature: failureSignatureValue };
  }
  if (statuses.every((status) => status === "timeout")) {
    return { status: "timeout", classification: "timeout", failureSignature: failureSignatureValue };
  }
  if (uniqueStatuses.has("timeout")) {
    return { status: "unstable-timeout", classification: "flaky-environment", failureSignature: failureSignatureValue };
  }
  if (uniqueSignatures.size > 1) {
    const classifications = signatures.map((signature) => classifyFailureSignature(signature, replay));
    const uniqueClassifications = new Set(classifications);
    if (uniqueClassifications.size === 1) {
      return { status: "stable-failed", classification: classifications.at(-1), failureSignature: failureSignatureValue };
    }
    return { status: "stable-failed", classification: "needs-manual-triage", failureSignature: failureSignatureValue };
  }
  return {
    status: "stable-failed",
    classification: classifyFailureSignature(failureSignatureValue, replay),
    failureSignature: failureSignatureValue
  };
}

function classifyFailureSignature(signature, replay) {
  if (!signature) return "harness-precondition";
  if (signature === "duplicate-request-per-intent") return "product-candidate:duplicate-intent";
  if (signature === "draft-or-browser-state-not-preserved") return "product-candidate:draft-recovery";
  if (signature === "modal-focus-management") return "product-candidate:modal-focus";
  if (/stale tab URL leaked into active address/i.test(signature)) return "product-candidate:stale-browser-tab-state";
  if (signature === "harness-port-conflict") return "harness-precondition:port-conflict";
  if (signature === "harness-spec-discovery") return "harness-precondition:spec-discovery";
  if (signature === "harness-webserver-start") return "harness-precondition:webserver-start";
  if (signature === "harness-dependency-resolution") return "harness-precondition:dependency-resolution";
  if (signature.startsWith("locator-or-precondition:")) {
    if (/connection-banner|intercepts pointer events|Reconnect backend|Back to workspace/i.test(signature)) {
      return "product-candidate:connection-banner-blocks-navigation";
    }
    if ((replay.coverage ?? []).some((tag) => tag.startsWith("invariant:"))) return "harness-precondition";
    return "harness-precondition";
  }
  if (signature.startsWith("test-timeout:")) return "needs-manual-triage:timeout";
  return "product-candidate:unclassified";
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error.message}\n`);
  process.exitCode = 1;
});
