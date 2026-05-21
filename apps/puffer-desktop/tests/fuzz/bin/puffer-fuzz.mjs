#!/usr/bin/env node
import { mkdir, rmdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  buildPlan,
  buildRun,
  filterSeedsByProfile,
  formatAgentTask,
  formatPlanMarkdown,
  formatReportMarkdown,
  formatTopCasesMarkdown,
  loadLedger,
  loadSeeds,
  readJson,
  selectTopCases,
  summarizeRun,
  validateFramework,
  writeJson,
  writeText
} from "../lib/fuzz-core.mjs";
import { buildFrontier, formatFrontierMarkdown } from "../lib/frontier.mjs";
import { evaluateGate, formatGateMarkdown } from "../lib/gate.mjs";
import { buildReplayTemplate, defaultReplaySpecPath, formatReplayMarkdown, selectCase } from "../lib/replay-template.mjs";
import {
  applyReplayFeedback,
  buildShardSchedule,
  formatScheduleMarkdown,
  loadFeedbackLedger,
  loadShards,
  validateSchedulerModel
} from "../lib/scheduler.mjs";
import { bugSignature, findDuplicateSignatures } from "../lib/signature.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const defaultManifestPath = path.join(fuzzRoot, "manifests", "puffer-ui.json");
const defaultUiTreePath = path.join(fuzzRoot, "manifests", "puffer-ui-tree.json");
const defaultSeedDir = path.join(fuzzRoot, "seeds");
const defaultShardDir = path.join(fuzzRoot, "shards");
const defaultAdapterPath = path.join(fuzzRoot, "adapters", "playwright-actions.json");
const defaultLedgerPath = path.join(fuzzRoot, "coverage-ledger.json");
const defaultFeedbackLedgerPath = path.join(fuzzRoot, "feedback-ledger.json");
const defaultFakeDaemonPath = path.resolve(fuzzRoot, "..", "support", "fakeDaemon.ts");

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

async function loadContext(args) {
  const manifest = await readJson(args.manifest ?? defaultManifestPath);
  const allSeeds = await loadSeeds(args["seed-dir"] ?? defaultSeedDir);
  const profileSeeds = args.seed ? allSeeds : filterSeedsByProfile(allSeeds, args.profile ?? "all");
  const selected = args.seed
    ? allSeeds.filter((seed) => seed.id === args.seed || seed.file === args.seed)
    : profileSeeds;
  if (args.seed && selected.length === 0) {
    throw new Error(`Unknown seed: ${args.seed}`);
  }
  return { manifest, seeds: selected, allSeeds, profileSeeds };
}

function printHelp() {
  process.stdout.write(`Puffer UI/UX interaction fuzz helper

Commands:
  list
  plan --out /tmp/puffer_fuzz_plan.md
  run --seed chat-turn-race --iterations 12 --steps 18 --profile core --out /tmp/puffer_fuzz_run.json
  report --input /tmp/puffer_fuzz_run.json --out /tmp/puffer_fuzz_report.md
  top-cases --input /tmp/puffer_fuzz_run.json --limit 5 --out /tmp/top.json --report-out /tmp/top.md
  top-cases --input /tmp/puffer_fuzz_run.json --shard chat-composer-send --limit 5
  top-cases --input /tmp/puffer_fuzz_run.json --limit 5 --no-diversity
  agent-task --seed chat-turn-race --out /tmp/puffer_agent_task.md
  validate
  smoke
  frontier --out /tmp/puffer_fuzz_frontier.md
  gate --out /tmp/puffer_uiux_ready.md
  schedule --limit 4 --out apps/puffer-desktop/tests/fuzz/.runs/manual/schedule.md
  record-feedback --shard chat-composer-send --input apps/puffer-desktop/tests/fuzz/.runs/<run>/bounded-replay-report.json
  signature --finding finding.json
  replay --input run.json --case-id chat-turn-race-0001 --out /tmp/replay.spec.ts

Options:
  --manifest <path>   Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-ui.json
  --ui-tree <path>    Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-ui-tree.json
  --seed-dir <path>   Default: apps/puffer-desktop/tests/fuzz/seeds
  --shard-dir <path>  Default: apps/puffer-desktop/tests/fuzz/shards
  --adapter <path>    Default: apps/puffer-desktop/tests/fuzz/adapters/playwright-actions.json
  --ledger <path>     Default: apps/puffer-desktop/tests/fuzz/coverage-ledger.json
  --feedback-ledger <path> Default: apps/puffer-desktop/tests/fuzz/feedback-ledger.json
  --seed <id>         Select one seed; omit to run all seeds
  --shards <ids>      Comma-separated shard ids for scheduler filtering
  --shard <id>        Single shard id for top-cases or feedback
  --profile <name>    all, core, secondary, low-priority
  --iterations <n>    Generated cases per seed
  --steps <n>         Fuzz actions per case
  --rng-seed <text>   Deterministic RNG namespace
  --input <path>      Input JSON file for report/gate helpers
  --case-id <id>      Generated case id for replay scaffolding
  --out <path>        Output file
`);
}

