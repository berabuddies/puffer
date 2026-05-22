import { writeJson, writeText } from "./fuzz-core.mjs";

export function buildScenarioPlan(scenarioManifest, options = {}) {
  const limit = Number(options.limit ?? 20);
  const rows = [
    ...(scenarioManifest.scenarios ?? []).map((item) => ({ ...item, layer: "scenario" })),
    ...(scenarioManifest.personas ?? []).map((item) => ({ ...item, layer: "persona" })),
    ...(scenarioManifest.metamorphicOracles ?? []).map((item) => ({ ...item, layer: "metamorphic" })),
    ...(scenarioManifest.multiClientScenarios ?? []).map((item) => ({ ...item, layer: "multi-client" }))
  ].sort((left, right) => {
    if (Number(right.priority ?? 0) !== Number(left.priority ?? 0)) return Number(right.priority ?? 0) - Number(left.priority ?? 0);
    return left.id.localeCompare(right.id);
  });
  return {
    version: 1,
    generatedAt: new Date().toISOString(),
    selected: rows.slice(0, limit),
    total: rows.length
  };
}

export function formatScenarioPlanMarkdown(plan) {
  const lines = [
    "# Puffer UI/UX Scenario Frontier",
    "",
    `Generated: ${plan.generatedAt}`,
    `Selected: ${plan.selected.length}/${plan.total}`,
    "",
    "## Items",
    ""
  ];
  for (const item of plan.selected) {
    lines.push(`### ${item.id}`, "");
    lines.push(`- Layer: ${item.layer}`);
    lines.push(`- Priority: ${item.priority ?? 0}`);
    if (item.description) lines.push(`- Description: ${item.description}`);
    if (item.shards) lines.push(`- Shards: ${item.shards.join(", ")}`);
    if (item.intents) lines.push(`- Intents: ${item.intents.join(", ")}`);
    if (item.invariants) lines.push(`- Invariants: ${item.invariants.join(", ")}`);
    if (item.paths) lines.push(`- Paths: ${item.paths.join(" vs ")}`);
    if (item.property) lines.push(`- Property: ${item.property}`);
    if (item.clients) lines.push(`- Clients: ${item.clients}`);
    if (item.env) lines.push(`- Environment: ${JSON.stringify(item.env)}`);
    lines.push("");
  }
  return `${lines.join("\n")}\n`;
}

export async function writeScenarioPlan(filePath, plan) {
  await writeJson(filePath, plan);
}

export async function writeScenarioPlanMarkdown(filePath, plan) {
  await writeText(filePath, formatScenarioPlanMarkdown(plan));
}
