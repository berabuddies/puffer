import { EVIDENCE_TYPES, hashEvidenceValue } from "./evidence-index.mjs";

export const VERDICT_DECISIONS = new Set(["admit", "candidate", "dismiss"]);
export const REVIEWER_DECISIONS = new Set(["admit", "dismiss", "human_queue"]);

export function parseStrictJsonObject(content, label = "JSON content") {
  const text = String(content ?? "").trim();
  if (!text) throw new Error(`${label} is empty`);
  if (text.startsWith("```") || text.endsWith("```")) {
    throw new Error(`${label} must be raw JSON, not a Markdown code fence`);
  }
  const parsed = JSON.parse(text);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error(`${label} must parse to a JSON object`);
  }
  return parsed;
}

export function parseStrictVerdictJson(content) {
  return parseStrictJsonObject(content, "verdict");
}

export function buildNoFindingVerdict({ namespace, shard, seed, replaySummary = {} }) {
  return {
    version: 1,
    decision: "dismiss",
    title: "No admitted UI/UX finding",
    severity: "P3",
    area: shard || "unknown",
    shard: shard || "unknown",
    source_run: namespace || "",
    primary_cause: null,
    citations: [],
    expected: "Replay should expose a deterministic product-visible blocker before admission.",
    actual: `Replay summary: ${JSON.stringify(replaySummary)}`,
    impact: "No product impact was admitted.",
    repro: [],
    notes: seed ? `Seed ${seed} produced no actionable replay signal.` : "No actionable replay signal."
  };
}

export function evaluateFindingAdmission(verdict, evidenceIndex = [], options = {}) {
  const schema = validateVerdictSchema(verdict);
  const normalized = normalizeVerdict(verdict);
  const byId = new Map((evidenceIndex ?? []).map((entry) => [entry.id, entry]));
  const clauses = [];
  const citations = normalized.citations ?? [];
  const cited = citations.filter(Boolean);
  const primary = normalized.primary_cause;
  const primaryCitation = primary ? [primary] : [];
  const allCitations = [...primaryCitation, ...cited];

  clauses.push(check(
    "schema-valid",
    schema.passed,
    schema.passed ? "verdict matches required schema" : schema.errors.join("; ")
  ));
  clauses.push(check(
    "citation-exists",
    allCitations.every((citation) => byId.has(citation.id)),
    missingIds(allCitations, byId).join(", ") || "all cited ids exist"
  ));
  clauses.push(check(
    "citation-type-match",
    allCitations.every((citation) => {
      const entry = byId.get(citation.id);
      return entry && citation.type === entry.type && EVIDENCE_TYPES.has(citation.type);
    }),
    "each cited type matches recorded evidence type"
  ));
  clauses.push(check(
    "citation-value-match",
    allCitations.every((citation) => {
      const entry = byId.get(citation.id);
      return entry && citation.quote_hash === entry.sha256 &&
        citation.quote_hash === hashEvidenceValue(entry.value ?? "");
    }),
    "each quote_hash matches recorded evidence hash"
  ));
  clauses.push(check(
    "primary-cause-is-predicate",
    Boolean(primary && byId.get(primary.id)?.type === "predicate"),
    primary ? `primary cause ${primary.id} resolves to ${byId.get(primary.id)?.type ?? "missing"}` : "primary cause missing"
  ));
  clauses.push(check(
    "prose-not-page-copy",
    !isShingleCopy(verdictProse(normalized), evidenceIndex, options),
    "verdict prose is not a page/string copy"
  ));

  const passed = clauses.every((clause) => clause.passed);
  const candidateEligible = normalized.decision !== "dismiss" && !passed &&
    clauses.filter((clause) => !clause.passed).map((clause) => clause.id).join(",") === "primary-cause-is-predicate";
  const disposition = passed && normalized.decision === "admit"
    ? "admitted"
    : candidateEligible && normalized.decision !== "dismiss"
      ? "candidate"
      : normalized.decision === "dismiss"
        ? "dismissed"
        : "gate_failed";
  return {
    version: 1,
    generatedAt: new Date().toISOString(),
    disposition,
    passed,
    candidateEligible,
    clauses,
    failureReasons: clauses.filter((clause) => !clause.passed).map((clause) => `${clause.id}: ${clause.detail}`),
    verdict: normalized
  };
}

export function normalizeVerdict(input) {
  const verdict = input && typeof input === "object" ? input : {};
  const decision = VERDICT_DECISIONS.has(verdict.decision) ? verdict.decision : "dismiss";
  return {
    version: 1,
    decision,
    title: stringField(verdict.title, decision === "dismiss" ? "No admitted UI/UX finding" : "Untitled UI/UX finding"),
    severity: ["P0", "P1", "P2", "P3"].includes(verdict.severity) ? verdict.severity : "P2",
    area: stringField(verdict.area, "unknown"),
    shard: stringField(verdict.shard, "unknown"),
    source_run: stringField(verdict.source_run, ""),
    primary_cause: normalizeCitation(verdict.primary_cause),
    citations: Array.isArray(verdict.citations) ? verdict.citations.map(normalizeCitation).filter(Boolean) : [],
    expected: stringField(verdict.expected, ""),
    actual: stringField(verdict.actual, ""),
    impact: stringField(verdict.impact, ""),
    repro: Array.isArray(verdict.repro) ? verdict.repro.map((item) => String(item)).filter(Boolean) : [],
    notes: stringField(verdict.notes, "")
  };
}

