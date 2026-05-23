import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { createRng, pick, randomInt, weightedPick } from "./seeded-rng.mjs";

const CORE_SEEDS = new Set(["chat-turn-race", "workspace-session-race", "provider-auth-model-race", "modal-focus-race"]);
const SECONDARY_SEEDS = new Set(["files-terminal-race", "browser-tab-race"]);
const CORE_TAG_PATTERNS = [
  /^route:(workspace|agent-detail|chat-composer|new-agent-modal|connect-project-modal|switch-workspace-modal|settings-providers)$/,
  /^control:(workspace\.agent-card|workspace\.connect-project|workspace\.switch-workspace|chat\.|new-agent\.|modal\.|settings\.provider|settings\.default-model)/,
  /^state:(session|auth|daemon)\./,
  /^async:(duplicate-submit|late-success|late-failure|stale-session-event|reconnect|delayed-list-refresh)$/,
  /^invariant:(app-no-crash|active-session-stable|draft-preserved-on-failure|one-request-per-intent|stale-error-scoped|no-permanent-loading|controls-disabled-while-pending|no-cross-provider-model|modal-initial-focus|modal-focus-trapped)$/
];
const SECONDARY_TAG_PATTERNS = [
  /^route:(browser-pane|files-pane|terminal-pane)$/,
  /^control:(browser|files|terminal)\./,
  /^state:(browser|files|terminal)\./,
  /^async:(stale-tab-event|out-of-order-frame)$/,
  /^invariant:active-tab-stable$/
];

export async function readJson(filePath) {
  return JSON.parse(await readFile(filePath, "utf8"));
}

export async function writeText(filePath, contents) {
  await mkdir(path.dirname(filePath), { recursive: true });
  await writeFile(filePath, contents);
}

export async function writeJson(filePath, value) {
  await writeText(filePath, `${JSON.stringify(value, null, 2)}\n`);
}

export async function loadSeeds(seedDir) {
  const files = (await readdir(seedDir))
    .filter((name) => name.endsWith(".json"))
    .sort();
  const seeds = [];
  for (const file of files) {
    seeds.push(await readJson(path.join(seedDir, file)));
  }
  return seeds;
}

export function productTierForSeed(seedOrId) {
  const id = typeof seedOrId === "string" ? seedOrId : seedOrId?.id;
  if (CORE_SEEDS.has(id)) return 0;
  if (SECONDARY_SEEDS.has(id)) return 1;
  return 2;
}

export function productTierForTag(tag, seeds = []) {
  const seedTiers = seeds.map((seed) => productTierForSeed(seed));
  if (seedTiers.includes(0)) return 0;
  if (CORE_TAG_PATTERNS.some((pattern) => pattern.test(tag))) return 0;
  if (seedTiers.includes(1)) return 1;
  if (SECONDARY_TAG_PATTERNS.some((pattern) => pattern.test(tag))) return 1;
  return 2;
}

export function filterSeedsByProfile(seeds, profile = "all") {
  if (!profile || profile === "all" || profile === "extended") return seeds;
  if (profile === "core") return seeds.filter((seed) => productTierForSeed(seed) === 0);
  if (profile === "secondary") return seeds.filter((seed) => productTierForSeed(seed) === 1);
  if (profile === "low-priority") return seeds.filter((seed) => productTierForSeed(seed) >= 2);
  throw new Error(`Unknown fuzz profile: ${profile}`);
}

export async function loadLedger(filePath) {
  try {
    return normalizeLedger(await readJson(filePath));
  } catch (error) {
    if (error && error.code === "ENOENT") {
      return normalizeLedger({ version: 1, validatedTags: [], fixedFindings: [], notes: [] });
    }
    throw error;
  }
}

export function normalizeLedger(ledger) {
  return {
    version: 2,
    validatedTags: [],
    replayedCases: [],
    fixedFindings: [],
    knownBugSignatures: [],
    runtimeCoverage: emptyRuntimeCoverageLedger(),
    metrics: {
      replaySuccessRate: 0,
      duplicateReportRate: 0,
      flakeRate: 0,
      falsePositiveRate: 0,
      p0Open: 0,
      p1Open: 0
    },
    notes: [],
    ...(ledger ?? {}),
    version: Number((ledger ?? {}).version ?? 2),
    runtimeCoverage: normalizeRuntimeCoverageLedger((ledger ?? {}).runtimeCoverage),
    metrics: {
      replaySuccessRate: 0,
      duplicateReportRate: 0,
      flakeRate: 0,
      falsePositiveRate: 0,
      p0Open: 0,
      p1Open: 0,
      ...((ledger ?? {}).metrics ?? {})
    }
  };
}

