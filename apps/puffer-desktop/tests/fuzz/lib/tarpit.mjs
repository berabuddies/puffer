export function detectTarpit(runtimeCoverage = {}) {
  const observedStateCount = Number(runtimeCoverage.observedStateCount ?? runtimeCoverage.states?.length ?? 0);
  const uniqueStateCount = Number(runtimeCoverage.states?.length ?? 0);
  const actionEventCount = Number(runtimeCoverage.actionEventCount ?? runtimeCoverage.edges?.length ?? 0);
  const edgeCount = Number(runtimeCoverage.edges?.length ?? 0);
  const repeatedStateCount = Number(runtimeCoverage.repeatedStateCount ?? 0);
  const stateNoveltyRatio = observedStateCount === 0 ? 1 : uniqueStateCount / observedStateCount;
  const edgeYieldRatio = actionEventCount === 0 ? 1 : edgeCount / actionEventCount;
  const reasons = [];
  if (actionEventCount > 0 && edgeCount === 0) reasons.push("no-runtime-edges");
  if (observedStateCount >= 6 && stateNoveltyRatio < 0.35) reasons.push("low-state-novelty");
  if (repeatedStateCount >= 3) reasons.push("repeated-state");
  if (actionEventCount >= 4 && edgeYieldRatio < 0.35) reasons.push("low-edge-yield");
  return {
    tarpit: reasons.length > 0,
    reasons,
    observedStateCount,
    uniqueStateCount,
    actionEventCount,
    edgeCount,
    repeatedStateCount,
    stateNoveltyRatio: Number(stateNoveltyRatio.toFixed(3)),
    edgeYieldRatio: Number(edgeYieldRatio.toFixed(3)),
    escapeSuggested: reasons.length > 0
  };
}

export function aggregateTarpit(items = []) {
  const tarpitItems = items.map((item) => item?.tarpit).filter(Boolean);
  const reasonCounts = {};
  for (const item of tarpitItems) {
    for (const reason of item.reasons ?? []) reasonCounts[reason] = (reasonCounts[reason] ?? 0) + 1;
  }
  return {
    total: tarpitItems.length,
    tarpitCount: tarpitItems.filter((item) => item.tarpit).length,
    escapeSuggestedCount: tarpitItems.filter((item) => item.escapeSuggested).length,
    reasonCounts
  };
}
