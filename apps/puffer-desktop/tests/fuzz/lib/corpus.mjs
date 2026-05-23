import { readJson, summarizeRun, writeJson, writeText } from "./fuzz-core.mjs";
import { createRng } from "./seeded-rng.mjs";

export async function loadCorpus(filePath) {
  try {
    return normalizeCorpus(await readJson(filePath));
  } catch (error) {
    if (error && error.code === "ENOENT") return normalizeCorpus({});
    throw error;
  }
}

export async function writeCorpus(filePath, corpus) {
  await writeJson(filePath, normalizeCorpus(corpus));
}

export function normalizeCorpus(corpus) {
  return {
    version: 1,
    generatedAt: corpus?.generatedAt ?? new Date().toISOString(),
    entries: [...(corpus?.entries ?? [])],
    notes: [...(corpus?.notes ?? [])]
  };
}

export function addReplayReportToCorpus(corpus, replayReport, options = {}) {
  const next = normalizeCorpus(corpus);
  const existing = new Set(next.entries.map((entry) => entry.entryId));
  const namespace = String(options.namespace ?? replayReport.namespace ?? "");
  const shard = String(options.shard ?? replayReport.shard ?? "");
  for (const result of replayReport.results ?? []) {
    const reasons = interestingReasons(result);
    if (reasons.length === 0) continue;
    const entry = corpusEntryFromReplayResult(result, {
      namespace,
      shard,
      artifactDir: replayReport.artifactDir ?? "",
      reasons
    });
    if (existing.has(entry.entryId)) continue;
    existing.add(entry.entryId);
    next.entries.push(entry);
  }
  next.generatedAt = new Date().toISOString();
  next.entries.sort((left, right) => {
    if (right.score !== left.score) return right.score - left.score;
    return left.entryId.localeCompare(right.entryId);
  });
  return next;
}

export function buildRunFromCorpus(manifest, corpus, options = {}) {
  const normalized = normalizeCorpus(corpus);
  const limit = Math.max(1, Number(options.limit ?? (normalized.entries.length || 1)));
  const rng = createRng(String(options.rngSeed ?? "puffer-corpus"));
  const cases = normalized.entries.slice(0, limit).map((entry, index) =>
    caseFromCorpusEntry(entry, { index, rng, mutate: options.mutate !== false })
  );
  return {
    version: 1,
    manifestVersion: manifest.version,
    generatedAt: new Date().toISOString(),
    options: {
      mode: "corpus",
      limit,
      mutate: options.mutate !== false,
      rngSeed: options.rngSeed ?? "puffer-corpus"
    },
    cases,
    summary: summarizeRun(manifest, cases)
  };
}