export function applyReplayCoverageToLedger(ledger, replayReport, options = {}) {
  const next = normalizeLedger(ledger);
  next.version = 2;
  const namespace = String(options.namespace ?? replayReport.namespace ?? "");
  const shard = String(options.shard ?? replayReport.shard ?? "");
  const recordedAt = new Date().toISOString();
  const runtimeCoverage = next.runtimeCoverage;
  const replayedCases = new Set(next.replayedCases ?? []);

  for (const result of replayReport.results ?? []) {
    if (result.caseId) replayedCases.add(result.caseId);
    const status = result.status ?? "unknown";
    const classification = result.classification ?? "unknown";
    const caseId = result.caseId ?? "";
    const coverage = result.runtimeCoverage ?? {};
    for (const state of coverage.states ?? []) {
      if (!state?.stateHash) continue;
      const previous = runtimeCoverage.states[state.stateHash] ?? {};
      runtimeCoverage.states[state.stateHash] = {
        ...previous,
        ...state,
        firstSeenAt: previous.firstSeenAt ?? recordedAt,
        lastSeenAt: recordedAt,
        seenCount: Number(previous.seenCount ?? 0) + 1,
        namespaces: appendUnique(previous.namespaces, namespace),
        shards: appendUnique(previous.shards, shard),
        cases: appendUnique(previous.cases, caseId)
      };
    }
    for (const edge of coverage.edges ?? []) {
      if (!edge?.edgeId) continue;
      const previous = runtimeCoverage.edges[edge.edgeId] ?? {};
      runtimeCoverage.edges[edge.edgeId] = {
        ...previous,
        ...edge,
        firstSeenAt: previous.firstSeenAt ?? recordedAt,
        lastSeenAt: recordedAt,
        seenCount: Number(previous.seenCount ?? 0) + 1,
        namespaces: appendUnique(previous.namespaces, namespace),
        shards: appendUnique(previous.shards, shard),
        cases: appendUnique(previous.cases, caseId),
        statuses: appendUnique(previous.statuses, status),
        classifications: appendUnique(previous.classifications, classification)
      };
    }
    addRuntimeSetValues(runtimeCoverage.asyncEdges, coverage.asyncEdges, { recordedAt, namespace, shard, caseId, status, classification });
    addRuntimeSetValues(runtimeCoverage.asyncInvariantPairs, coverage.asyncInvariantPairs, { recordedAt, namespace, shard, caseId, status, classification });
    addRuntimeSetValues(runtimeCoverage.routeControlStateTriples, coverage.routeControlStateTriples, { recordedAt, namespace, shard, caseId, status, classification });
    addRuntimeSetValues(runtimeCoverage.invariantObservations, coverage.invariantObservations, { recordedAt, namespace, shard, caseId, status, classification });
  }

  if (shard) {
    const previousShard = runtimeCoverage.shards[shard] ?? {};
    runtimeCoverage.shards[shard] = {
      ...previousShard,
      lastRunAt: recordedAt,
      namespaces: appendUnique(previousShard.namespaces, namespace),
      replayedCases: [...new Set([...(previousShard.replayedCases ?? []), ...[...replayedCases]])].sort(),
      stateCount: Object.values(runtimeCoverage.states).filter((item) => (item.shards ?? []).includes(shard)).length,
      edgeCount: Object.values(runtimeCoverage.edges).filter((item) => (item.shards ?? []).includes(shard)).length
    };
  }

  next.replayedCases = [...replayedCases].sort();
  next.runtimeCoverage = runtimeCoverage;
  return next;
}

function emptyRuntimeCoverageLedger() {
  return {
    states: {},
    edges: {},
    asyncEdges: {},
    asyncInvariantPairs: {},
    routeControlStateTriples: {},
    invariantObservations: {},
    shards: {}
  };
}

