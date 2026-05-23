import { readdir } from "node:fs/promises";
import path from "node:path";
import { indexCoverageTargets, productTierForSeed, readJson } from "./fuzz-core.mjs";

export async function loadShards(shardDir) {
  const files = (await readdir(shardDir))
    .filter((name) => name.endsWith(".json"))
    .sort();
  const shards = [];
  for (const file of files) {
    shards.push({
      ...(await readJson(path.join(shardDir, file))),
      file
    });
  }
  return shards.sort((left, right) => String(left.id).localeCompare(String(right.id)));
}

export async function loadFeedbackLedger(filePath) {
  try {
    return normalizeFeedbackLedger(await readJson(filePath));
  } catch (error) {
    if (error && error.code === "ENOENT") return normalizeFeedbackLedger({});
    throw error;
  }
}

export function normalizeFeedbackLedger(ledger) {
  return {
    version: 1,
    runs: [],
    shards: {},
    notes: [],
    ...(ledger ?? {}),
    runs: [...((ledger ?? {}).runs ?? [])],
    shards: { ...((ledger ?? {}).shards ?? {}) },
    notes: [...((ledger ?? {}).notes ?? [])]
  };
}

export function validateSchedulerModel(manifest, seeds, uiTree, shards, feedbackLedger = {}) {
  const targetTags = new Set(indexCoverageTargets(manifest).keys());
  const seedIds = new Set(seeds.map((seed) => seed.id));
  const nodeIds = new Set((uiTree.nodes ?? []).map((node) => node.id));
  const asyncIds = new Set((manifest.asyncEvents ?? []).map((item) => item.id));
  const invariantIds = new Set((manifest.invariants ?? []).map((item) => item.id));
  const shardIds = new Set();
  const errors = [];
  const warnings = [];

  if (!nodeIds.has(uiTree.root)) errors.push(`ui-tree root ${uiTree.root} is not a declared node`);
  for (const node of uiTree.nodes ?? []) {
    if (node.parent !== null && node.parent !== undefined && !nodeIds.has(node.parent)) {
      errors.push(`ui-tree node ${node.id} references missing parent ${node.parent}`);
    }
    for (const child of node.children ?? []) {
      if (!nodeIds.has(child)) errors.push(`ui-tree node ${node.id} references missing child ${child}`);
    }
    for (const tag of node.tags ?? []) {
      if (!targetTags.has(tag)) warnings.push(`ui-tree node ${node.id} uses non-manifest tag ${tag}`);
    }
  }

  for (const shard of shards) {
    if (!shard.id) errors.push(`${shard.file ?? "shard"} is missing id`);
    if (shardIds.has(shard.id)) errors.push(`duplicate shard id ${shard.id}`);
    shardIds.add(shard.id);
    if (!nodeIds.has(shard.startNode)) errors.push(`${shard.id} references missing startNode ${shard.startNode}`);
    if (!seedIds.has(shard.seed)) errors.push(`${shard.id} references missing seed ${shard.seed}`);
    for (const nodeId of [...(shard.ownedNodes ?? []), ...(shard.allowedSetupNodes ?? [])]) {
      if (!nodeIds.has(nodeId)) errors.push(`${shard.id} references missing node ${nodeId}`);
    }
    for (const tag of shard.ownedCoverage ?? []) {
      if (!targetTags.has(tag)) warnings.push(`${shard.id} owns non-manifest coverage tag ${tag}`);
    }
    for (const eventId of shard.allowedAsyncEvents ?? []) {
      if (!asyncIds.has(eventId)) errors.push(`${shard.id} references missing async event ${eventId}`);
    }
    for (const invariantId of shard.invariants ?? []) {
      if (!invariantIds.has(invariantId)) errors.push(`${shard.id} references missing invariant ${invariantId}`);
    }
  }

  for (const shardId of Object.keys(feedbackLedger.shards ?? {})) {
    if (!shardIds.has(shardId)) warnings.push(`feedback-ledger references unknown shard ${shardId}`);
  }

  return {
    ok: errors.length === 0,
    errorCount: errors.length,
    warningCount: warnings.length,
    errors,
    warnings
  };
}

