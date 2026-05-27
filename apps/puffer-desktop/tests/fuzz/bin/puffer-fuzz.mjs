#!/usr/bin/env node
import { mkdir, readFile, rmdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  applyReplayCoverageToLedger,
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
import {
  addReplayReportToCorpus,
  buildRunFromCorpus,
  formatCorpusMarkdown,
  loadCorpus,
  summarizeCorpus,
  writeCorpus,
  writeCorpusMarkdown
} from "../lib/corpus.mjs";
import { buildBugMemory, formatBugMemoryMarkdown, writeBugMemory, writeBugMemoryMarkdown } from "../lib/bug-memory.mjs";
import { buildPromptEvolutionPack } from "../lib/prompt-evolution.mjs";
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
import { shrinkRunCase } from "../lib/shrinker.mjs";
import { buildScenarioPlan, formatScenarioPlanMarkdown, writeScenarioPlan, writeScenarioPlanMarkdown } from "../lib/scenario-plan.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const defaultManifestPath = path.join(fuzzRoot, "manifests", "puffer-ui.json");
const defaultUiTreePath = path.join(fuzzRoot, "manifests", "puffer-ui-tree.json");
const defaultIntentManifestPath = path.join(fuzzRoot, "manifests", "puffer-intents.json");
const defaultScenarioManifestPath = path.join(fuzzRoot, "manifests", "puffer-scenarios.json");
const defaultSeedDir = path.join(fuzzRoot, "seeds");
const defaultShardDir = path.join(fuzzRoot, "shards");
const defaultAdapterPath = path.join(fuzzRoot, "adapters", "playwright-actions.json");
const defaultLedgerPath = path.join(fuzzRoot, "coverage-ledger.json");
const defaultFeedbackLedgerPath = path.join(fuzzRoot, "feedback-ledger.json");
const defaultBugListPath = path.join(fuzzRoot, "BUGS.md");
const defaultCorpusPath = path.join(fuzzRoot, "corpus", "puffer-corpus.json");
const defaultPromptEvolutionPath = path.join(fuzzRoot, "prompt_evolution.md");
const defaultIssuePath = "/tmp/puffer_issue.md";
const defaultPicsDir = path.resolve(fuzzRoot, "..", "..", "..", "..", "bugs", "pics");
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
  gate --gate-profile bootstrap --out /tmp/puffer_uiux_ready.md
  schedule --limit 4 --out apps/puffer-desktop/tests/fuzz/.runs/manual/schedule.md
  record-feedback --shard chat-composer-send --input apps/puffer-desktop/tests/fuzz/.runs/<run>/bounded-replay-report.json
  evolve-prompt --out apps/puffer-desktop/tests/fuzz/.runs/manual/prompt-evolution.md
  corpus --from-replay apps/puffer-desktop/tests/fuzz/.runs/<run>/bounded-replay-report.json --out apps/puffer-desktop/tests/fuzz/.runs/<run>/corpus.json
  corpus --input apps/puffer-desktop/tests/fuzz/.runs/<run>/corpus.json --run-out apps/puffer-desktop/tests/fuzz/.runs/<run>/corpus-run.json
  bug-memory --runs-dir apps/puffer-desktop/tests/fuzz/.runs --out /tmp/puffer_bug_memory.json --report-out /tmp/puffer_bug_memory.md
  scenario-plan --out /tmp/puffer_scenarios.json --report-out /tmp/puffer_scenarios.md
  signature --finding finding.json
  replay --input run.json --case-id chat-turn-race-0001 --out /tmp/replay.spec.ts
  shrink --input run.json --case-id chat-turn-race-0001 --out /tmp/shrunk-run.json --report-out /tmp/shrink.md
  bug-list --append --title "..." --severity P1 --area chat --shard chat-composer-send --evidence apps/.../final.md
  bug-list --set-status --id PUF-FUZZ-0001 --status fixed --note "fixed by abc123"

Related helper:
  puffer-fuzz-replay-loop.mjs --input run.json --seeds chat-turn-race --shard chat-composer-send

Options:
  --manifest <path>   Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-ui.json
  --ui-tree <path>    Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-ui-tree.json
  --intents <path>    Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-intents.json
  --scenarios <path>  Default: apps/puffer-desktop/tests/fuzz/manifests/puffer-scenarios.json
  --seed-dir <path>   Default: apps/puffer-desktop/tests/fuzz/seeds
  --shard-dir <path>  Default: apps/puffer-desktop/tests/fuzz/shards
  --adapter <path>    Default: apps/puffer-desktop/tests/fuzz/adapters/playwright-actions.json
  --ledger <path>     Default: apps/puffer-desktop/tests/fuzz/coverage-ledger.json
  --no-coverage-ledger Do not update runtime coverage when recording feedback
  --feedback-ledger <path> Default: apps/puffer-desktop/tests/fuzz/feedback-ledger.json
  --corpus <path>    Default: apps/puffer-desktop/tests/fuzz/corpus/puffer-corpus.json
  --bug-list <path> Default: apps/puffer-desktop/tests/fuzz/BUGS.md
  --prompt-guide <path> Default: apps/puffer-desktop/tests/fuzz/prompt_evolution.md
  --issue <path>      Default: /tmp/puffer_issue.md
  --pics-dir <path>   Default: bugs/pics
  --seed <id>         Select one seed; omit to run all seeds
  --shards <ids>      Comma-separated shard ids for scheduler filtering
  --shard <id>        Single shard id for top-cases or feedback
  --profile <name>    all, core, secondary, low-priority
  --gate-profile <name> bootstrap, ready, release
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
    const manifest = await readJson(args.manifest ?? defaultManifestPath);
    const ledger = await loadLedger(args.ledger ?? defaultLedgerPath);
    const result = evaluateGate(manifest, ledger, {
      profile: args["gate-profile"] ?? args.profile ?? "ready",
      highRiskCoverage: args["high-risk-coverage"],
      replaySuccessRate: args["replay-success-rate"],
      duplicateReportRate: args["duplicate-report-rate"],
      flakeRate: args["flake-rate"],
      minReplayCases: args["min-replay-cases"]
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
    const intentManifest = await readJson(args.intents ?? defaultIntentManifestPath);
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
      intentManifest,
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
    const coverageLedgerPath = args.ledger ?? defaultLedgerPath;
    const coverageOutputPath = args["coverage-ledger-out"] ?? coverageLedgerPath;
    const replayReport = await readJson(args.input);
    await withFileLock(outputLedgerPath, async () => {
      const feedbackLedger = await loadFeedbackLedger(outputLedgerPath);
      const next = applyReplayFeedback(feedbackLedger, replayReport, {
        shard: args.shard,
        namespace: args.namespace,
        input: args.input,
        "out-of-scope": args["out-of-scope"]
      });
      await writeJson(outputLedgerPath, next);
    });
    if (!args["no-coverage-ledger"]) {
      await withFileLock(coverageOutputPath, async () => {
        const coverageLedger = await loadLedger(coverageOutputPath);
        const nextCoverage = applyReplayCoverageToLedger(coverageLedger, replayReport, {
          shard: args.shard,
          namespace: args.namespace
        });
        await writeJson(coverageOutputPath, nextCoverage);
      });
    }
    process.stdout.write(`Recorded feedback for shard ${args.shard}\n`);
    process.stdout.write(`Ledger: ${outputLedgerPath}\n`);
    if (!args["no-coverage-ledger"]) process.stdout.write(`Coverage ledger: ${coverageOutputPath}\n`);
    return;
  }

  if (command === "evolve-prompt") {
    const result = await buildPromptEvolutionPack({
      baseGuidePath: args["prompt-guide"] ?? defaultPromptEvolutionPath,
      bugListPath: args["bug-list"] ?? defaultBugListPath,
      feedbackLedgerPath: args["feedback-ledger"] ?? defaultFeedbackLedgerPath,
      bugMemoryPath: args["bug-memory"],
      issuePath: args.issue ?? defaultIssuePath,
      picsDir: args["pics-dir"] ?? defaultPicsDir
    });
    if (args.out) await writeText(args.out, result.markdown);
    if (args["json-out"]) await writeJson(args["json-out"], result.pack);
    process.stdout.write(result.markdown);
    return;
  }

  if (command === "corpus") {
    const corpusPath = args.input ?? args.corpus ?? defaultCorpusPath;
    let corpus = await loadCorpus(corpusPath);
    if (args["from-replay"]) {
      const replayReport = await readJson(args["from-replay"]);
      corpus = addReplayReportToCorpus(corpus, replayReport, {
        shard: args.shard,
        namespace: args.namespace
      });
      if (args.out) await writeCorpus(args.out, corpus);
    }
    const markdown = formatCorpusMarkdown(corpus);
    if (args["report-out"]) await writeCorpusMarkdown(args["report-out"], corpus);
    if (args["summary-out"]) await writeJson(args["summary-out"], summarizeCorpus(corpus));
    if (args["run-out"]) {
      const manifest = await readJson(args.manifest ?? defaultManifestPath);
      const run = buildRunFromCorpus(manifest, corpus, {
        limit: args.limit,
        rngSeed: args["rng-seed"],
        mutate: args["no-mutate"] ? false : true
      });
      await writeJson(args["run-out"], run);
    }
    process.stdout.write(markdown);
    return;
  }

  if (command === "bug-memory") {
    const memory = await buildBugMemory({
      runsDir: args["runs-dir"] ?? path.join(fuzzRoot, ".runs"),
      limit: args.limit
    });
    if (args.out) await writeBugMemory(args.out, memory);
    if (args["report-out"]) await writeBugMemoryMarkdown(args["report-out"], memory);
    process.stdout.write(formatBugMemoryMarkdown(memory));
    return;
  }

  if (command === "scenario-plan") {
    const scenarioManifest = await readJson(args.scenarios ?? defaultScenarioManifestPath);
    const plan = buildScenarioPlan(scenarioManifest, { limit: args.limit });
    if (args.out) await writeScenarioPlan(args.out, plan);
    if (args["report-out"]) await writeScenarioPlanMarkdown(args["report-out"], plan);
    process.stdout.write(formatScenarioPlanMarkdown(plan));
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

  if (command === "shrink") {
    if (!args.input) throw new Error("--input is required for shrink");
    if (!args["case-id"]) throw new Error("--case-id is required for shrink");
    const manifest = await readJson(args.manifest ?? defaultManifestPath);
    const run = await readJson(args.input);
    const result = shrinkRunCase(manifest, run, args["case-id"], {
      verified: Boolean(args.verified)
    });
    if (args.out) await writeJson(args.out, result.run);
    if (args["report-out"]) await writeText(args["report-out"], result.report);
    process.stdout.write(result.report);
    return;
  }

  if (command === "bug-list") {
    const bugListPath = args["bug-list"] ?? defaultBugListPath;
    if (args.append) {
      const text = await appendBugListEntry(bugListPath, args);
      process.stdout.write(text);
      return;
    }
    if (args["set-status"]) {
      const text = await updateBugListStatus(bugListPath, args);
      process.stdout.write(text);
      return;
    }
    await ensureBugList(bugListPath);
    process.stdout.write(`Bug list: ${bugListPath}\n`);
    return;
  }

  throw new Error(`Unknown command: ${command}`);
}

const bugListStatusValues = new Set(["pending", "fixed", "duplicate", "rejected", "out-of-scope"]);

function bugListTemplate() {
  return `# Puffer UI/UX Fuzz Bug List

This file is the main-agent-owned ledger for confirmed or candidate UI/UX fuzz
findings. Subagents should not edit this file directly. They should report a
finding block in their final shard report, then the main agent appends it here
with \`puffer-fuzz.mjs bug-list --append\`.

## Status Values

- \`pending\`: accepted as a real product candidate, not fixed yet.
- \`fixed\`: fixed with regression coverage.
- \`duplicate\`: same root cause as an existing ledger entry.
- \`rejected\`: investigated and not a product bug.
- \`out-of-scope\`: real evidence, but outside the shard or current campaign.

## Ledger

| ID | Status | Severity | Area | Shard | Title | Evidence | Updated |
| --- | --- | --- | --- | --- | --- | --- | --- |

## Details

`;
}

async function ensureBugList(filePath) {
  try {
    await readFile(filePath, "utf8");
  } catch (error) {
    if (!error || error.code !== "ENOENT") throw error;
    await mkdir(path.dirname(filePath), { recursive: true });
    await writeFile(filePath, bugListTemplate(), "utf8");
  }
}

async function readBugList(filePath) {
  await ensureBugList(filePath);
  return readFile(filePath, "utf8");
}

async function writeBugList(filePath, text) {
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, text, "utf8");
}

async function appendBugListEntry(filePath, args) {
  const title = requiredArg(args, "title");
  const status = normalizeBugStatus(args.status ?? "pending");
  const severity = args.severity ?? "P2";
  const area = args.area ?? "unknown";
  const shard = args.shard ?? "unknown";
  const evidence = args.evidence ?? args.report ?? "";
  const sourceRun = args["source-run"] ?? "";
  const stability = args.stability ?? "";
  const impact = args.impact ?? "";
  const expected = args.expected ?? "";
  const actual = args.actual ?? "";
  const repro = args.repro ?? "";
  const notes = args.notes ?? "";
  const updated = new Date().toISOString();

  return withFileLock(filePath, async () => {
    const current = await readBugList(filePath);
    const id = args.id ?? nextBugId(current);
    if (current.includes(`| ${id} |`) || current.includes(`### ${id}:`)) {
      throw new Error(`Bug id already exists in ${filePath}: ${id}`);
    }
    const row = `| ${escapeTable(id)} | ${escapeTable(status)} | ${escapeTable(severity)} | ${escapeTable(area)} | ${escapeTable(shard)} | ${escapeTable(title)} | ${escapeTable(evidence)} | ${escapeTable(updated)} |`;
    const detail = [
      `### ${id}: ${title}`,
      "",
      `- Status: ${status}`,
      `- Severity: ${severity}`,
      `- Area: ${area}`,
      `- Shard: ${shard}`,
      `- Source run: ${sourceRun || "n/a"}`,
      `- Evidence: ${evidence || "n/a"}`,
      `- Stability: ${stability || "n/a"}`,
      `- Expected: ${expected || "n/a"}`,
      `- Actual: ${actual || "n/a"}`,
      `- Impact: ${impact || "n/a"}`,
      `- Repro: ${repro || "n/a"}`,
      `- Notes: ${notes || "n/a"}`,
      `- Updated: ${updated}`,
      ""
    ].join("\n");
    const next = insertBugListRow(current, row).trimEnd() + `\n\n${detail}`;
    await writeBugList(filePath, next);
    return `Appended ${id} to ${filePath}\n`;
  });
}

async function updateBugListStatus(filePath, args) {
  const id = requiredArg(args, "id");
  const status = normalizeBugStatus(requiredArg(args, "status"));
  const note = args.note ?? "";
  const updated = new Date().toISOString();

  return withFileLock(filePath, async () => {
    const current = await readBugList(filePath);
    if (!current.includes(`| ${id} |`) && !current.includes(`### ${id}:`)) {
      throw new Error(`Bug id not found in ${filePath}: ${id}`);
    }
    const lines = current.split("\n").map((line) => {
      if (!line.startsWith(`| ${id} |`)) return line;
      const parts = line.split("|").map((item) => item.trim());
      if (parts.length < 9) return line;
      parts[2] = status;
      parts[8] = updated;
      return `| ${parts.slice(1, 9).join(" | ")} |`;
    });
    const statusPattern = new RegExp(`(### ${escapeRegExp(id)}:[\\s\\S]*?\\n- Status: )[^\\n]+`);
    let next = lines.join("\n").replace(statusPattern, `$1${status}`);
    const detailPattern = new RegExp(`(### ${escapeRegExp(id)}:[\\s\\S]*?)(?=\\n### PUF-FUZZ-|\\n*$)`);
    next = next.replace(detailPattern, (match) => {
      const lines = [`${match.trimEnd()}`, `- Status update: ${updated} ${status}${note ? ` - ${note}` : ""}`, ""];
      return lines.join("\n");
    });
    await writeBugList(filePath, next);
    return `Updated ${id} to ${status} in ${filePath}\n`;
  });
}

function insertBugListRow(text, row) {
  const marker = "## Details";
  const index = text.indexOf(`\n${marker}`);
  if (index === -1) return `${text.trimEnd()}\n${row}\n`;
  const before = text.slice(0, index).trimEnd();
  const after = text.slice(index);
  return `${before}\n${row}\n${after}`;
}

function nextBugId(text) {
  const matches = [...text.matchAll(/PUF-FUZZ-(\d{4})/g)];
  const max = matches.reduce((value, match) => Math.max(value, Number(match[1])), 0);
  return `PUF-FUZZ-${String(max + 1).padStart(4, "0")}`;
}

function normalizeBugStatus(value) {
  const status = String(value).trim().toLowerCase();
  if (!bugListStatusValues.has(status)) {
    throw new Error(`Invalid status "${value}". Expected one of: ${[...bugListStatusValues].join(", ")}`);
  }
  return status;
}

function requiredArg(args, key) {
  const value = args[key];
  if (value === undefined || value === true || String(value).trim() === "") {
    throw new Error(`--${key} is required`);
  }
  return String(value);
}

function escapeTable(value) {
  return String(value ?? "").replaceAll("|", "\\|").replace(/\s+/g, " ").trim();
}

function escapeRegExp(value) {
  return String(value).replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
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
      "open-settings-connectors",
      "open-settings-mcp",
      "open-permissions",
      "open-new-agent",
      "open-workflows"
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