async function main() {
  const [command = "help", ...rest] = process.argv.slice(2);
  const args = parseArgs(rest);

  if (command === "help" || command === "--help" || command === "-h") {
    printHelp();
    return;
  }

  if (command === "list") {
    const { manifest, allSeeds } = await loadContext(args);
    process.stdout.write(`Manifest: ${manifest.name} v${manifest.version}\n`);
    process.stdout.write(`Routes: ${manifest.routes.length}\n`);
    process.stdout.write(`Controls: ${manifest.controls.length}\n`);
    process.stdout.write(`States: ${manifest.states.length}\n`);
    process.stdout.write(`Async events: ${manifest.asyncEvents.length}\n`);
    process.stdout.write(`Invariants: ${manifest.invariants.length}\n\n`);
    for (const seed of allSeeds) {
      process.stdout.write(`- ${seed.id}: ${seed.focus}\n`);
    }
    return;
  }

  if (command === "plan") {
    const { manifest, seeds } = await loadContext(args);
    const ledger = await loadLedger(args.ledger ?? defaultLedgerPath);
    const plan = buildPlan(manifest, seeds, { limit: args.limit, ledger, profile: args.profile ?? "all" });
    const markdown = formatPlanMarkdown(plan);
    if (args.out) await writeText(args.out, markdown);
    process.stdout.write(markdown);
    return;
  }

  if (command === "run") {
    const { manifest, seeds } = await loadContext(args);
    const run = buildRun(manifest, seeds, {
      iterations: args.iterations,
      steps: args.steps,
      rngSeed: args["rng-seed"],
      profile: args.profile ?? "all"
    });
    if (args.out) {
      await writeJson(args.out, run);
    } else {
      process.stdout.write(`${JSON.stringify(run, null, 2)}\n`);
    }
    return;
  }

  if (command === "report") {
    if (!args.input) throw new Error("--input is required for report");
    const run = await readJson(args.input);
    const markdown = formatReportMarkdown(run);
    if (args.out) await writeText(args.out, markdown);
    process.stdout.write(markdown);
    return;
  }

  if (command === "top-cases") {
    if (!args.input) throw new Error("--input is required for top-cases");
    let run = await readJson(args.input);
    if (args.shard) {
      const { manifest } = await loadContext(args);
      const shards = await loadShards(args["shard-dir"] ?? defaultShardDir);
      const shard = shards.find((item) => item.id === args.shard);
      if (!shard) throw new Error(`Unknown shard: ${args.shard}`);
      run = filterRunToShard(run, manifest, shard);
    }
    const selection = selectTopCases(run, { limit: args.limit, diversity: args["no-diversity"] ? false : true });
    const markdown = formatTopCasesMarkdown(selection);
    if (args.out) await writeJson(args.out, selection);
    if (args["report-out"]) await writeText(args["report-out"], markdown);
    process.stdout.write(markdown);
    return;
  }

  if (command === "agent-task") {
    const { seeds } = await loadContext(args);
    if (seeds.length !== 1) throw new Error("--seed is required for agent-task");
    const markdown = formatAgentTask(seeds[0], {
      iterations: args.iterations,
      steps: args.steps
    });
    if (args.out) await writeText(args.out, markdown);
    process.stdout.write(markdown);
    return;
  }

  if (command === "validate") {
    const { manifest, seeds, allSeeds } = await loadContext(args);
    const adapter = await readJson(args.adapter ?? defaultAdapterPath);
    const uiTree = await readJson(args["ui-tree"] ?? defaultUiTreePath);
    const shards = await loadShards(args["shard-dir"] ?? defaultShardDir);
    const feedbackLedger = await loadFeedbackLedger(args["feedback-ledger"] ?? defaultFeedbackLedgerPath);
    let fakeDaemonSource = "";
    try {
      fakeDaemonSource = await readFileText(args["fake-daemon"] ?? defaultFakeDaemonPath);
    } catch {
      fakeDaemonSource = "";
    }
    const result = validateFramework(manifest, seeds, adapter, fakeDaemonSource);
    const schedulerResult = validateSchedulerModel(manifest, allSeeds, uiTree, shards, feedbackLedger);
    const errors = [...result.errors, ...schedulerResult.errors];
    const warnings = [...result.warnings, ...schedulerResult.warnings];
    const lines = [
      `Validation: ${errors.length === 0 ? "ok" : "failed"}`,
      `Errors: ${errors.length}`,
      `Warnings: ${warnings.length}`,
      ""
    ];
    if (errors.length > 0) {
      lines.push("Errors:");
      for (const item of errors) lines.push(`- ${item}`);
      lines.push("");
    }
    if (warnings.length > 0) {
      lines.push("Warnings:");
      for (const item of warnings) lines.push(`- ${item}`);
      lines.push("");
    }
    const output = `${lines.join("\n")}\n`;
    if (args.out) await writeText(args.out, output);
    process.stdout.write(output);
    if (errors.length > 0) process.exitCode = 1;
    return;
  }

  if (command === "smoke") {
    const { manifest, seeds } = await loadContext(args);
    const adapter = await readJson(args.adapter ?? defaultAdapterPath);
    const fakeDaemonSource = await readFileText(args["fake-daemon"] ?? defaultFakeDaemonPath).catch(() => "");
    const validation = validateFramework(manifest, seeds, adapter, fakeDaemonSource);
    const jsonOut = args["json-out"] ?? "/tmp/puffer_fuzz_smoke.json";
    const reportOut = args["report-out"] ?? "/tmp/puffer_fuzz_smoke.md";
    if (!validation.ok) {
      process.stdout.write(`Validation: failed\nErrors: ${validation.errorCount}\n`);
      for (const item of validation.errors) process.stdout.write(`- ${item}\n`);
      process.exitCode = 1;
      return;
    }
    const run = buildRun(manifest, seeds, {
      iterations: args.iterations ?? 1,
      steps: args.steps ?? 6,
      rngSeed: args["rng-seed"] ?? "smoke",
      profile: args.profile ?? "all"
    });
    await writeJson(jsonOut, run);
    await writeText(reportOut, formatReportMarkdown(run));
    process.stdout.write(`Validation: ok\n`);
    process.stdout.write(`Smoke cases: ${run.cases.length}\n`);
    process.stdout.write(`Run JSON: ${jsonOut}\n`);
    process.stdout.write(`Report: ${reportOut}\n`);
    return;
  }

  if (command === "frontier") {
    const { manifest, seeds } = await loadContext(args);
    const ledger = await loadLedger(args.ledger ?? defaultLedgerPath);
    const frontier = buildFrontier(manifest, seeds, ledger, { limit: args.limit });
    const markdown = formatFrontierMarkdown(frontier);
    if (args.out) await writeText(args.out, markdown);
    if (args["json-out"]) await writeJson(args["json-out"], frontier);
    process.stdout.write(markdown);
    return;
  }

  if (command === "gate") {
    const { manifest } = await loadContext(args);
    const ledger = await loadLedger(args.ledger ?? defaultLedgerPath);
    const result = evaluateGate(manifest, ledger, {
      highRiskCoverage: args["high-risk-coverage"],
      replaySuccessRate: args["replay-success-rate"],
      duplicateReportRate: args["duplicate-report-rate"],
      flakeRate: args["flake-rate"]
    });
    const markdown = formatGateMarkdown(result);
    if (args.out) await writeText(args.out, markdown);
    if (args["json-out"]) await writeJson(args["json-out"], result);
    process.stdout.write(markdown);
    if (!result.passed && args["fail-on-blocker"]) process.exitCode = 1;
    return;
  }

  if (command === "schedule") {
    const { manifest, allSeeds } = await loadContext(args);
    const uiTree = await readJson(args["ui-tree"] ?? defaultUiTreePath);
    const shards = await loadShards(args["shard-dir"] ?? defaultShardDir);
    const coverageLedger = await loadLedger(args.ledger ?? defaultLedgerPath);
    const feedbackLedger = await loadFeedbackLedger(args["feedback-ledger"] ?? defaultFeedbackLedgerPath);
    const validation = validateSchedulerModel(manifest, allSeeds, uiTree, shards, feedbackLedger);
    if (!validation.ok) {
      for (const item of validation.errors) process.stderr.write(`- ${item}\n`);
      process.exitCode = 1;
      return;
    }
    const schedule = buildShardSchedule(manifest, allSeeds, uiTree, shards, coverageLedger, feedbackLedger, {
      limit: args.limit,
      namespace: args.namespace,
      shards: args.shards,
      exclude: args.exclude,
      "min-iterations": args["min-iterations"],
      "max-iterations": args["max-iterations"]
    });
    const markdown = formatScheduleMarkdown(schedule);
    if (args.out) await writeText(args.out, markdown);
    if (args["json-out"]) await writeJson(args["json-out"], schedule);
    if (args.format === "json") {
      process.stdout.write(`${JSON.stringify(schedule, null, 2)}\n`);
    } else {
      process.stdout.write(markdown);
    }
    return;
  }

  if (command === "record-feedback") {
    if (!args.input) throw new Error("--input is required for record-feedback");
    if (!args.shard) throw new Error("--shard is required for record-feedback");
    const feedbackLedgerPath = args["feedback-ledger"] ?? defaultFeedbackLedgerPath;
    const outputLedgerPath = args.out ?? feedbackLedgerPath;
    await withFileLock(outputLedgerPath, async () => {
      const feedbackLedger = await loadFeedbackLedger(outputLedgerPath);
      const replayReport = await readJson(args.input);
      const next = applyReplayFeedback(feedbackLedger, replayReport, {
        shard: args.shard,
        namespace: args.namespace,
        input: args.input,
        "out-of-scope": args["out-of-scope"]
      });
      await writeJson(outputLedgerPath, next);
    });
    process.stdout.write(`Recorded feedback for shard ${args.shard}\n`);
    process.stdout.write(`Ledger: ${outputLedgerPath}\n`);
    return;
  }

  if (command === "signature") {
    if (!args.finding && !args.input) throw new Error("--finding or --input is required for signature");
    const finding = await readJson(args.finding ?? args.input);
    const signature = bugSignature(finding);
    const ledger = await loadLedger(args.ledger ?? defaultLedgerPath);
    const duplicates = findDuplicateSignatures(signature, [
      ...(ledger.knownBugSignatures ?? []),
      ...(ledger.fixedFindings ?? []).map((item) => item.bugSignature).filter(Boolean)
    ]);
    const result = { bugSignature: signature, duplicates, duplicate: duplicates.length > 0 };
    if (args.out) await writeJson(args.out, result);
    process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
    return;
  }

  if (command === "replay") {
    if (!args.input) throw new Error("--input is required for replay");
    if (!args["case-id"]) throw new Error("--case-id is required for replay");
    const run = await readJson(args.input);
    const selected = selectCase(run, args["case-id"]);
    const outputPath = args.out ?? defaultReplaySpecPath(selected);
    const resolvedOutputPath = path.resolve(outputPath);
    const template = buildReplayTemplate(selected, {
      coverageImport: args["coverage-import"] ??
        moduleSpecifier(resolvedOutputPath, path.join(fuzzRoot, "playwright", "pufferCoverage")),
      fakeDaemonImport: args["fake-daemon-import"] ??
        moduleSpecifier(resolvedOutputPath, path.resolve(fuzzRoot, "..", "support", "fakeDaemon"))
    });
    await writeText(outputPath, template);
    const markdown = formatReplayMarkdown(selected, outputPath);
    if (args["report-out"]) await writeText(args["report-out"], markdown);
    process.stdout.write(markdown);
    return;
  }

  throw new Error(`Unknown command: ${command}`);
}