export function buildShardSchedule(manifest, seeds, uiTree, shards, coverageLedger, feedbackLedger, options = {}) {
  const limit = Number(options.limit ?? 4);
  const namespace = String(options.namespace ?? `uiux-${dateStamp(new Date())}`);
  const requestedShardIds = parseList(options.shards);
  const excludedShardIds = new Set(parseList(options.exclude));
  const seedById = new Map(seeds.map((seed) => [seed.id, seed]));
  const targetTags = indexCoverageTargets(manifest);
  const validatedTags = new Set(coverageLedger.validatedTags ?? []);
  const feedbackCoveredTags = new Set(Object.values(feedbackLedger.shards ?? {})
    .flatMap((item) => item.coveredTags ?? []));
  const treeNodeById = new Map((uiTree.nodes ?? []).map((node) => [node.id, node]));

  const candidates = shards
    .filter((shard) => requestedShardIds.length === 0 || requestedShardIds.includes(shard.id))
    .filter((shard) => !excludedShardIds.has(shard.id))
    .map((shard) => scoreShard(shard, {
      seed: seedById.get(shard.seed),
      targetTags,
      validatedTags,
      feedbackCoveredTags,
      feedback: feedbackLedger.shards?.[shard.id] ?? {},
      runtimeCoverage: coverageLedger.runtimeCoverage?.shards?.[shard.id] ?? {},
      layeredFrontier: layeredFrontierForShard(shard, options.intentManifest),
      treeNode: treeNodeById.get(shard.startNode),
      namespace,
      minIterations: options["min-iterations"],
      maxIterations: options["max-iterations"]
    }))
    .sort((left, right) => {
      if (right.score !== left.score) return right.score - left.score;
      if (right.priority !== left.priority) return right.priority - left.priority;
      return left.shardId.localeCompare(right.shardId);
    });

  const items = candidates.slice(0, limit);
  return {
    version: 1,
    generatedAt: new Date().toISOString(),
    namespace,
    limit,
    totalCandidates: candidates.length,
    selectedCount: items.length,
    selectedShardIds: items.map((item) => item.shardId),
    items
  };
}