function normalizeRuntimeCoverageLedger(value) {
  return {
    ...emptyRuntimeCoverageLedger(),
    ...(value ?? {}),
    states: { ...((value ?? {}).states ?? {}) },
    edges: { ...((value ?? {}).edges ?? {}) },
    asyncEdges: { ...((value ?? {}).asyncEdges ?? {}) },
    asyncInvariantPairs: { ...((value ?? {}).asyncInvariantPairs ?? {}) },
    routeControlStateTriples: { ...((value ?? {}).routeControlStateTriples ?? {}) },
    invariantObservations: { ...((value ?? {}).invariantObservations ?? {}) },
    shards: { ...((value ?? {}).shards ?? {}) }
  };
}

function addRuntimeSetValues(target, values = [], context) {
  for (const value of values ?? []) {
    if (!value) continue;
    const previous = target[value] ?? {};
    target[value] = {
      ...previous,
      firstSeenAt: previous.firstSeenAt ?? context.recordedAt,
      lastSeenAt: context.recordedAt,
      seenCount: Number(previous.seenCount ?? 0) + 1,
      namespaces: appendUnique(previous.namespaces, context.namespace),
      shards: appendUnique(previous.shards, context.shard),
      cases: appendUnique(previous.cases, context.caseId),
      statuses: appendUnique(previous.statuses, context.status),
      classifications: appendUnique(previous.classifications, context.classification)
    };
  }
}

function appendUnique(values = [], value) {
  return [...new Set([...values, value].filter(Boolean))].sort();
}

export function indexCoverageTargets(manifest) {
  const dimensions = [
    ["routes", "route"],
    ["controls", "control"],
    ["states", "state"],
    ["asyncEvents", "async"],
    ["invariants", "invariant"]
  ];
  const targets = new Map();
  for (const [key, prefix] of dimensions) {
    for (const item of manifest[key] ?? []) {
      targets.set(`${prefix}:${item.id}`, {
        ...item,
        dimension: prefix,
        tag: `${prefix}:${item.id}`
      });
    }
  }
  return targets;
}

export function collectCoverageTags(step) {
  return new Set([
    ...(step.coverage ?? []),
    ...(step.assertions ?? []).map((id) => `invariant:${id}`)
  ]);
}

export function materializeParams(template, rng) {
  if (template === null || typeof template !== "object") return template;
  if (Array.isArray(template)) return template.map((item) => materializeParams(item, rng));
  if (Array.isArray(template.oneOf)) return materializeParams(pick(rng, template.oneOf), rng);
  if (Array.isArray(template.intRange) && template.intRange.length === 2) {
    return randomInt(rng, Number(template.intRange[0]), Number(template.intRange[1]));
  }
  if (Array.isArray(template.repeat) && template.repeat.length === 2) {
    const [item, range] = template.repeat;
    const count = Array.isArray(range) ? randomInt(rng, Number(range[0]), Number(range[1])) : Number(range);
    return Array.from({ length: count }, () => materializeParams(item, rng));
  }
  const result = {};
  for (const [key, value] of Object.entries(template)) {
    result[key] = materializeParams(value, rng);
  }
  return result;
}