function filterRunToShard(run, manifest, shard) {
  const selectorCoverage = shardSelectorCoverage(shard);
  const cases = (run.cases ?? []).filter((item) =>
    (item.coverage ?? []).some((tag) => selectorCoverage.has(tag))
  ).map((item) => projectCaseToShard(item, shard));
  if (cases.length === 0) {
    throw new Error(`No generated cases in ${run.options?.rngSeed ?? "run"} match shard ${shard.id}`);
  }
  return {
    ...run,
    cases,
    summary: summarizeRun(manifest, cases),
    shard: {
      id: shard.id,
      startNode: shard.startNode,
      ownedNodes: shard.ownedNodes ?? [],
      ownedCoverage: shard.ownedCoverage ?? [],
      selectorCoverage: [...selectorCoverage].sort()
    }
  };
}

function projectCaseToShard(testCase, shard) {
  const ownedCoverage = new Set(shard.ownedCoverage ?? []);
  const wantedInvariants = new Set((shard.invariants ?? []).map((item) => `invariant:${item}`));
  const allowedAsyncEvents = new Set((shard.allowedAsyncEvents ?? []).map((item) => `async:${item}`));
  const relevantPrefixes = shardRelevantPrefixes(ownedCoverage);
  const projectedSteps = [];
  let sawOwnedAction = false;
  let insertedNewAgentEntrypoint = false;
  for (const step of testCase.steps ?? []) {
    if (isShardSetupStep(step)) {
      projectedSteps.push(step);
      if (step.action === "open-new-agent") insertedNewAgentEntrypoint = true;
      continue;
    }
    if (step.phase === "assert") {
      if (wantedInvariants.has(`invariant:${step.target}`) || ownedCoverage.has(`invariant:${step.target}`)) {
        projectedSteps.push(step);
      }
      continue;
    }
    const stepCoverage = coverageForStep(step);
    const ownsStep = stepCoverage.some((tag) => ownedCoverage.has(tag));
    if (ownsStep) {
      if (!insertedNewAgentEntrypoint && step.target?.startsWith("new-agent.")) {
        projectedSteps.push({
          phase: "setup",
          action: "open-new-agent",
          kind: "ui",
          target: "new-agent-modal",
          params: {}
        });
        insertedNewAgentEntrypoint = true;
      }
      sawOwnedAction = true;
      projectedSteps.push(step);
      continue;
    }
    if (isRelevantShardAsyncStep(step, allowedAsyncEvents, relevantPrefixes)) {
      projectedSteps.push(step);
    }
  }

  const fallbackSteps = sawOwnedAction ? projectedSteps : testCase.steps ?? [];
  const projectedCoverage = (testCase.coverage ?? []).filter((tag) =>
    ownedCoverage.has(tag) ||
    wantedInvariants.has(tag) ||
    allowedAsyncEvents.has(tag) ||
    tag.startsWith("state:daemon.") ||
    tag.startsWith("state:session.")
  );
  return {
    ...testCase,
    diversityKey: `${testCase.seedId}|${shard.id}|${fallbackSteps.map((step) => step.action).join(">")}`,
    coverage: projectedCoverage.length > 0 ? [...new Set(projectedCoverage)] : testCase.coverage,
    steps: fallbackSteps,
    shard: {
      id: shard.id,
      startNode: shard.startNode,
      projected: sawOwnedAction
    }
  };
}

