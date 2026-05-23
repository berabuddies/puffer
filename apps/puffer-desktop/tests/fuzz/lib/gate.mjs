import { indexCoverageTargets } from "./fuzz-core.mjs";

export function evaluateGate(manifest, ledger = {}, options = {}) {
  const profile = String(options.profile ?? "ready");
  const profileDefaults = gateProfileDefaults(profile);
  const thresholds = {
    highRiskCoverage: Number(options.highRiskCoverage ?? profileDefaults.highRiskCoverage),
    replaySuccessRate: Number(options.replaySuccessRate ?? profileDefaults.replaySuccessRate),
    duplicateReportRate: Number(options.duplicateReportRate ?? profileDefaults.duplicateReportRate),
    flakeRate: Number(options.flakeRate ?? profileDefaults.flakeRate),
    minReplayCases: Number(options.minReplayCases ?? profileDefaults.minReplayCases)
  };
  const targets = [...indexCoverageTargets(manifest).values()];
  const highRiskTargets = targets.filter((item) => Number(item.priority ?? 0) >= 8);
  const validated = new Set(ledger.validatedTags ?? []);
  const replayedCases = Array.isArray(ledger.replayedCases) ? ledger.replayedCases.length : 0;
  const highRiskValidated = highRiskTargets.filter((item) => validated.has(item.tag)).length;
  const highRiskCoverage = highRiskTargets.length === 0 ? 1 : highRiskValidated / highRiskTargets.length;
  const metrics = ledger.metrics ?? {};
  const replaySuccessRate = Number(metrics.replaySuccessRate ?? 0);
  const duplicateReportRate = Number(metrics.duplicateReportRate ?? 0);
  const flakeRate = Number(metrics.flakeRate ?? 0);
  const openP0 = Number(metrics.p0Open ?? 0);
  const openP1 = Number(metrics.p1Open ?? 0);
  const checks = [
    {
      id: "replay-evidence",
      passed: replayedCases >= thresholds.minReplayCases,
      value: replayedCases,
      threshold: thresholds.minReplayCases
    },
    {
      id: "high-risk-coverage",
      passed: highRiskCoverage >= thresholds.highRiskCoverage,
      value: highRiskCoverage,
      threshold: thresholds.highRiskCoverage
    },
    {
      id: "replay-success-rate",
      passed: replaySuccessRate >= thresholds.replaySuccessRate,
      value: replaySuccessRate,
      threshold: thresholds.replaySuccessRate
    },
    {
      id: "duplicate-report-rate",
      passed: duplicateReportRate <= thresholds.duplicateReportRate,
      value: duplicateReportRate,
      threshold: thresholds.duplicateReportRate
    },
    {
      id: "flake-rate",
      passed: flakeRate <= thresholds.flakeRate,
      value: flakeRate,
      threshold: thresholds.flakeRate
    },
    {
      id: "open-p0",
      passed: openP0 === 0,
      value: openP0,
      threshold: 0
    },
    {
      id: "open-p1",
      passed: openP1 === 0,
      value: openP1,
      threshold: 0
    }
  ];
  const blockingChecks = checks.filter((item) => item.id !== "replay-evidence");
  const hasEnoughReplayEvidence = replayedCases >= thresholds.minReplayCases;
  const blockerPassed = blockingChecks.every((item) => item.passed);
  const status = gateStatus({
    profile,
    hasEnoughReplayEvidence,
    blockerPassed,
    replayedCases
  });
  return {
    profile,
    generatedAt: new Date().toISOString(),
    status,
    passed: status === "PASS" || status === "BOOTSTRAP",
    thresholds,
    replayedCases,
    highRiskCoverage,
    highRiskValidated,
    highRiskTotal: highRiskTargets.length,
    checks
  };
}

export function formatGateMarkdown(result) {
  const lines = [
    "# Puffer UI/UX Fuzz Ready Gate",
    "",
    `Generated: ${result.generatedAt}`,
    `Profile: ${result.profile ?? "ready"}`,
    `Status: ${result.status ?? (result.passed ? "PASS" : "FAIL")}`,
    `High-risk coverage: ${(result.highRiskCoverage * 100).toFixed(1)}% (${result.highRiskValidated}/${result.highRiskTotal})`,
    `Replay evidence: ${result.replayedCases ?? 0} cases`,
    "",
    "## Checks",
    ""
  ];
  for (const check of result.checks) {
    lines.push(`- ${check.passed ? "PASS" : "FAIL"} ${check.id}: value ${formatNumber(check.value)}, threshold ${formatNumber(check.threshold)}`);
  }
  return `${lines.join("\n")}\n`;
}

function gateProfileDefaults(profile) {
  if (profile === "bootstrap") {
    return {
      highRiskCoverage: 0,
      replaySuccessRate: 0,
      duplicateReportRate: 1,
      flakeRate: 1,
      minReplayCases: 0
    };
  }
  if (profile === "release") {
    return {
      highRiskCoverage: 0.9,
      replaySuccessRate: 0.9,
      duplicateReportRate: 0.2,
      flakeRate: 0.03,
      minReplayCases: 100
    };
  }
  if (profile === "ready") {
    return {
      highRiskCoverage: 0.8,
      replaySuccessRate: 0.8,
      duplicateReportRate: 0.35,
      flakeRate: 0.05,
      minReplayCases: 20
    };
  }
  throw new Error(`Unknown gate profile: ${profile}`);
}

function gateStatus({ profile, hasEnoughReplayEvidence, blockerPassed, replayedCases }) {
  if (profile === "bootstrap" && replayedCases === 0) return blockerPassed ? "BOOTSTRAP" : "FAIL";
  if (!hasEnoughReplayEvidence) return "INSUFFICIENT_DATA";
  return blockerPassed ? "PASS" : "FAIL";
}

function formatNumber(value) {
  return Number.isFinite(value) ? Number(value).toFixed(3).replace(/0+$/, "").replace(/\.$/, "") : String(value);
}
