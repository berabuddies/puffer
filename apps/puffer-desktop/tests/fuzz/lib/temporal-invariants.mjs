const INTENT_ACTIONS = {
  "send-chat-turn": ["send-prompt", "rapid-send-prompt"],
  "stop-turn": ["stop-turn", "settle-canceled-turn"],
  "answer-permission": ["answer-permission"],
  "answer-question": ["answer-question"],
  "switch-session": ["switch-session", "open-agent-card"],
  "select-model": ["change-model", "save-default-model", "switch-new-agent-provider"],
  "create-agent": ["open-new-agent", "submit-new-agent"],
  "navigate-browser": ["type-url", "press-address-enter", "reload", "history"],
  "save-file": ["edit-file", "save-file"],
  "terminal-command": ["type-terminal"],
  "update-settings": ["import-credential", "refresh-credential", "save-permissions", "add-mcp-server"]
};

export function buildIntentLedger(replay) {
  const intents = [];
  for (const [intentId, actions] of Object.entries(INTENT_ACTIONS)) {
    const matched = (replay.steps ?? []).filter((action) => actions.includes(action));
    if (matched.length === 0) continue;
    intents.push({
      intentId,
      actions: matched,
      count: matched.length
    });
  }
  return {
    version: 1,
    intents
  };
}

export function evaluateTemporalReplay(replay) {
  const coverage = new Set(replay.coverage ?? []);
  const classification = String(replay.classification ?? "");
  const observations = [];
  addObservation(observations, {
    id: "one-request-per-intent",
    applies: hasAny(replay.steps, ["send-prompt", "rapid-send-prompt", "press-address-enter"]) ||
      coverage.has("async:duplicate-submit"),
    failed: classification === "product-candidate:duplicate-intent",
    evidence: replay.failureSignature ?? ""
  });
  addObservation(observations, {
    id: "late-event-scoped-to-origin-object",
    applies: coverage.has("async:stale-session-event") || coverage.has("async:stale-tab-event"),
    failed: classification === "product-candidate:stale-browser-tab-state" ||
      classification === "product-candidate:connection-banner-blocks-navigation",
    evidence: replay.failureSignature ?? ""
  });
  addObservation(observations, {
    id: "draft-preserved-on-failure",
    applies: coverage.has("async:late-failure") || coverage.has("async:dropped-response") ||
      hasAny(replay.steps, ["type-composer", "type-url", "edit-file"]),
    failed: classification === "product-candidate:draft-recovery",
    evidence: replay.failureSignature ?? ""
  });
  addObservation(observations, {
    id: "pending-state-recoverable",
    applies: hasAny(replay.steps, ["emit-permission", "emit-question", "hold-next-browser-response", "disconnect-reconnect"]),
    failed: classification === "needs-manual-triage:timeout" || classification === "timeout",
    evidence: replay.failureSignature ?? ""
  });
  addObservation(observations, {
    id: "provider-model-pair-consistent",
    applies: hasAny(replay.steps, ["save-default-model", "switch-new-agent-provider", "change-model", "submit-new-agent"]),
    failed: classification === "product-candidate:unclassified" && coverage.has("invariant:no-cross-provider-model"),
    evidence: replay.failureSignature ?? ""
  });
  return {
    version: 1,
    observations,
    observed: observations.length,
    failed: observations.filter((item) => item.status === "failed").length,
    passed: observations.filter((item) => item.status === "passed").length
  };
}

export function aggregateTemporal(results = []) {
  const byRule = {};
  let observed = 0;
  let failed = 0;
  let passed = 0;
  for (const result of results) {
    for (const observation of result.temporalInvariants?.observations ?? []) {
      const previous = byRule[observation.id] ?? { observed: 0, passed: 0, failed: 0 };
      previous.observed += 1;
      if (observation.status === "failed") previous.failed += 1;
      if (observation.status === "passed") previous.passed += 1;
      byRule[observation.id] = previous;
      observed += 1;
      if (observation.status === "failed") failed += 1;
      if (observation.status === "passed") passed += 1;
    }
  }
  return { observed, passed, failed, byRule };
}

function addObservation(observations, { id, applies, failed, evidence }) {
  if (!applies) return;
  observations.push({
    id,
    status: failed ? "failed" : "passed",
    evidence
  });
}

function hasAny(values = [], candidates = []) {
  return values.some((value) => candidates.includes(value));
}
