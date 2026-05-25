#!/usr/bin/env node
import { mkdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { buildEvidenceIndex } from "../lib/evidence-index.mjs";
import { loadShards } from "../lib/scheduler.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const fuzzCli = path.join(fuzzRoot, "bin", "puffer-fuzz.mjs");
const replayLoop = path.join(fuzzRoot, "bin", "puffer-fuzz-replay-loop.mjs");
const defaultShardDir = path.join(fuzzRoot, "shards");

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const shardId = requiredArg(args, "shard");
  const namespace = sanitizeNamespace(String(args.namespace ?? `bridge-${shardId}-${Date.now()}`));
  const tmpDir = path.resolve(String(args["tmp-dir"] ?? path.join(fuzzRoot, ".runs", namespace)));
  const shardDir = path.resolve(String(args["shard-dir"] ?? defaultShardDir));
  const shards = await loadShards(shardDir);
  const bridgeShard = shards.find((item) => item.id === shardId);
  if (!bridgeShard) throw new Error(`Unknown bridge shard: ${shardId}`);
  if (!bridgeShard.bridge?.leftShard || !bridgeShard.bridge?.rightShard) {
    throw new Error(`Shard ${shardId} does not declare bridge.leftShard/rightShard`);
  }

  const leftShard = requireShard(shards, bridgeShard.bridge.leftShard);
  const rightShard = requireShard(shards, bridgeShard.bridge.rightShard);
  await mkdir(tmpDir, { recursive: true });

  const sourceRunPath = path.join(tmpDir, "source-run.json");
  if (args.input) {
    await writeFile(sourceRunPath, await readFile(path.resolve(String(args.input)), "utf8"));
  } else {
    await runCommand("node", [
      fuzzCli,
      "run",
      "--seed",
      String(args.seed ?? bridgeShard.seed),
      "--iterations",
      String(args.iterations ?? Math.max(Number(leftShard.iterations ?? 8), Number(rightShard.iterations ?? 8), Number(bridgeShard.iterations ?? 8))),
      "--steps",
      String(args.steps ?? Math.max(Number(leftShard.steps ?? 12), Number(rightShard.steps ?? 12), Number(bridgeShard.steps ?? 12))),
      "--rng-seed",
      String(args["rng-seed"] ?? namespace),
      "--out",
      sourceRunPath
    ], { cwd: repoRoot, timeoutSeconds: 90 });
  }

  const leftTopPath = path.join(tmpDir, "left-top.json");
  const rightTopPath = path.join(tmpDir, "right-top.json");
  await selectBridgeTopCase(sourceRunPath, leftShard.id, leftTopPath);
  await selectBridgeTopCase(sourceRunPath, rightShard.id, rightTopPath);
  const leftTop = JSON.parse(await readFile(leftTopPath, "utf8"));
  const rightTop = JSON.parse(await readFile(rightTopPath, "utf8"));
  const leftCase = firstCase(leftTop, leftShard.id);
  const rightCase = firstCase(rightTop, rightShard.id);
  const combinedRun = buildCombinedRun({ bridgeShard, leftShard, rightShard, leftCase, rightCase, namespace });
  const combinedRunPath = path.join(tmpDir, "bridge-run.json");
  await writeFile(combinedRunPath, `${JSON.stringify(combinedRun, null, 2)}\n`);

  const replayArgs = [
    replayLoop,
    "--input",
    combinedRunPath,
    "--seeds",
    bridgeShard.seed,
    "--limit",
    String(args.limit ?? bridgeShard.replayLimit ?? 1),
    "--attempts",
    String(args.attempts ?? 1),
    "--timeout",
    String(args.timeout ?? 120),
    "--rng-seed",
    namespace,
    "--namespace",
    namespace
  ];
  if (args["fail-on-new-finding"]) replayArgs.push("--fail-on-new-finding");
  const replay = await runCommand("node", replayArgs, { cwd: repoRoot, timeoutSeconds: Number(args.timeout ?? 120) + 90, allowFailure: true });
  const replayJsonPath = path.join(tmpDir, "bounded-replay-report.json");
  const payload = JSON.parse(await readFile(replayJsonPath, "utf8"));
  const enhanced = enhanceBridgeReport(payload, {
    bridgeShard,
    leftShard,
    rightShard,
    leftCase,
    rightCase,
    namespace,
    replayExitCode: replay.exitCode,
    combinedRunPath,
    leftTopPath,
    rightTopPath
  });
  await writeFile(replayJsonPath, `${JSON.stringify(enhanced, null, 2)}\n`);
  await writeFile(path.join(tmpDir, "bounded-replay-report.md"), formatBridgeMarkdown(enhanced));
  process.stdout.write(`Bridge report: ${path.relative(repoRoot, replayJsonPath).replaceAll(path.sep, "/")}\n`);
  process.stdout.write(`Bridge witness: ${bridgeShard.bridge.sharedWitness}\n`);
  process.stdout.write(`Bridge replay exit: ${replay.exitCode}\n`);
  if (replay.exitCode !== 0) process.exitCode = replay.exitCode;
}

