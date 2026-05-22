import { summarizeRun } from "./fuzz-core.mjs";

export function shrinkRunCase(manifest, run, caseId, options = {}) {
  const original = (run.cases ?? []).find((item) => item.caseId === caseId);
  if (!original) throw new Error(`Case not found: ${caseId}`);
  const attempts = [];
  const chunkShrunk = shrinkByDeletion(original, attempts);
  const paramShrunk = simplifyParams(chunkShrunk, attempts);
  const shrunkCase = {
    ...paramShrunk,
    caseId: `${original.caseId}-shrunk`,
    originalCaseId: original.caseId,
    shrink: {
      version: 1,
      originalStepCount: original.steps?.length ?? 0,
      shrunkStepCount: paramShrunk.steps?.length ?? 0,
      removedStepCount: (original.steps?.length ?? 0) - (paramShrunk.steps?.length ?? 0),
      attempts,
      verification: options.verified ? "replay-verified" : "needs-replay-verification"
    },
    coverage: [...new Set((paramShrunk.steps ?? []).flatMap((step) => [
      ...(step.coverage ?? []),
      ...(step.assertions ?? []).map((id) => `invariant:${id}`)
    ]))].sort()
  };
  const shrunkRun = {
    ...run,
    generatedAt: new Date().toISOString(),
    options: {
      ...(run.options ?? {}),
      mode: "shrink",
      sourceCaseId: original.caseId
    },
    cases: [shrunkCase]
  };
  shrunkRun.summary = summarizeRun(manifest, shrunkRun.cases);
  return {
    original,
    shrunkCase,
    run: shrunkRun,
    report: formatShrinkMarkdown({ original, shrunkCase })
  };
}

export function formatShrinkMarkdown({ original, shrunkCase }) {
  const lines = [
    "# Puffer UI/UX Fuzz Shrink Report",
    "",
    `Original case: ${original.caseId}`,
    `Shrunk case: ${shrunkCase.caseId}`,
    `Original steps: ${shrunkCase.shrink.originalStepCount}`,
    `Shrunk steps: ${shrunkCase.shrink.shrunkStepCount}`,
    `Removed steps: ${shrunkCase.shrink.removedStepCount}`,
    `Verification: ${shrunkCase.shrink.verification}`,
    "",
    "## Attempts",
    ""
  ];
  for (const attempt of shrunkCase.shrink.attempts) {
    lines.push(`- ${attempt.kind}: ${attempt.accepted ? "accepted" : "rejected"}; ${attempt.reason}`);
  }
  lines.push("", "## Shrunk Steps", "");
  for (const step of shrunkCase.steps ?? []) {
    lines.push(`- ${step.phase ?? "unknown"}: ${step.action}${step.params ? ` ${JSON.stringify(step.params)}` : ""}`);
  }
  lines.push(
    "",
    "## Replay Requirement",
    "",
    "- This shrinker preserves local resource dependencies but does not accept findings by itself.",
    "- Replay this shrunk case with bounded replay and require the same failure signature before promoting a product bug."
  );
  return `${lines.join("\n")}\n`;
}

function shrinkByDeletion(testCase, attempts) {
  let current = { ...testCase, steps: [...(testCase.steps ?? [])] };
  let changed = true;
  while (changed) {
    changed = false;
    for (let index = 0; index < current.steps.length; index += 1) {
      const step = current.steps[index];
      if (!canDeleteStep(step)) continue;
      const candidateSteps = current.steps.filter((_, candidateIndex) => candidateIndex !== index);
      const candidate = { ...current, steps: candidateSteps };
      const validation = validateDependencyClosure(candidate.steps);
      attempts.push({
        kind: "delete-step",
        step: step.action,
        accepted: validation.ok,
        reason: validation.ok ? "dependency closure preserved" : validation.reason
      });
      if (!validation.ok) continue;
      current = candidate;
      changed = true;
      break;
    }
  }
  return current;
}

function simplifyParams(testCase, attempts) {
  const steps = (testCase.steps ?? []).map((step) => {
    if (!step.params || Object.keys(step.params).length === 0) return step;
    const simplified = simplifyValue(step.params);
    const changed = JSON.stringify(simplified) !== JSON.stringify(step.params);
    if (changed) {
      attempts.push({
        kind: "simplify-params",
        step: step.action,
        accepted: true,
        reason: "parameter values simplified for replay minimization"
      });
    }
    return changed ? { ...step, params: simplified } : step;
  });
  return { ...testCase, steps };
}

function canDeleteStep(step) {
  if (step.phase === "setup") return false;
  if (step.phase === "assert") return false;
  if (String(step.action ?? "").startsWith("assert-")) return false;
  if ((step.coverage ?? []).some((tag) => String(tag).startsWith("invariant:"))) return false;
  if ((step.produces ?? []).length > 0) return false;
  return true;
}

function validateDependencyClosure(steps) {
  const resources = new Set();
  for (const step of steps) {
    for (const required of step.requires ?? []) {
      if (!resources.has(required)) return { ok: false, reason: `missing required resource ${required} before ${step.action}` };
    }
    for (const blocked of step.blocks ?? []) {
      if (resources.has(blocked)) return { ok: false, reason: `blocked resource ${blocked} before ${step.action}` };
    }
    for (const consumed of step.consumes ?? []) resources.delete(consumed);
    for (const invalidated of step.invalidates ?? []) resources.delete(invalidated);
    for (const covered of step.coverage ?? []) resources.add(covered);
    for (const produced of step.produces ?? []) resources.add(produced);
  }
  return { ok: true, reason: "ok" };
}

function simplifyValue(value) {
  if (typeof value === "string") {
    if (/^https?:\/\//.test(value)) return "https://example.test/";
    if (value.length > 12) return value.slice(0, 12);
    return value;
  }
  if (typeof value === "number") return Math.min(value, 1);
  if (Array.isArray(value)) return value.slice(0, 1).map(simplifyValue);
  if (value && typeof value === "object") {
    return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, simplifyValue(item)]));
  }
  return value;
}