export function generateCase(seed, options) {
  const iteration = options.iteration ?? 0;
  const maxSteps = options.steps ?? seed.defaultSteps ?? 12;
  const rngSeed = `${options.rngSeed ?? "puffer-fuzz"}:${seed.id}:${iteration}`;
  const rng = createRng(rngSeed);
  const generatedSteps = [];
  const actionUseCounts = new Map();
  const resources = new Set();

  for (const setup of seed.setup ?? []) {
    const step = {
      ...setup,
      phase: "setup",
      params: materializeParams(setup.params ?? {}, rng)
    };
    generatedSteps.push(step);
    for (const tag of step.coverage ?? []) resources.add(tag);
    for (const tag of step.produces ?? []) resources.add(tag);
  }

  for (let index = 0; index < maxSteps; index += 1) {
    const candidates = (seed.actions ?? []).filter((action) => {
      const count = actionUseCounts.get(action.id) ?? 0;
      if (action.maxPerCase && count >= action.maxPerCase) return false;
      return (action.requires ?? []).every((tag) => resources.has(tag)) &&
        (action.blocks ?? []).every((tag) => !resources.has(tag));
    });
    if (candidates.length === 0) break;
    const action = weightedPick(rng, candidates);
    actionUseCounts.set(action.id, (actionUseCounts.get(action.id) ?? 0) + 1);
    const step = {
      id: `${action.id}-${index + 1}`,
      action: action.id,
      kind: action.kind,
      target: action.target,
      phase: "fuzz",
      params: materializeParams(action.params ?? {}, rng),
      coverage: action.coverage ?? [],
      requires: action.requires ?? [],
      blocks: action.blocks ?? [],
      consumes: action.consumes ?? [],
      invalidates: action.invalidates ?? [],
      produces: action.produces ?? [],
      expectedDaemon: action.expectedDaemon ?? null,
      note: action.note ?? null
    };
    generatedSteps.push(step);
    applyResourceTransitions(resources, step);
  }

  appendAutoSettleSteps(seed, resources, generatedSteps, rng);

  for (const invariant of seed.invariants ?? []) {
    generatedSteps.push({
      id: `assert-${invariant}`,
      action: "assert",
      kind: "assertion",
      target: invariant,
      phase: "assert",
      assertions: [invariant],
      coverage: [`invariant:${invariant}`]
    });
  }

  const coverage = summarizeCaseCoverage(generatedSteps);
  return {
    caseId: `${seed.id}-${String(iteration + 1).padStart(4, "0")}`,
    seedId: seed.id,
    title: seed.title,
    rngSeed,
    focus: seed.focus,
    severityTarget: seed.severityTarget,
    steps: generatedSteps,
    coverage: [...coverage].sort()
  };
}

function appendAutoSettleSteps(seed, resources, generatedSteps, rng) {
  const cleanupOrder = [
    ["resource:permission.pending", "answer-permission", { answer: "approve" }],
    ["resource:question.pending", "answer-question", { answer: "yes" }],
    ["resource:turn.canceling", "settle-canceled-turn", {}],
    ["resource:turn.running", "complete-turn", {}]
  ];
  for (const [resource, actionId, defaultParams] of cleanupOrder) {
    if (!resources.has(resource)) continue;
    const action = (seed.actions ?? []).find((item) => item.id === actionId);
    if (!action) continue;
    const index = generatedSteps.length + 1;
    const step = {
      id: `settle-${action.id}-${index}`,
      action: action.id,
      kind: action.kind,
      target: action.target,
      phase: "settle",
      params: { ...defaultParams, ...materializeParams(action.params ?? {}, rng) },
      coverage: action.coverage ?? [],
      requires: action.requires ?? [],
      blocks: action.blocks ?? [],
      consumes: action.consumes ?? [],
      invalidates: action.invalidates ?? [],
      produces: action.produces ?? [],
      expectedDaemon: action.expectedDaemon ?? null,
      note: "Auto-added before invariant checks to avoid asserting against intentionally pending UI."
    };
    generatedSteps.push(step);
    applyResourceTransitions(resources, step);
  }
}

function applyResourceTransitions(resources, step) {
  for (const tag of step.consumes ?? []) resources.delete(tag);
  for (const tag of step.invalidates ?? []) resources.delete(tag);
  for (const tag of step.coverage ?? []) resources.add(tag);
  for (const tag of step.produces ?? []) resources.add(tag);
}

export function summarizeCaseCoverage(steps) {
  const coverage = new Set();
  for (const step of steps) {
    for (const tag of collectCoverageTags(step)) coverage.add(tag);
  }
  return coverage;
}

export function summarizeRun(manifest, cases) {
  const targets = indexCoverageTargets(manifest);
  const covered = new Set();
  for (const item of cases) {
    for (const tag of item.coverage ?? []) covered.add(tag);
  }
  const byDimension = {};
  for (const target of targets.values()) {
    const bucket = byDimension[target.dimension] ?? {
      covered: 0,
      total: 0,
      coveredTags: [],
      missingTags: []
    };
    bucket.total += 1;
    if (covered.has(target.tag)) {
      bucket.covered += 1;
      bucket.coveredTags.push(target.tag);
    } else {
      bucket.missingTags.push(target.tag);
    }
    byDimension[target.dimension] = bucket;
  }
  for (const bucket of Object.values(byDimension)) {
    bucket.coveragePct = bucket.total === 0 ? 100 : Number(((bucket.covered / bucket.total) * 100).toFixed(1));
    bucket.coveredTags.sort();
    bucket.missingTags.sort();
  }
  return {
    caseCount: cases.length,
    generatedAt: new Date().toISOString(),
    byDimension,
    coveredTags: [...covered].sort()
  };
}