function requireShard(shards, shardId) {
  const shard = shards.find((item) => item.id === shardId);
  if (!shard) throw new Error(`Bridge references unknown shard: ${shardId}`);
  return shard;
}

async function selectBridgeTopCase(sourceRunPath, shardId, outPath) {
  await runCommand("node", [
    fuzzCli,
    "top-cases",
    "--input",
    sourceRunPath,
    "--shard",
    shardId,
    "--limit",
    "1",
    "--out",
    outPath
  ], { cwd: repoRoot, timeoutSeconds: 90, quiet: true });
}

function firstCase(selection, shardId) {
  const item = selection.cases?.[0];
  if (!item) throw new Error(`No top case selected for bridge side ${shardId}`);
  return item;
}

function buildCombinedRun({ bridgeShard, leftShard, rightShard, leftCase, rightCase, namespace }) {
  const leftSteps = normalizeBridgeSideSteps(leftCase.steps ?? []);
  const rightSteps = normalizeBridgeSideSteps(rightCase.steps ?? []);
  const combinedCase = {
    caseId: `${bridgeShard.id}-bridge-0001`,
    seedId: bridgeShard.seed,
    focus: bridgeShard.title ?? bridgeShard.id,
    severityTarget: "cross-shard-bridge",
    coverage: unique([
      ...(leftCase.coverage ?? []),
      ...(rightCase.coverage ?? []),
      ...(bridgeShard.ownedCoverage ?? []),
      `bridge:${leftShard.id}+${rightShard.id}`
    ]),
    steps: [
      ...leftSteps,
      {
        phase: "fuzz",
        action: "disconnect-reconnect",
        kind: "daemon",
        target: "daemon.reconnect",
        params: { bridgeCheckpoint: true, witness: bridgeShard.bridge?.sharedWitness ?? "" },
        coverage: ["async:reconnect", "invariant:active-session-stable"]
      },
      ...rightSteps
    ],
    bridge: {
      id: bridgeShard.id,
      leftShard: leftShard.id,
      rightShard: rightShard.id,
      sharedWitness: bridgeShard.bridge?.sharedWitness ?? "",
      leftCaseId: leftCase.caseId,
      rightCaseId: rightCase.caseId
    }
  };
  return {
    version: 1,
    manifestVersion: "bridge-combined",
    generatedAt: new Date().toISOString(),
    options: {
      rngSeed: namespace,
      profile: "bridge"
    },
    bridge: combinedCase.bridge,
    cases: [combinedCase],
    summary: {
      caseCount: 1,
      generatedAt: new Date().toISOString(),
      coveredTags: combinedCase.coverage
    }
  };
}

function normalizeBridgeSideSteps(steps) {
  return steps.map((step) => ({
    phase: step.phase ?? "fuzz",
    action: step.action,
    kind: step.kind ?? "ui",
    target: step.target ?? "",
    params: step.params ?? {},
    coverage: step.coverage ?? []
  }));
}