export function formatCorpusMarkdown(corpus) {
  const normalized = normalizeCorpus(corpus);
  const summary = summarizeCorpus(normalized);
  const lines = [
    "# Puffer UI/UX Fuzz Corpus",
    "",
    `Generated: ${normalized.generatedAt}`,
    `Entries: ${normalized.entries.length}`,
    "",
    "## Summary",
    "",
    `- States: ${summary.stateCount}`,
    `- Edges: ${summary.edgeCount}`,
    `- Async-invariant pairs: ${summary.asyncInvariantPairCount}`,
    `- Stable failures: ${summary.stableFailureCount}`,
    `- Flaky entries: ${summary.flakyCount}`,
    `- Harness/precondition entries: ${summary.harnessCount}`,
    "",
    "## Entries",
    ""
  ];
  for (const entry of normalized.entries) {
    lines.push(`### ${entry.entryId}`, "");
    lines.push(`- Source: ${entry.namespace || "unknown"} / ${entry.caseId}`);
    lines.push(`- Seed: ${entry.seedId || "unknown"}`);
    lines.push(`- Shard: ${entry.shard || "unknown"}`);
    lines.push(`- Status: ${entry.status}`);
    lines.push(`- Classification: ${entry.classification}`);
    lines.push(`- Score: ${entry.score}`);
    lines.push(`- Keep reasons: ${entry.keepReasons.join(", ")}`);
    lines.push(`- Runtime states: ${entry.runtime.stateCount}`);
    lines.push(`- Runtime edges: ${entry.runtime.edgeCount}`);
    lines.push(`- Coverage: ${entry.coverage.join(", ") || "none"}`);
    lines.push(`- Steps: ${entry.steps.map((step) => step.action).join(" -> ") || "none"}`);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

export async function writeCorpusMarkdown(filePath, corpus) {
  await writeText(filePath, formatCorpusMarkdown(corpus));
}

export function summarizeCorpus(corpus) {
  const states = new Set();
  const edges = new Set();
  const asyncInvariantPairs = new Set();
  let stableFailureCount = 0;
  let flakyCount = 0;
  let harnessCount = 0;
  for (const entry of normalizeCorpus(corpus).entries) {
    for (const state of entry.runtime.states ?? []) states.add(state);
    for (const edge of entry.runtime.edges ?? []) edges.add(edge);
    for (const pair of entry.runtime.asyncInvariantPairs ?? []) asyncInvariantPairs.add(pair);
    if (entry.status === "stable-failed") stableFailureCount += 1;
    if (entry.status === "flaky") flakyCount += 1;
    if (String(entry.classification).startsWith("harness-precondition")) harnessCount += 1;
  }
  return {
    stateCount: states.size,
    edgeCount: edges.size,
    asyncInvariantPairCount: asyncInvariantPairs.size,
    stableFailureCount,
    flakyCount,
    harnessCount
  };
}

function corpusEntryFromReplayResult(result, context) {
  const runtime = summarizeRuntime(result.runtimeCoverage ?? {});
  const entryId = [
    "corpus",
    result.seed ?? "unknown-seed",
    result.caseId ?? "unknown-case",
    runtime.edgeFingerprint || result.failureSignature || result.classification || "no-signal"
  ].map(sanitize).join("-");
  return {
    entryId,
    addedAt: new Date().toISOString(),
    namespace: context.namespace,
    shard: context.shard,
    artifactDir: context.artifactDir,
    seedId: result.seed ?? "",
    caseId: result.caseId ?? "",
    status: result.status ?? "unknown",
    classification: result.classification ?? "unknown",
    failureSignature: result.failureSignature ?? "",
    keepReasons: context.reasons,
    score: scoreReasons(context.reasons),
    coverage: [...(result.coverage ?? [])].sort(),
    runtime,
    steps: normalizeReplaySteps(result)
  };
}

function normalizeReplaySteps(result) {
  if (Array.isArray(result.stepDetails) && result.stepDetails.length > 0) {
    return result.stepDetails.map((step) => ({
      action: step.action,
      phase: step.phase,
      kind: step.kind,
      target: step.target,
      params: step.params ?? {},
      coverage: step.coverage ?? []
    }));
  }
  return (result.steps ?? []).map((action) => ({
    action,
    phase: action === "assert" ? "assert" : undefined,
    params: {},
    coverage: []
  }));
}

function interestingReasons(result) {
  const reasons = [];
  const runtime = result.runtimeCoverage ?? {};
  if ((runtime.states ?? []).length > 0) reasons.push("runtime-state");
  if ((runtime.edges ?? []).length > 0) reasons.push("runtime-edge");
  if ((runtime.asyncInvariantPairs ?? []).length > 0) reasons.push("async-invariant-pair");
  if (result.status === "stable-failed") reasons.push("stable-failure");
  if (result.status === "flaky") reasons.push("flaky-signal");
  if (String(result.classification ?? "").startsWith("harness-precondition")) reasons.push("harness-precondition");
  if (String(result.classification ?? "").startsWith("product-candidate:")) reasons.push("product-candidate");
  if (String(result.classification ?? "").startsWith("needs-manual-triage")) reasons.push("manual-triage");
  return reasons;
}

function summarizeRuntime(runtimeCoverage) {
  const states = (runtimeCoverage.states ?? []).map((state) => state.stateHash).filter(Boolean).sort();
  const edges = (runtimeCoverage.edges ?? []).map((edge) => edge.edgeId).filter(Boolean).sort();
  const asyncInvariantPairs = [...(runtimeCoverage.asyncInvariantPairs ?? [])].sort();
  const routeControlStateTriples = [...(runtimeCoverage.routeControlStateTriples ?? [])].sort();
  return {
    stateCount: states.length,
    edgeCount: edges.length,
    asyncInvariantPairCount: asyncInvariantPairs.length,
    routeControlStateTripleCount: routeControlStateTriples.length,
    states,
    edges,
    asyncInvariantPairs,
    routeControlStateTriples,
    edgeFingerprint: edges.slice(0, 3).join("_")
  };
}

function caseFromCorpusEntry(entry, { index, rng, mutate }) {
  const originalSteps = entry.steps.map((step, stepIndex) => ({
    id: `${step.action}-${stepIndex + 1}`,
    action: step.action,
    kind: step.kind ?? "corpus",
    target: step.target ?? step.action,
    phase: step.phase ?? (step.action === "assert" ? "assert" : stepIndex === 0 ? "setup" : "fuzz"),
    params: step.params ?? {},
    coverage: step.coverage ?? []
  }));
  const mutation = mutate ? mutateSteps(originalSteps, rng) : { steps: originalSteps, mutation: "none" };
  const coverage = [...new Set([...(entry.coverage ?? []), `corpus:${entry.entryId}`, `mutation:${mutation.mutation}`])].sort();
  return {
    caseId: `${entry.seedId || "corpus"}-${String(index + 1).padStart(4, "0")}`,
    seedId: entry.seedId || "corpus",
    title: `Corpus replay for ${entry.caseId}`,
    rngSeed: `corpus:${entry.entryId}:${index}`,
    focus: `Corpus entry ${entry.entryId}`,
    severityTarget: entry.classification?.startsWith("product-candidate:") ? "P1" : "P2",
    corpusEntryId: entry.entryId,
    mutation: mutation.mutation,
    mutationIsIntentionalNegative: mutation.intentionalNegative,
    steps: mutation.steps,
    coverage
  };
}

function mutateSteps(steps, rng) {
  if (steps.length === 0) return { steps, mutation: "empty", intentionalNegative: false };
  const choices = ["duplicate-effectful-action", "repeat-tail", "identity"];
  const choice = choices[Math.floor(rng() * choices.length)];
  if (choice === "identity") return { steps, mutation: "identity", intentionalNegative: false };
  if (choice === "repeat-tail" && steps.length > 1) {
    const tail = [...steps].reverse().find((step) => step.phase !== "assert") ?? steps.at(-1);
    return {
      steps: [...steps, { ...tail, id: `${tail.id}-repeat`, phase: "fuzz" }],
      mutation: "repeat-tail",
      intentionalNegative: true
    };
  }
  const candidate = steps.find((step) => step.phase === "fuzz") ?? steps.find((step) => step.phase !== "assert") ?? steps.at(-1);
  return {
    steps: [...steps, { ...candidate, id: `${candidate.id}-duplicate`, phase: "fuzz" }],
    mutation: "duplicate-effectful-action",
    intentionalNegative: true
  };
}

function scoreReasons(reasons) {
  let score = 0;
  for (const reason of reasons) {
    if (reason === "product-candidate") score += 50;
    else if (reason === "stable-failure") score += 35;
    else if (reason === "manual-triage") score += 25;
    else if (reason === "runtime-edge") score += 20;
    else if (reason === "runtime-state") score += 10;
    else if (reason === "async-invariant-pair") score += 15;
    else if (reason === "flaky-signal") score += 5;
    else if (reason === "harness-precondition") score += 2;
  }
  return score;
}

function sanitize(value) {
  return String(value || "none")
    .replace(/[^a-zA-Z0-9._-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 48) || "none";
}
