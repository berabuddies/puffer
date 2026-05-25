#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { evaluateFindingAdmission } from "../lib/admission-gate.mjs";
import { buildEvidenceIndex } from "../lib/evidence-index.mjs";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const runDir = path.join(fuzzRoot, ".runs", `selftest-${Date.now()}`);
fs.mkdirSync(runDir, { recursive: true });

const evidence = buildEvidenceIndex([
  {
    type: "action",
    value: JSON.stringify({ caseId: "selftest", action: "click", selector: "#submit" }),
    metadata: { caseId: "selftest" }
  },
  {
    type: "predicate",
    value: JSON.stringify({ caseId: "selftest", predicate: "one request per intent failed" }),
    metadata: { caseId: "selftest", predicate: "one-request-per-intent" }
  }
]);
const action = evidence.evidence_index.find((entry) => entry.type === "action");
const predicate = evidence.evidence_index.find((entry) => entry.type === "predicate");
const admittedVerdict = verdict({
  decision: "admit",
  primary: predicate,
  citations: [action],
  title: "Selftest admitted predicate finding"
});
const candidateVerdict = verdict({
  decision: "candidate",
  primary: action,
  citations: [action],
  title: "Selftest predicate-missing candidate"
});
const hallucinatedVerdict = {
  ...admittedVerdict,
  primary_cause: { id: "ev-predicate-9999", type: "predicate", quote_hash: predicate.sha256 }
};
const malformedVerdict = {
  ...admittedVerdict
};
delete malformedVerdict.impact;

const admittedGate = evaluateFindingAdmission(admittedVerdict, evidence.evidence_index);
const candidateGate = evaluateFindingAdmission(candidateVerdict, evidence.evidence_index);
const hallucinatedGate = evaluateFindingAdmission(hallucinatedVerdict, evidence.evidence_index);
const malformedGate = evaluateFindingAdmission(malformedVerdict, evidence.evidence_index);

assert(admittedGate.disposition === "admitted" && admittedGate.passed, "valid predicate verdict must be admitted");
assert(candidateGate.disposition === "candidate", "non-predicate primary cause must become candidate");
assert(hallucinatedGate.disposition === "gate_failed", "hallucinated evidence id must fail gate");
assert(malformedGate.disposition === "gate_failed", "schema-invalid admitted verdict must fail gate");

const admittedVerdictPath = writeJson("admitted-verdict.json", admittedVerdict);
const admittedGatePath = writeJson("admitted-gate.json", admittedGate);
const candidateVerdictPath = writeJson("candidate-verdict.json", candidateVerdict);
const candidateGatePath = writeJson("candidate-gate.json", candidateGate);
const hallucinatedVerdictPath = writeJson("hallucinated-verdict.json", hallucinatedVerdict);
const hallucinatedGatePath = writeJson("hallucinated-gate.json", hallucinatedGate);
const malformedVerdictPath = writeJson("malformed-verdict.json", malformedVerdict);
const malformedGatePath = writeJson("malformed-gate.json", malformedGate);
const bugListPath = path.join(runDir, "BUGS.md");
const candidateListPath = path.join(runDir, "BUGS_CAND.md");
const evidencePath = writeJson("evidence.json", evidence);
const reviewerPath = path.join(runDir, "reviewer.json");

runExpect(0, [
  "node",
  "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
  "bug-list",
  "--bug-list",
  relative(bugListPath),
  "--append-from-verdict",
  "--verdict",
  relative(admittedVerdictPath),
  "--gate",
  relative(admittedGatePath)
]);
runExpect(1, [
  "node",
  "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
  "bug-list",
  "--bug-list",
  relative(path.join(runDir, "BUGS_REFUSE.md")),
  "--append-from-verdict",
  "--verdict",
  relative(hallucinatedVerdictPath),
  "--gate",
  relative(hallucinatedGatePath)
]);
runExpect(1, [
  "node",
  "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
  "bug-list",
  "--bug-list",
  relative(path.join(runDir, "BUGS_SCHEMA_REFUSE.md")),
  "--append-from-verdict",
  "--verdict",
  relative(malformedVerdictPath),
  "--gate",
  relative(malformedGatePath)
]);
runExpect(0, [
  "node",
  "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
  "candidate-list",
  "--candidate-list",
  relative(candidateListPath),
  "--append",
  "--verdict",
  relative(candidateVerdictPath),
  "--gate",
  relative(candidateGatePath),
  "--evidence",
  relative(evidencePath)
]);
runExpect(0, [
  "node",
  "apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-reviewer.mjs",
  "--offline",
  "--verdict",
  relative(candidateVerdictPath),
  "--gate",
  relative(candidateGatePath),
  "--replay",
  relative(evidencePath),
  "--out",
  relative(reviewerPath)
]);
const review = JSON.parse(fs.readFileSync(reviewerPath, "utf8"));
assert(review.decision === "human_queue", "offline candidate reviewer must produce a human_queue decision");
runExpect(0, [
  "node",
  "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
  "candidate-list",
  "--candidate-list",
  relative(candidateListPath),
  "--set-status",
  "--id",
  "PUF-CAND-0001",
  "--status",
  "human-queue",
  "--note",
  "offline reviewer smoke"
]);

const result = {
  version: 1,
  generatedAt: new Date().toISOString(),
  runDir: relative(runDir),
  admittedDisposition: admittedGate.disposition,
  candidateDisposition: candidateGate.disposition,
  hallucinatedDisposition: hallucinatedGate.disposition,
  malformedDisposition: malformedGate.disposition,
  reviewerDecision: review.decision,
  bugListPath: relative(bugListPath),
  candidateListPath: relative(candidateListPath)
};
writeJson("selftest-result.json", result);
process.stdout.write(`PUFFER_FUZZ_SELFTEST_OK ${relative(path.join(runDir, "selftest-result.json"))}\n`);

function verdict({ decision, primary, citations, title }) {
  return {
    version: 1,
    decision,
    title,
    severity: "P1",
    area: "selftest",
    shard: "selftest-shard",
    source_run: "selftest",
    primary_cause: { id: primary.id, type: primary.type, quote_hash: primary.sha256 },
    citations: citations.map((entry) => ({ id: entry.id, type: entry.type, quote_hash: entry.sha256 })),
    expected: "The deterministic gate should enforce cited evidence.",
    actual: "The selftest emits controlled evidence and verdicts.",
    impact: "Invalid claims must not reach the bug ledger.",
    repro: ["run puffer-fuzz-selftest"],
    notes: "selftest"
  };
}

function writeJson(name, value) {
  const filePath = path.join(runDir, name);
  fs.writeFileSync(filePath, `${JSON.stringify(value, null, 2)}\n`);
  return filePath;
}

function runExpect(expected, command) {
  const result = spawnSync(command[0], command.slice(1), {
    cwd: repoRoot,
    encoding: "utf8"
  });
  const ok = expected === 0 ? result.status === 0 : result.status !== 0;
  if (!ok) {
    throw new Error([
      `Expected ${command.join(" ")} to ${expected === 0 ? "pass" : "fail"}`,
      `status=${result.status}`,
      result.stdout,
      result.stderr
    ].join("\n"));
  }
}

function assert(value, message) {
  if (!value) throw new Error(message);
}

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