function enhanceBridgeReport(payload, context) {
  const bridge = {
    id: context.bridgeShard.id,
    executionMode: "combined-left-right",
    leftShard: context.leftShard.id,
    rightShard: context.rightShard.id,
    sharedWitness: context.bridgeShard.bridge?.sharedWitness ?? "",
    leftCaseId: context.leftCase.caseId,
    rightCaseId: context.rightCase.caseId,
    replayExitCode: context.replayExitCode,
    artifacts: {
      combinedRun: relativeRepoPath(context.combinedRunPath),
      leftTop: relativeRepoPath(context.leftTopPath),
      rightTop: relativeRepoPath(context.rightTopPath)
    }
  };
  const records = [];
  for (const entry of payload.evidence_index ?? []) {
    records.push({
      type: entry.type,
      value: entry.value,
      metadata: { ...(entry.metadata ?? {}), previousEvidenceId: entry.id }
    });
  }
  records.push({
    type: "action",
    value: JSON.stringify({
      bridge,
      leftSteps: context.leftCase.steps?.map((step) => step.action) ?? [],
      rightSteps: context.rightCase.steps?.map((step) => step.action) ?? []
    }),
    metadata: { bridgeId: bridge.id, bridgePhase: "combined-sequence" }
  });
  const hasFailure = (payload.results ?? []).some((item) => item.status !== "passed");
  if (hasFailure) {
    records.push({
      type: "predicate",
      value: JSON.stringify({
        bridgeId: bridge.id,
        predicate: "bridge-combined-replay-failed",
        leftShard: bridge.leftShard,
        rightShard: bridge.rightShard,
        sharedWitness: bridge.sharedWitness,
        summary: payload.summary ?? {}
      }),
      metadata: { bridgeId: bridge.id, predicate: "bridge-combined-replay-failed" }
    });
  }
  return {
    ...payload,
    bridge,
    ...buildEvidenceIndex(records)
  };
}

function formatBridgeMarkdown(payload) {
  const bridge = payload.bridge ?? {};
  const lines = [
    "# Puffer Bridge UI/UX Replay Report",
    "",
    `Started: ${payload.startedAt}`,
    `Finished: ${payload.finishedAt}`,
    `Namespace: ${payload.namespace}`,
    `Bridge shard: ${bridge.id ?? payload.shard ?? ""}`,
    `Execution mode: ${bridge.executionMode ?? "unknown"}`,
    `Left shard: ${bridge.leftShard ?? ""}`,
    `Right shard: ${bridge.rightShard ?? ""}`,
    `Shared witness: ${bridge.sharedWitness ?? ""}`,
    `Left case: ${bridge.leftCaseId ?? ""}`,
    `Right case: ${bridge.rightCaseId ?? ""}`,
    "",
    "## Summary",
    "",
    `- Total replay cases: ${payload.summary?.total ?? 0}`,
    `- Passed: ${payload.summary?.passed ?? 0}`,
    `- Stable failed: ${payload.summary?.stableFailed ?? 0}`,
    `- Flaky: ${payload.summary?.flaky ?? 0}`,
    `- Timed out: ${payload.summary?.timeout ?? 0}`,
    `- Actionable product failures: ${payload.summary?.actionableFailures ?? 0}`,
    `- Evidence entries: ${payload.evidence_index?.length ?? 0}`,
    "",
    "## Artifacts",
    "",
    `- Combined run: ${bridge.artifacts?.combinedRun ?? ""}`,
    `- Left top case: ${bridge.artifacts?.leftTop ?? ""}`,
    `- Right top case: ${bridge.artifacts?.rightTop ?? ""}`,
    "",
    "## Replay Results",
    ""
  ];
  for (const item of payload.results ?? []) {
    lines.push(`- ${item.caseId}: ${item.status}; ${item.classification ?? "unknown"}`);
  }
  return `${lines.join("\n")}\n`;
}

function unique(items) {
  return [...new Set(items.filter(Boolean))].sort();
}

function runCommand(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env ?? process.env,
      stdio: ["ignore", "pipe", "pipe"]
    });
    let output = "";
    const timer = setTimeout(() => {
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
      reject(error);
    });
    child.on("close", (exitCode) => {
      clearTimeout(timer);
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

function parseArgs(argv) {
  const args = {};
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith("--")) continue;
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

function requiredArg(args, key) {
  if (args[key] === undefined || args[key] === true || String(args[key]).trim() === "") {
    throw new Error(`--${key} is required`);
  }
  return String(args[key]);
}

function sanitizeNamespace(value) {
  return value.replace(/[^a-zA-Z0-9._-]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 80) || "bridge-replay";
}

function relativeRepoPath(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}

main().catch((error) => {
  console.error(error?.stack ?? error?.message ?? error);
  process.exitCode = 1;
});
