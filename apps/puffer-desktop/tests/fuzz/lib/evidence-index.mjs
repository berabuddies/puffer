import { createHash } from "node:crypto";

export const EVIDENCE_TYPES = new Set([
  "action",
  "network",
  "storage",
  "url",
  "console",
  "accessibility",
  "predicate"
]);

export function buildReplayEvidenceIndex(payload) {
  const builder = new EvidenceIndexBuilder();
  for (const result of payload.results ?? []) {
    addActionEvidence(builder, result);
    addNetworkEvidence(builder, result);
    addUrlEvidence(builder, result);
    addConsoleEvidence(builder, result);
    addAccessibilityEvidence(builder, result);
    addPredicateEvidence(builder, result);
  }
  if ((payload.results ?? []).length === 0) {
    builder.add("predicate", "No replay cases were executed.", {
      predicate: "replay-cases-executed",
      status: "not-run"
    });
  }
  return {
    evidence_index: builder.entries,
    evidence_source: builder.source
  };
}

export function buildEvidenceIndex(records = []) {
  const builder = new EvidenceIndexBuilder();
  for (const record of records) {
    builder.add(record.type, record.value, record.metadata ?? {});
  }
  return {
    evidence_index: builder.entries,
    evidence_source: builder.source
  };
}

export function evidenceValue(entry) {
  if (entry?.value !== undefined) return String(entry.value);
  return "";
}

export function hashEvidenceValue(value) {
  return sha256(String(value ?? ""));
}

class EvidenceIndexBuilder {
  constructor() {
    this.entries = [];
    this.source = "";
    this.counts = {};
  }

  add(type, value, metadata = {}) {
    if (!EVIDENCE_TYPES.has(type)) throw new Error(`Unknown evidence type: ${type}`);
    const normalizedValue = normalizeEvidenceValue(value);
    const start = Buffer.byteLength(this.source, "utf8");
    const line = `${JSON.stringify({ type, value: normalizedValue, metadata })}\n`;
    this.source += line;
    const end = Buffer.byteLength(this.source, "utf8");
    const ordinal = (this.counts[type] ?? 0) + 1;
    this.counts[type] = ordinal;
    this.entries.push({
      id: `ev-${type}-${String(ordinal).padStart(4, "0")}`,
      type,
      byte_span: [start, end],
      sha256: hashEvidenceValue(normalizedValue),
      value: normalizedValue,
      metadata
    });
  }
}

function addActionEvidence(builder, result) {
  for (let index = 0; index < (result.stepDetails ?? []).length; index += 1) {
    const step = result.stepDetails[index];
    builder.add("action", JSON.stringify({
      caseId: result.caseId,
      step: index + 1,
      action: step.action,
      phase: step.phase,
      target: step.target,
      params: step.params ?? {},
      coverage: step.coverage ?? []
    }), {
      caseId: result.caseId,
      action: step.action,
      step: index + 1
    });
  }
}

function addNetworkEvidence(builder, result) {
  for (const step of result.stepDetails ?? []) {
    if (!step.expectedDaemon) continue;
    builder.add("network", JSON.stringify({
      caseId: result.caseId,
      action: step.action,
      expectedDaemon: step.expectedDaemon
    }), {
      caseId: result.caseId,
      action: step.action,
      expectedDaemon: step.expectedDaemon
    });
  }
}

function addUrlEvidence(builder, result) {
  const urls = new Set();
  for (const step of result.stepDetails ?? []) {
    if (step.params?.url) urls.add(String(step.params.url));
  }
  for (const tag of result.coverage ?? []) {
    if (String(tag).startsWith("route:")) urls.add(String(tag));
  }
  for (const value of urls) {
    builder.add("url", JSON.stringify({ caseId: result.caseId, value }), {
      caseId: result.caseId
    });
  }
}

function addConsoleEvidence(builder, result) {
  for (const attempt of result.attempts ?? []) {
    if (!attempt.excerpt && !attempt.failureSignature) continue;
    builder.add("console", JSON.stringify({
      caseId: result.caseId,
      attempt: attempt.attempt,
      status: attempt.status,
      exitCode: attempt.exitCode,
      failureSignature: attempt.failureSignature ?? "",
      excerpt: attempt.excerpt ?? ""
    }), {
      caseId: result.caseId,
      attempt: attempt.attempt
    });
  }
}

function addAccessibilityEvidence(builder, result) {
  const states = result.runtimeCoverage?.states ?? [];
  for (const state of states.slice(0, 5)) {
    if (!state.a11y && !state.modalStack && !state.visual) continue;
    builder.add("accessibility", JSON.stringify({
      caseId: result.caseId,
      stateHash: state.stateHash,
      routePattern: state.routePattern,
      activePanel: state.activePanel,
      activeTab: state.activeTab,
      focusRegion: state.focusRegion,
      modalStack: state.modalStack ?? [],
      a11y: state.a11y ?? {},
      visual: state.visual ?? {}
    }), {
      caseId: result.caseId,
      stateHash: state.stateHash
    });
  }
}

function addPredicateEvidence(builder, result) {
  const failedTemporal = (result.temporalInvariants?.observations ?? [])
    .filter((item) => item.status === "failed");
  const shouldEmitPredicate = result.status !== "passed" && (
    String(result.classification ?? "").startsWith("product-candidate:") ||
    String(result.classification ?? "").startsWith("needs-manual-triage") ||
    failedTemporal.length > 0
  );
  if (!shouldEmitPredicate) return;
  builder.add("predicate", JSON.stringify({
    caseId: result.caseId,
    status: result.status,
    classification: result.classification,
    failureSignature: result.failureSignature ?? "",
    failedTemporal
  }), {
    caseId: result.caseId,
    classification: result.classification ?? ""
  });
}

function normalizeEvidenceValue(value) {
  return String(value ?? "")
    .replace(/[0-9a-fA-F]{8}-[0-9a-fA-F-]{27,}/g, "<uuid>")
    .replace(/\b\d{13}\b/g, "<timestamp>")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, 4000);
}

function sha256(value) {
  return createHash("sha256").update(String(value)).digest("hex");
}
