import { indexCoverageTargets, productTierForTag, scoreCase } from "./fuzz-core.mjs";

export function buildFrontier(manifest, seeds, ledger = {}, options = {}) {
  const limit = Number(options.limit ?? 30);
  const validated = new Set(ledger.validatedTags ?? []);
  const fixedSignatures = new Set([
    ...(ledger.knownBugSignatures ?? []),
    ...(ledger.fixedFindings ?? []).map((item) => item.bugSignature).filter(Boolean)
  ]);
  const targets = indexCoverageTargets(manifest);
  const rows = [];

  for (const target of targets.values()) {
    if (validated.has(target.tag)) continue;
    const matchingSeeds = seeds
      .filter((seed) => seedCovers(seed, target.tag))
    const matchingSeedIds = matchingSeeds.map((seed) => seed.id);
    const priority = Number(target.priority ?? 0);
    const productTier = productTierForTag(target.tag, matchingSeeds);
    const tierWeight = productTier === 0 ? 40 : productTier === 1 ? 12 : 0;
    const riskWeight = priority >= 10 ? 3 : priority >= 8 ? 2 : 1;
    const seedWeight = matchingSeedIds.length === 0 ? 0 : Math.min(3, matchingSeedIds.length);
    const dimensionWeight = target.dimension === "async" || target.dimension === "invariant" ? 3 : 1;
    rows.push({
      tag: target.tag,
      dimension: target.dimension,
      priority,
      productTier,
      score: tierWeight + priority * riskWeight + seedWeight + dimensionWeight,
      description: target.description ?? "",
      seeds: matchingSeedIds,
      duplicateGuardCount: fixedSignatures.size
    });
  }

  rows.sort((left, right) => {
    if (left.productTier !== right.productTier) return left.productTier - right.productTier;
    if (right.score !== left.score) return right.score - left.score;
    if (right.priority !== left.priority) return right.priority - left.priority;
    return left.tag.localeCompare(right.tag);
  });

  return {
    generatedAt: new Date().toISOString(),
    limit,
    items: rows.slice(0, limit),
    totalOpenFrontier: rows.length
  };
}

export function formatFrontierMarkdown(frontier) {
  const lines = [
    "# Puffer UI/UX Fuzz Frontier",
    "",
    `Generated: ${frontier.generatedAt}`,
    `Open frontier items: ${frontier.totalOpenFrontier}`,
    "",
    "## Top Targets",
    ""
  ];
  for (const item of frontier.items) {
    lines.push(`- ${item.tag} | tier ${item.productTier} | score ${item.score} | priority ${item.priority} | seeds: ${item.seeds.join(", ") || "none"}`);
    lines.push(`  ${item.description}`);
  }
  return `${lines.join("\n")}\n`;
}

export function summarizeRunForLedger(run) {
  const ranked = [...(run.cases ?? [])].sort((left, right) => scoreCase(right) - scoreCase(left));
  return {
    generatedAt: new Date().toISOString(),
    runGeneratedAt: run.generatedAt,
    caseCount: run.cases?.length ?? 0,
    topCaseIds: ranked.slice(0, 10).map((item) => item.caseId),
    coveredTags: run.summary?.coveredTags ?? []
  };
}

function seedCovers(seed, tag) {
  for (const step of [...(seed.setup ?? []), ...(seed.actions ?? [])]) {
    if ((step.coverage ?? []).includes(tag)) return true;
  }
  if (tag.startsWith("invariant:")) {
    return (seed.invariants ?? []).includes(tag.slice("invariant:".length));
  }
  return false;
}