export function formatScheduleMarkdown(schedule) {
  const lines = [
    "# Puffer UI/UX Shard Schedule",
    "",
    `Generated: ${schedule.generatedAt}`,
    `Namespace: ${schedule.namespace}`,
    `Selected shards: ${schedule.selectedCount}/${schedule.totalCandidates}`,
    "",
    "## Shards",
    ""
  ];
  for (const item of schedule.items ?? []) {
    lines.push(`### ${item.shardId}`);
    lines.push("");
    lines.push(`- Title: ${item.title}`);
    lines.push(`- Start node: ${item.startNode}`);
    lines.push(`- Seed: ${item.seed}`);
    lines.push(`- Score: ${item.score}`);
    lines.push(`- Priority: ${item.priority}`);
    lines.push(`- Missing owned coverage: ${item.reason.missingOwnedCoverage.join(", ") || "none"}`);
    lines.push(`- Feedback: runs=${item.reason.feedbackRuns}, replaySuccess=${item.reason.replaySuccessRate}, flakeRate=${item.reason.flakeRate}, duplicateRate=${item.reason.duplicateRate}, outOfScope=${item.reason.outOfScopeCount}`);
    lines.push(`- Runtime coverage: states=${item.reason.runtimeStateCount}, edges=${item.reason.runtimeEdgeCount}`);
    lines.push(`- Layered frontier: intents=${item.reason.intentIds.join(", ") || "none"}, races=${item.reason.raceIds.join(", ") || "none"}`);
    lines.push(`- Owned nodes: ${item.ownedNodes.join(", ")}`);
    lines.push(`- Allowed setup nodes: ${item.allowedSetupNodes.join(", ")}`);
    lines.push(`- Allowed async events: ${item.allowedAsyncEvents.join(", ") || "none"}`);
    lines.push(`- Invariants: ${item.invariants.join(", ") || "none"}`);
    lines.push("");
    lines.push("Commands:");
    for (const command of item.commands) lines.push(`- \`${command}\``);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

export function applyReplayFeedback(feedbackLedger, replayReport, options = {}) {
  const shardId = options.shard;
  if (!shardId) throw new Error("--shard is required to record replay feedback");
  const namespace = String(options.namespace ?? replayReport.namespace ?? shardId);
  const summary = replayReport.summary ?? {};
  const results = replayReport.results ?? [];
  const coveredTags = [...new Set(results.flatMap((item) => item.coverage ?? []))].sort();
  const replayedCases = results.map((item) => item.caseId).filter(Boolean);
  const runEntry = {
    runId: `${namespace}:${new Date().toISOString()}`,
    shardId,
    namespace,
    recordedAt: new Date().toISOString(),
    artifactDir: normalizeRepoPath(replayReport.artifactDir ?? ""),
    jsonReport: options.input ?? "",
    total: Number(summary.total ?? results.length ?? 0),
    passed: Number(summary.passed ?? 0),
    stableFailed: Number(summary.stableFailed ?? 0),
    flaky: Number(summary.flaky ?? 0),
    timeout: Number(summary.timeout ?? 0),
    actionableFailures: Number(summary.actionableFailures ?? 0),
    newCandidateFindings: Number(summary.newCandidateFindings ?? 0),
    knownDuplicateFindings: Number(summary.knownDuplicateFindings ?? 0),
    knownDuplicateFailures: Number(summary.knownDuplicateFailures ?? 0),
    outOfScopeFindings: Number(options["out-of-scope"] ?? 0),
    coveredTags,
    replayedCases
  };

  const next = normalizeFeedbackLedger(feedbackLedger);
  next.runs.push(runEntry);
  const previous = next.shards[shardId] ?? emptyShardFeedback();
  const total = Number(previous.total ?? 0) + runEntry.total;
  const passed = Number(previous.passed ?? 0) + runEntry.passed;
  const flaky = Number(previous.flaky ?? 0) + runEntry.flaky;
  const duplicateFailures = Number(previous.knownDuplicateFailures ?? 0) + runEntry.knownDuplicateFailures;
  next.shards[shardId] = {
    ...previous,
    runs: Number(previous.runs ?? 0) + 1,
    lastRunAt: runEntry.recordedAt,
    total,
    passed,
    stableFailed: Number(previous.stableFailed ?? 0) + runEntry.stableFailed,
    flaky,
    timeout: Number(previous.timeout ?? 0) + runEntry.timeout,
    actionableFailures: Number(previous.actionableFailures ?? 0) + runEntry.actionableFailures,
    newCandidateFindings: Number(previous.newCandidateFindings ?? 0) + runEntry.newCandidateFindings,
    knownDuplicateFindings: Number(previous.knownDuplicateFindings ?? 0) + runEntry.knownDuplicateFindings,
    knownDuplicateFailures: duplicateFailures,
    outOfScopeFindings: Number(previous.outOfScopeFindings ?? 0) + runEntry.outOfScopeFindings,
    coveredTags: [...new Set([...(previous.coveredTags ?? []), ...coveredTags])].sort(),
    replayedCases: [...new Set([...(previous.replayedCases ?? []), ...replayedCases])].sort(),
    replaySuccessRate: total === 0 ? 0 : Number((passed / total).toFixed(3)),
    flakeRate: total === 0 ? 0 : Number((flaky / total).toFixed(3)),
    duplicateRate: total === 0 ? 0 : Number((duplicateFailures / total).toFixed(3))
  };
  return next;
}

function normalizeRepoPath(value) {
  const text = String(value ?? "");
  const marker = "apps/puffer-desktop/tests/fuzz/";
  const index = text.indexOf(marker);
  return index < 0 ? text : text.slice(index);
}

function scoreShard(shard, context) {
  const feedback = context.feedback ?? {};
  const seedTier = productTierForSeed(context.seed ?? shard.seed);
  const treeTags = context.treeNode?.tags ?? [];
  const ownedCoverage = [...new Set([...(shard.ownedCoverage ?? []), ...treeTags])].sort();
  const missingOwnedCoverage = ownedCoverage.filter((tag) =>
    context.targetTags.has(tag) &&
    !context.validatedTags.has(tag) &&
    !context.feedbackCoveredTags.has(tag)
  );
  const highPriorityMissing = missingOwnedCoverage.filter((tag) => Number(context.targetTags.get(tag)?.priority ?? 0) >= 8);
  const feedbackRuns = Number(feedback.runs ?? 0);
  const replaySuccessRate = Number(feedback.replaySuccessRate ?? 0);
  const flakeRate = Number(feedback.flakeRate ?? 0);
  const duplicateRate = Number(feedback.duplicateRate ?? 0);
  const outOfScopeCount = Number(feedback.outOfScopeFindings ?? 0);
  const newFindingCount = Number(feedback.newCandidateFindings ?? 0);
  const actionableFailures = Number(feedback.actionableFailures ?? 0);
  const runtimeStateCount = Number(context.runtimeCoverage.stateCount ?? 0);
  const runtimeEdgeCount = Number(context.runtimeCoverage.edgeCount ?? 0);
  const intentIds = context.layeredFrontier.intentIds;
  const raceIds = context.layeredFrontier.raceIds;
  const layeredPriority = context.layeredFrontier.priority;
  const priority = Number(shard.priority ?? context.treeNode?.priority ?? 0);

  let score = priority * 10;
  score += seedTier === 0 ? 30 : seedTier === 1 ? 12 : 0;
  score += missingOwnedCoverage.length * 4;
  score += highPriorityMissing.length * 6;
  score += newFindingCount * 10;
  score += actionableFailures * 4;
  score += layeredPriority * 2;
  score -= feedbackRuns * 3;
  score -= Math.round(flakeRate * 25);
  score -= Math.round(duplicateRate * 20);
  score -= outOfScopeCount * 6;
  score -= Math.min(runtimeEdgeCount, 20);

  const baseIterations = Number(shard.iterations ?? 8);
  const minIterations = Number(context.minIterations ?? 4);
  const maxIterations = Number(context.maxIterations ?? 30);
  const coverageBoost = highPriorityMissing.length >= 3 ? 1.2 : missingOwnedCoverage.length === 0 ? 0.75 : 1;
  const flakePenalty = flakeRate >= 0.34 ? 0.75 : 1;
  const iterations = clamp(Math.round(baseIterations * coverageBoost * flakePenalty), minIterations, maxIterations);
  const replayLimit = clamp(Number(shard.replayLimit ?? 2), 1, 5);
  const namespace = `${context.namespace}-${shard.id}`;
  const runPath = `apps/puffer-desktop/tests/fuzz/.runs/${namespace}/run.json`;
  const reportPath = `apps/puffer-desktop/tests/fuzz/.runs/${namespace}/report.md`;
  const topPath = `apps/puffer-desktop/tests/fuzz/.runs/${namespace}/top.json`;
  const topReportPath = `apps/puffer-desktop/tests/fuzz/.runs/${namespace}/top.md`;
  const replayJsonPath = `apps/puffer-desktop/tests/fuzz/.runs/${namespace}/bounded-replay-report.json`;

  return {
    shardId: shard.id,
    title: shard.title ?? shard.id,
    priority,
    score,
    seed: shard.seed,
    startNode: shard.startNode,
    entrypoint: shard.entrypoint,
    iterations,
    steps: Number(shard.steps ?? 12),
    replayLimit,
    namespace,
    ownedNodes: shard.ownedNodes ?? [],
    allowedSetupNodes: shard.allowedSetupNodes ?? [],
    ownedCoverage,
    allowedAsyncEvents: shard.allowedAsyncEvents ?? [],
    invariants: shard.invariants ?? [],
    reason: {
      seedTier,
      missingOwnedCoverage,
      highPriorityMissing,
      feedbackRuns,
      replaySuccessRate,
      flakeRate,
      duplicateRate,
      outOfScopeCount,
      newFindingCount,
      actionableFailures,
      runtimeStateCount,
      runtimeEdgeCount,
      intentIds,
      raceIds,
      layeredPriority
    },
    artifacts: {
      runPath,
      reportPath,
      topPath,
      topReportPath,
      replayJsonPath
    },
    commands: [
      `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed ${shard.seed} --iterations ${iterations} --steps ${Number(shard.steps ?? 12)} --rng-seed ${namespace} --out ${runPath}`,
      `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input ${runPath} --out ${reportPath}`,
      `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input ${runPath} --shard ${shard.id} --limit ${replayLimit} --out ${topPath} --report-out ${topReportPath}`,
      `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds ${shard.seed} --shard ${shard.id} --limit ${replayLimit} --attempts 3 --timeout 120 --rng-seed ${namespace} --namespace ${namespace} --fail-on-new-finding`,
      `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback --shard ${shard.id} --input ${replayJsonPath}`
    ]
  };
}

function layeredFrontierForShard(shard, intentManifest = {}) {
  const shardId = shard.id;
  const intents = (intentManifest.intents ?? []).filter((intent) =>
    intent.owner === shardId || (intent.secondaryOwners ?? []).includes(shardId)
  );
  const races = (intentManifest.races ?? []).filter((race) =>
    race.owner === shardId || intents.some((intent) => intent.id === race.intent)
  );
  return {
    intentIds: intents.map((intent) => intent.id).sort(),
    raceIds: races.map((race) => race.id).sort(),
    priority: Math.max(
      0,
      ...intents.map((intent) => Number(intent.priority ?? 0)),
      ...races.map((race) => Number(race.priority ?? 0))
    )
  };
}

function emptyShardFeedback() {
  return {
    runs: 0,
    total: 0,
    passed: 0,
    stableFailed: 0,
    flaky: 0,
    timeout: 0,
    actionableFailures: 0,
    newCandidateFindings: 0,
    knownDuplicateFindings: 0,
    knownDuplicateFailures: 0,
    outOfScopeFindings: 0,
    coveredTags: [],
    replayedCases: [],
    replaySuccessRate: 0,
    flakeRate: 0,
    duplicateRate: 0
  };
}

function clamp(value, min, max) {
  return Math.max(min, Math.min(max, value));
}

function parseList(value) {
  if (!value) return [];
  if (Array.isArray(value)) return value;
  return String(value).split(",").map((item) => item.trim()).filter(Boolean);
}

function dateStamp(value) {
  return value.toISOString().replace(/[:.]/g, "-").slice(0, 19);
}