function isShardSetupStep(step) {
  return step.phase === "setup" ||
    [
      "open-agent-detail",
      "open-workspace",
      "open-settings-providers",
      "open-settings-mcp",
      "open-permissions",
      "open-new-agent",
      "open-pipelines"
    ].includes(step.action);
}

function coverageForStep(step) {
  const tags = [];
  if (step.target) {
    if (String(step.target).includes(".")) tags.push(`control:${step.target}`);
    if (["app-no-crash", "no-data-loss", "no-permanent-loading", "one-request-per-intent", "active-session-stable", "stale-error-scoped"].includes(step.target)) {
      tags.push(`invariant:${step.target}`);
    }
  }
  if (step.action === "open-terminal") tags.push("route:terminal-pane", "control:terminal.new-tab");
  if (step.action === "type-terminal") tags.push("control:terminal.input");
  if (step.action === "close-terminal") tags.push("control:terminal.close-tab");
  if (step.action === "open-file") tags.push("route:files-pane", "control:files.open");
  if (step.action === "edit-file") tags.push("control:files.editor");
  if (step.action === "save-file") tags.push("control:files.save");
  if (step.action === "open-browser") tags.push("route:browser-pane");
  return [...new Set(tags)];
}

function shardRelevantPrefixes(ownedCoverage) {
  const prefixes = new Set();
  for (const tag of ownedCoverage) {
    const match = tag.match(/^(?:control|route|state):([^.:-]+)/);
    if (match) prefixes.add(match[1]);
  }
  return prefixes;
}

