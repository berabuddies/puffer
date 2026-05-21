import { indexCoverageTargets } from "./fuzz-core.mjs";

export function evaluateGate(manifest, ledger = {}, options = {}) {
  const thresholds = {
    highRiskCoverage: Number(options.highRiskCoverage ?? 0.8),
    replaySuccessRate: Number(options.replaySuccessRate ?? 0.8),
    duplicateReportRate: Number(options.duplicateReportRate ?? 0.35),
    flakeRate: Number(options.flakeRate ?? 0.05)
  };
  const targets = [...indexCoverageTargets(manifest).values()];
  const highRiskTargets = targets.filter((item) => Number(item.priority ?? 0) >= 8);
  const validated = new Set(ledger.validatedTags ?? []);
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
  return {
    generatedAt: new Date().toISOString(),
    passed: checks.every((item) => item.passed),
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
    `Status: ${result.passed ? "PASS" : "FAIL"}`,
    `High-risk coverage: ${(result.highRiskCoverage * 100).toFixed(1)}% (${result.highRiskValidated}/${result.highRiskTotal})`,
    "",
    "## Checks",
    ""
  ];
  for (const check of result.checks) {
    lines.push(`- ${check.passed ? "PASS" : "FAIL"} ${check.id}: value ${formatNumber(check.value)}, threshold ${formatNumber(check.threshold)}`);
  }
  return `${lines.join("\n")}\n`;
}

function formatNumber(value) {
  return Number.isFinite(value) ? Number(value).toFixed(3).replace(/0+$/, "").replace(/\.$/, "") : String(value);
}
