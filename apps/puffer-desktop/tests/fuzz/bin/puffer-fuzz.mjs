#!/usr/bin/env node
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
  validateFramework,
  writeJson,
  writeText
} from "../lib/fuzz-core.mjs";
import { buildFrontier, formatFrontierMarkdown } from "../lib/frontier.mjs";
import { evaluateGate, formatGateMarkdown } from "../lib/gate.mjs";
import { buildReplayTemplate, defaultReplaySpecPath, formatReplayMarkdown, selectCase } from "../lib/replay-template.mjs";
import { bugSignature, findDuplicateSignatures } from "../lib/signature.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const defaultManifestPath = path.join(fuzzRoot, "manifests", "puffer-ui.json");
const defaultSeedDir = path.join(fuzzRoot, "seeds");
const defaultAdapterPath = path.join(fuzzRoot, "adapters", "playwright-actions.json");
const defaultLedgerPath = path.join(fuzzRoot, "coverage-ledger.json");
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
  top-cases --input /tmp/puffer_fuzz_run.json --limit 5 --no-diversity
  agent-task --seed chat-turn-race --out /tmp/puffer_agent_task.md
  validate
  smoke
  frontier --out /tmp/puffer_fuzz_frontier.md
  gate --out /tmp/puffer_uiux_ready.md
  signature --finding finding.json
  replay --input run.json --case-id chat-turn-race-0001 --out /tmp/replay.spec.ts

Options:
  --manifest <path>   Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-ui.json
  --seed-dir <path>   Default: apps/puffer-desktop/tests/fuzz/seeds
  --adapter <path>    Default: apps/puffer-desktop/tests/fuzz/adapters/playwright-actions.json
  --ledger <path>     Default: apps/puffer-desktop/tests/fuzz/coverage-ledger.json
  --seed <id>         Select one seed; omit to run all seeds
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
    const run = await readJson(args.input);
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
    const { manifest, seeds } = await loadContext(args);
    const adapter = await readJson(args.adapter ?? defaultAdapterPath);
    let fakeDaemonSource = "";
    try {
      fakeDaemonSource = await readFileText(args["fake-daemon"] ?? defaultFakeDaemonPath);
    } catch {
      fakeDaemonSource = "";
    }
    const result = validateFramework(manifest, seeds, adapter, fakeDaemonSource);
    const lines = [
      `Validation: ${result.ok ? "ok" : "failed"}`,
      `Errors: ${result.errorCount}`,
      `Warnings: ${result.warningCount}`,
      ""
    ];
    if (result.errors.length > 0) {
      lines.push("Errors:");
      for (const item of result.errors) lines.push(`- ${item}`);
      lines.push("");
    }
    if (result.warnings.length > 0) {
      lines.push("Warnings:");
      for (const item of result.warnings) lines.push(`- ${item}`);
      lines.push("");
    }
    const output = `${lines.join("\n")}\n`;
    if (args.out) await writeText(args.out, output);
    process.stdout.write(output);
    if (!result.ok) process.exitCode = 1;
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

main().catch((error) => {
  process.stderr.write(`${error.stack ?? error.message}\n`);
  process.exitCode = 1;
});