function isRelevantShardAsyncStep(step, allowedAsyncEvents, relevantPrefixes) {
  if (step.action === "disconnect-reconnect") return allowedAsyncEvents.has("async:reconnect");
  if (step.action === "emit-late-pty-output") {
    return (allowedAsyncEvents.has("async:late-success") || allowedAsyncEvents.has("async:late-failure")) &&
      relevantPrefixes.has("terminal");
  }
  if (step.action === "emit-file-restore") {
    return allowedAsyncEvents.has("async:server-push-update") && relevantPrefixes.has("files");
  }
  if (step.action === "emit-permissions-refresh") {
    return allowedAsyncEvents.has("async:late-success") && relevantPrefixes.has("settings");
  }
  if (step.action === "emit-mcp-list-refresh" || step.action === "late-mcp-test-result") {
    return allowedAsyncEvents.has("async:late-success") && relevantPrefixes.has("settings");
  }
  if (step.action === "emit-state-for-old-tab" || step.action === "hold-next-browser-response") {
    return relevantPrefixes.has("browser");
  }
  return false;
}

function shardSelectorCoverage(shard) {
  const owned = shard.ownedCoverage ?? [];
  const specific = owned.filter((tag) => {
    if (tag.startsWith("invariant:")) return false;
    if (tag.startsWith("async:")) return false;
    if (tag.startsWith("state:daemon.")) return false;
    if (tag.startsWith("state:session.idle")) return false;
    if (tag.startsWith("control:modal.")) return false;
    return !["route:workspace", "route:agent-detail"].includes(tag);
  });
  const controls = specific.filter((tag) => tag.startsWith("control:"));
  if (controls.length > 0) return new Set(controls);
  const routes = specific.filter((tag) => tag.startsWith("route:"));
  if (routes.length > 0) return new Set(routes);
  if (specific.length > 0) return new Set(specific);
  return new Set(owned);
}

async function readFileText(filePath) {
  const { readFile } = await import("node:fs/promises");
  return readFile(filePath, "utf8");
}

function moduleSpecifier(fromFile, targetWithoutExtension) {
  const relative = path
    .relative(path.dirname(fromFile), targetWithoutExtension)
    .replaceAll(path.sep, "/");
  return relative.startsWith(".") ? relative : `./${relative}`;
}

async function withFileLock(filePath, callback) {
  const lockPath = `${path.resolve(filePath)}.lock`;
  const startedAt = Date.now();
  while (true) {
    try {
      await mkdir(lockPath, { recursive: false });
      break;
    } catch (error) {
      if (!error || error.code !== "EEXIST") throw error;
      if (Date.now() - startedAt > 30_000) throw new Error(`Timed out waiting for lock ${lockPath}`);
      await sleep(100);
    }
  }
  try {
    return await callback();
  } finally {
    await rmdir(lockPath).catch(() => {});
  }
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error.message}\n`);
  process.exitCode = 1;
});