export function validateVerdictSchema(input) {
  const errors = [];
  if (!input || typeof input !== "object" || Array.isArray(input)) {
    return { passed: false, errors: ["verdict must be an object"] };
  }
  requireEqual(input.version, 1, "version", errors);
  requireEnum(input.decision, VERDICT_DECISIONS, "decision", errors);
  requireEnum(input.severity, new Set(["P0", "P1", "P2", "P3"]), "severity", errors);
  for (const field of ["title", "area", "shard", "source_run", "expected", "actual", "impact", "notes"]) {
    requireString(input[field], field, errors);
  }
  if (!Array.isArray(input.repro) || !input.repro.every((item) => typeof item === "string")) {
    errors.push("repro must be an array of strings");
  }
  if (!Array.isArray(input.citations)) {
    errors.push("citations must be an array");
  } else {
    input.citations.forEach((citation, index) => requireCitation(citation, `citations[${index}]`, errors));
  }
  if (input.primary_cause !== null) {
    requireCitation(input.primary_cause, "primary_cause", errors);
  }

  if (["admit", "candidate"].includes(input.decision)) {
    for (const field of ["title", "area", "shard", "source_run", "expected", "actual", "impact"]) {
      if (typeof input[field] !== "string" || input[field].trim() === "") {
        errors.push(`${field} is required for ${input.decision}`);
      }
    }
    if (input.primary_cause === null || input.primary_cause === undefined) {
      errors.push(`primary_cause is required for ${input.decision}`);
    }
    if (!Array.isArray(input.repro) || input.repro.length === 0) {
      errors.push(`repro requires at least one step for ${input.decision}`);
    }
  }

  return { passed: errors.length === 0, errors };
}

function normalizeCitation(value) {
  if (!value || typeof value !== "object") return null;
  const type = String(value.type ?? "");
  return {
    id: String(value.id ?? ""),
    type,
    quote_hash: String(value.quote_hash ?? "")
  };
}

function requireEqual(actual, expected, field, errors) {
  if (actual !== expected) errors.push(`${field} must be ${JSON.stringify(expected)}`);
}

function requireEnum(actual, values, field, errors) {
  if (!values.has(actual)) errors.push(`${field} must be one of: ${[...values].join(", ")}`);
}

function requireString(value, field, errors) {
  if (typeof value !== "string") errors.push(`${field} must be a string`);
}

function requireCitation(value, field, errors) {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    errors.push(`${field} must be a citation object or null`);
    return;
  }
  if (typeof value.id !== "string" || value.id.trim() === "") errors.push(`${field}.id must be a non-empty string`);
  if (!EVIDENCE_TYPES.has(value.type)) errors.push(`${field}.type must be a supported evidence type`);
  if (typeof value.quote_hash !== "string" || value.quote_hash.trim() === "") {
    errors.push(`${field}.quote_hash must be a non-empty string`);
  }
}

function stringField(value, fallback) {
  const text = String(value ?? "").trim();
  return text || fallback;
}

function check(id, passed, detail) {
  return { id, passed: Boolean(passed), detail };
}

function missingIds(citations, byId) {
  return citations.map((citation) => citation.id).filter((id) => !byId.has(id));
}

function verdictProse(verdict) {
  return [
    verdict.title,
    verdict.expected,
    verdict.actual,
    verdict.impact,
    ...(verdict.repro ?? []),
    verdict.notes
  ].join(" ");
}

function isShingleCopy(prose, evidenceIndex, options) {
  const text = normalizeForShingles(prose);
  const tokens = text.split(" ").filter(Boolean);
  if (tokens.length < 10) return false;
  const proseShingles = new Set(shingles(tokens, 5));
  if (proseShingles.size === 0) return false;
  const evidenceText = normalizeForShingles(
    (evidenceIndex ?? [])
      .filter((entry) => entry.type !== "predicate")
      .map((entry) => entry.value ?? "")
      .join(" ")
  );
  if (!evidenceText) return false;
  const evidenceShingles = new Set(shingles(evidenceText.split(" ").filter(Boolean), 5));
  let overlap = 0;
  for (const shingle of proseShingles) {
    if (evidenceShingles.has(shingle)) overlap += 1;
  }
  const threshold = Number(options.copyThreshold ?? 0.75);
  return overlap / proseShingles.size >= threshold;
}

function normalizeForShingles(value) {
  return String(value ?? "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function shingles(tokens, size) {
  const result = [];
  for (let index = 0; index <= tokens.length - size; index += 1) {
    result.push(tokens.slice(index, index + size).join(" "));
  }
  return result;
}
