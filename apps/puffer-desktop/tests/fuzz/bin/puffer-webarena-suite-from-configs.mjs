#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const configDir = path.resolve(repoRoot, String(args["config-dir"] ?? "/tmp/puffer-bench-inspect-webarena/config_files"));
const outPath = path.resolve(repoRoot, String(args.out ?? "apps/puffer-desktop/tests/fuzz/benchmarks/webarena_full_suite.local.json"));
const files = fs.readdirSync(configDir)
  .filter((name) => /^\d+\.json$/.test(name))
  .sort((left, right) => Number.parseInt(left, 10) - Number.parseInt(right, 10));
const shards = files.map((name) => {
  const configPath = path.join(configDir, name);
  const task = JSON.parse(fs.readFileSync(configPath, "utf8"));
  const taskId = task.task_id ?? Number.parseInt(name, 10);
  const sites = Array.isArray(task.sites) ? task.sites.map(String) : [];
  const storageState = task.storage_state ? path.resolve(configDir, "..", String(task.storage_state)) : null;
  return {
    id: `task-${String(taskId).padStart(3, "0")}`,
    area: sites.join("+") || "unknown",
    task_id: taskId,
    shard_path: `webarena/${sites.join("+") || "unknown"}/${taskId}`,
    config_path: relative(configPath),
    require_login: Boolean(task.require_login),
    storage_state: storageState ? relative(storageState) : null,
    start_url: task.start_url,
    intent: task.intent,
    eval: task.eval,
    offline_actions: [{ type: "goto", url: task.start_url }]
  };
});
const suite = {
  version: 1,
  name: "webarena-full-suite",
  description: "Generated from official WebArena config_files/*.json. Requires the full WebArena multi-site environment and auth state.",
  source: {
    config_dir: relative(configDir),
    score_semantics: "Mean evaluator pass rate across generated WebArena config files."
  },
  shards
};
fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(outPath, `${JSON.stringify(suite, null, 2)}\n`);
process.stdout.write(`WEBARENA_SUITE_FROM_CONFIGS_OK ${relative(outPath)} shards=${shards.length}\n`);

function parseArgs(argv) {
  const parsed = {};
  for (let index = 0; index < argv.length; index += 1) {
    const item = argv[index];
    if (!item.startsWith("--")) continue;
    const key = item.slice(2);
    const next = argv[index + 1];
    if (!next || next.startsWith("--")) parsed[key] = "true";
    else {
      parsed[key] = next;
      index += 1;
    }
  }
  return parsed;
}

function relative(filePath) {
  return path.relative(repoRoot, filePath).replaceAll(path.sep, "/");
}