export function buildRun(manifest, seeds, options) {
  const cases = [];
  const iterations = Number(options.iterations ?? 8);
  const steps = Number(options.steps ?? 12);
  for (const seed of seeds) {
    const seedIterations = Number(options.seedIterations?.[seed.id] ?? iterations);
    for (let iteration = 0; iteration < seedIterations; iteration += 1) {
      cases.push(generateCase(seed, {
        iteration,
        steps,
        rngSeed: options.rngSeed
      }));
    }
  }
  return {
    version: 1,
    manifestVersion: manifest.version,
    generatedAt: new Date().toISOString(),
    options: {
      iterations,
      steps,
      rngSeed: options.rngSeed ?? "puffer-fuzz",
      profile: options.profile ?? "all"
    },
    cases,
    summary: summarizeRun(manifest, cases)
  };
}

export function buildPlan(manifest, seeds, options = {}) {
  const limit = Number(options.limit ?? 20);
  const validatedTags = new Set(options.ledger?.validatedTags ?? []);
  const targets = [...indexCoverageTargets(manifest).values()]
    .sort((left, right) => {
      const leftTier = productTierForTag(left.tag, seeds.filter((seed) => seedCoversTag(seed, left.tag)));
      const rightTier = productTierForTag(right.tag, seeds.filter((seed) => seedCoversTag(seed, right.tag)));
      if (leftTier !== rightTier) return leftTier - rightTier;
      return Number(right.priority ?? 0) - Number(left.priority ?? 0);
    });
  const seedRows = seeds.map((seed) => {
    const tags = new Set();
    for (const step of [...(seed.setup ?? []), ...(seed.actions ?? [])]) {
      for (const tag of step.coverage ?? []) tags.add(tag);
    }
    const productTier = productTierForSeed(seed);
    return {
      seed,
      productTier,
      targetCount: [...tags].filter((tag) => targets.some((target) => target.tag === tag)).length,
      highPriorityHits: [...tags].filter((tag) => {
        const target = targets.find((item) => item.tag === tag);
        return target && Number(target.priority ?? 0) >= 8;
      }).length,
      unvalidatedHighPriorityHits: [...tags].filter((tag) => {
        const target = targets.find((item) => item.tag === tag);
        return target && Number(target.priority ?? 0) >= 8 && !validatedTags.has(tag);
      }).length,
      tags: [...tags].sort()
    };
  }).sort((left, right) => {
    if (left.productTier !== right.productTier) return left.productTier - right.productTier;
    if (right.unvalidatedHighPriorityHits !== left.unvalidatedHighPriorityHits) {
      return right.unvalidatedHighPriorityHits - left.unvalidatedHighPriorityHits;
    }
    if (right.highPriorityHits !== left.highPriorityHits) return right.highPriorityHits - left.highPriorityHits;
    return right.targetCount - left.targetCount;
  });

  return {
    generatedAt: new Date().toISOString(),
    profile: options.profile ?? "all",
    topTargets: targets.slice(0, limit),
    seeds: seedRows
  };
}

export function formatPlanMarkdown(plan) {
  const lines = [
    "# Puffer UI/UX Fuzz Coverage Plan",
    "",
    `Generated: ${plan.generatedAt}`,
    `Profile: ${plan.profile ?? "all"}`,
    "",
    "## Priority Coverage Targets",
    ""
  ];
  for (const target of plan.topTargets) {
    const matchingSeeds = plan.seeds.map((row) => row.seed).filter((seed) => seedCoversTag(seed, target.tag));
    lines.push(`- ${target.tag} (tier ${productTierForTag(target.tag, matchingSeeds)}, priority ${target.priority ?? 0}): ${target.description}`);
  }
  lines.push("", "## Seed Queue", "");
  for (const row of plan.seeds) {
    lines.push(`### ${row.seed.id}`);
    lines.push("");
    lines.push(`- Focus: ${row.seed.focus}`);
    lines.push(`- Product tier: ${row.productTier}`);
    lines.push(`- Severity target: ${row.seed.severityTarget}`);
    lines.push(`- High-priority coverage hits: ${row.highPriorityHits}`);
    lines.push(`- Unvalidated high-priority hits: ${row.unvalidatedHighPriorityHits}`);
    const profileArg = plan.profile && plan.profile !== "all" ? ` --profile ${plan.profile}` : "";
    lines.push(`- Command: \`node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed ${row.seed.id} --iterations ${row.seed.defaultIterations ?? 8} --steps ${row.seed.defaultSteps ?? 12}${profileArg} --out apps/puffer-desktop/tests/fuzz/.runs/manual/${row.seed.id}.json\``);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

export function formatReportMarkdown(run) {
  const lines = [
    "# Puffer UI/UX Fuzz Run Report",
    "",
    `Generated: ${run.generatedAt}`,
    `Cases: ${run.cases.length}`,
    "",
    "## Coverage",
    ""
  ];
  for (const [dimension, bucket] of Object.entries(run.summary.byDimension ?? {})) {
    lines.push(`- ${dimension}: ${bucket.covered}/${bucket.total} (${bucket.coveragePct}%)`);
  }
  lines.push("", "## Top Cases", "");
  const rankedCases = [...run.cases].sort((left, right) => scoreCase(right) - scoreCase(left));
  for (const item of rankedCases.slice(0, 20)) {
    lines.push(`### ${item.caseId}`);
    lines.push("");
    lines.push(`- Focus: ${item.focus}`);
    lines.push(`- Product tier: ${productTierForSeed(item.seedId)}`);
    lines.push(`- Severity target: ${item.severityTarget}`);
    lines.push(`- Replay value score: ${scoreCase(item)}`);
    lines.push(`- Coverage tags: ${item.coverage.join(", ")}`);
    lines.push(`- Steps: ${item.steps.map((step) => step.action).join(" -> ")}`);
    const params = item.steps
      .filter((step) => step.phase === "fuzz" && step.params && Object.keys(step.params).length > 0)
      .map((step) => `${step.action}=${JSON.stringify(step.params)}`);
    if (params.length > 0) lines.push(`- Params: ${params.join("; ")}`);
    lines.push("");
  }
  lines.push("## Missing Coverage", "");
  for (const [dimension, bucket] of Object.entries(run.summary.byDimension ?? {})) {
    if ((bucket.missingTags ?? []).length === 0) continue;
    lines.push(`### ${dimension}`);
    lines.push("");
    for (const tag of bucket.missingTags.slice(0, 50)) lines.push(`- ${tag}`);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

export function selectTopCases(run, options = {}) {
  const limit = Number(options.limit ?? 5);
  const sorted = [...(run.cases ?? [])].sort((left, right) => {
    const scoreDelta = scoreCase(right) - scoreCase(left);
    if (scoreDelta !== 0) return scoreDelta;
    return String(left.caseId).localeCompare(String(right.caseId));
  });
  const selected = options.diversity === false ? sorted.slice(0, limit) : selectDiverseCases(sorted, limit);
  const cases = selected.map((item) => ({
      caseId: item.caseId,
      seedId: item.seedId,
      score: scoreCase(item),
      diversityKey: caseDiversityKey(item),
      focus: item.focus,
      severityTarget: item.severityTarget,
      coverage: item.coverage,
      steps: item.steps.map((step) => ({
        phase: step.phase,
        action: step.action,
        kind: step.kind,
        target: step.target,
        params: step.params ?? {}
      }))
    }));
  return {
    version: 1,
    generatedAt: new Date().toISOString(),
    selectionMode: options.diversity === false ? "score" : "diverse-score",
    sourceGeneratedAt: run.generatedAt,
    caseCount: run.cases?.length ?? 0,
    selectedCount: cases.length,
    summary: run.summary,
    cases
  };
}

export function formatTopCasesMarkdown(selection) {
  const lines = [
    "# Puffer UI/UX Fuzz Top Cases",
    "",
    `Generated: ${selection.generatedAt}`,
    `Source cases: ${selection.caseCount}`,
    `Selected cases: ${selection.selectedCount}`,
    "",
    "## Coverage",
    ""
  ];
  for (const [dimension, bucket] of Object.entries(selection.summary?.byDimension ?? {})) {
    lines.push(`- ${dimension}: ${bucket.covered}/${bucket.total} (${bucket.coveragePct}%)`);
  }
  lines.push("", "## Cases", "");
  for (const item of selection.cases ?? []) {
    lines.push(`### ${item.caseId}`);
    lines.push("");
    lines.push(`- Seed: ${item.seedId}`);
    lines.push(`- Replay value score: ${item.score}`);
    lines.push(`- Diversity key: ${item.diversityKey}`);
    lines.push(`- Focus: ${item.focus}`);
    lines.push(`- Severity target: ${item.severityTarget}`);
    lines.push(`- Coverage tags: ${item.coverage.join(", ")}`);
    lines.push(`- Steps: ${item.steps.map((step) => step.action).join(" -> ")}`);
    const params = item.steps
      .filter((step) => step.phase === "fuzz" && step.params && Object.keys(step.params).length > 0)
      .map((step) => `${step.action}=${JSON.stringify(step.params)}`);
    if (params.length > 0) lines.push(`- Params: ${params.join("; ")}`);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

function selectDiverseCases(sortedCases, limit) {
  const selected = [];
  const usedRoutes = new Set();
  for (const item of sortedCases) {
    const routeKey = `${item.seedId ?? "seed:none"}|${casePrimaryRoute(item)}`;
    if (usedRoutes.has(routeKey)) continue;
    selected.push(item);
    usedRoutes.add(routeKey);
    if (selected.length >= limit) return selected;
  }
  const usedKeys = new Set();
  for (const item of sortedCases) {
    if (selected.includes(item)) continue;
    const key = caseDiversityKey(item);
    if (usedKeys.has(key)) continue;
    selected.push(item);
    usedKeys.add(key);
    if (selected.length >= limit) return selected;
  }
  for (const item of sortedCases) {
    if (selected.includes(item)) continue;
    selected.push(item);
    if (selected.length >= limit) return selected;
  }
  return selected;
}

function casePrimaryRoute(item) {
  const routeTags = [...(item.coverage ?? [])]
    .filter((tag) => String(tag).startsWith("route:"))
    .sort();
  return routeTags.find((tag) => !["route:workspace", "route:agent-detail"].includes(tag)) ?? routeTags[0] ?? "route:none";
}

export function caseDiversityKey(item) {
  const tags = item.coverage ?? [];
  const primaryInvariant = firstTag(tags, "invariant:") ?? "invariant:none";
  const primaryAsync = firstTag(tags, "async:") ?? "async:none";
  const primaryRoute = casePrimaryRoute(item);
  const primaryControl = firstTag(tags, "control:") ?? "control:none";
  const actionShape = (item.steps ?? [])
    .filter((step) => step.phase === "fuzz")
    .map((step) => step.action)
    .slice(0, 5)
    .join(">");
  return [
    item.seedId ?? "seed:none",
    primaryRoute,
    primaryControl,
    primaryAsync,
    primaryInvariant,
    actionShape || "actions:none"
  ].join("|");
}

function firstTag(tags, prefix) {
  return [...tags].filter((tag) => String(tag).startsWith(prefix)).sort()[0];
}

export function scoreCase(item) {
  const tags = new Set(item.coverage ?? []);
  const tier = productTierForSeed(item.seedId);
  let score = tier === 0 ? 30 : tier === 1 ? 10 : 0;
  for (const tag of tags) {
    if (tag.startsWith("async:")) score += 5;
    if (tag.startsWith("invariant:")) score += 2;
    if (tag.startsWith("control:")) score += 1;
  }
  for (const tag of ["async:late-failure", "async:late-success", "async:stale-session-event", "async:stale-tab-event", "async:duplicate-submit", "async:reconnect", "async:dropped-response"]) {
    if (tags.has(tag)) score += 5;
  }
  return score;
}

export function formatAgentTask(seed, options = {}) {
  const iterations = Number(options.iterations ?? seed.defaultIterations ?? 8);
  const steps = Number(options.steps ?? seed.defaultSteps ?? 12);
  return `# Agent Fuzz Task: ${seed.id}

Use the Puffer interaction fuzz framework to investigate reproducible UI/UX bugs.

Scope:
- Seed: ${seed.id}
- Focus: ${seed.focus}
- Severity target: ${seed.severityTarget}
- Primary routes: ${(seed.primaryRoutes ?? []).join(", ")}

Workflow:
1. Run \`node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed ${seed.id} --iterations ${iterations} --steps ${steps} --out apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/run.json\`.
2. Run \`node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/run.json --out apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/report.md\`.
3. Run \`node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/run.json --limit 5 --out apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/top.json --report-out apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/top.md\`.
4. Replay selected cases with \`node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds ${seed.id} --limit 5 --attempts 3 --namespace ${seed.id} --fail-on-new-finding\`.
5. If it reproduces a product bug, shrink the steps to the smallest stable sequence.
6. Record the finding under \`apps/puffer-desktop/tests/fuzz/.runs/${seed.id}/findings.md\` with trigger, expected, actual, impact, and regression target.
7. Do not patch product code from this fuzz task; product fixes and deterministic regressions are a separate follow-up.

Acceptance:
- Do not count fixture-only or environment-only failures.
- Do not count visual-only polish unless it blocks input, navigation, save, submit, cancel, or recovery.
- Every accepted bug must be triggered by click, type, keyboard, resize, daemon event, reconnect, or response ordering.
- Treat \`Known duplicate: yes\` or \`knownDuplicate: true\` as duplicate evidence,
  not as a fresh actionable finding.
`;
}

export function validateFramework(manifest, seeds, adapter, fakeDaemonSource = "") {
  const targets = indexCoverageTargets(manifest);
  const targetTags = new Set(targets.keys());
  const adapterActions = new Set((adapter.actions ?? []).map((item) => item.id));
  const knownDaemonMethods = new Set(
    [...fakeDaemonSource.matchAll(/case "([^"]+)":/g)].map((match) => match[1])
  );
  const knownEventPatterns = new Set((adapter.eventPatterns ?? []).map((item) => item.id));
  const errors = [];
  const warnings = [];
  const covered = new Set();
  const generatedActions = new Set();

  for (const seed of seeds) {
    for (const step of [...(seed.setup ?? []), ...(seed.actions ?? [])]) {
      const actionId = step.action ?? step.id;
      generatedActions.add(actionId);
      if (!adapterActions.has(actionId)) {
        errors.push(`${seed.id}:${actionId} has no adapter action mapping`);
      }
      for (const tag of step.coverage ?? []) {
        covered.add(tag);
        if (!targetTags.has(tag)) errors.push(`${seed.id}:${actionId} uses unknown coverage tag ${tag}`);
      }
      if (step.expectedDaemon && knownDaemonMethods.size > 0 && !knownDaemonMethods.has(step.expectedDaemon)) {
        errors.push(`${seed.id}:${actionId} expects unknown FakeDaemon method ${step.expectedDaemon}`);
      }
      if (step.kind === "daemon-event" && step.target && knownEventPatterns.size > 0) {
        const supported = [...knownEventPatterns].some((pattern) => eventPatternMatches(pattern, step.target));
        if (!supported) warnings.push(`${seed.id}:${actionId} uses event target ${step.target} without an adapter event pattern`);
      }
    }
    for (const invariant of seed.invariants ?? []) covered.add(`invariant:${invariant}`);
  }

  for (const action of adapterActions) {
    if (!generatedActions.has(action)) warnings.push(`adapter action ${action} is not generated by any seed`);
  }

  for (const tag of targetTags) {
    if (!covered.has(tag)) warnings.push(`manifest tag ${tag} is not covered by any seed`);
  }

  return {
    ok: errors.length === 0,
    errorCount: errors.length,
    warningCount: warnings.length,
    errors,
    warnings
  };
}

function eventPatternMatches(pattern, target) {
  if (pattern === target) return true;
  const regex = new RegExp(`^${pattern.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replaceAll("<session>", "[^:]+").replaceAll("<tab>", "[^:]+").replaceAll("<pty>", "[^:]+")}$`);
  return regex.test(target);
}

function seedCoversTag(seed, tag) {
  for (const step of [...(seed.setup ?? []), ...(seed.actions ?? [])]) {
    if ((step.coverage ?? []).includes(tag)) return true;
  }
  if (tag.startsWith("invariant:")) {
    return (seed.invariants ?? []).includes(tag.slice("invariant:".length));
  }
  return false;
}
