#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const fuzzRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = path.resolve(fuzzRoot, "..", "..", "..", "..");
const args = parseArgs(process.argv.slice(2));
const baseUrl = String(args["base-url"] ?? process.env.PUFFER_JUICE_SHOP_BASE_URL ?? "http://127.0.0.1:3000").replace(/\/+$/, "");
const inputPath = args.input ? path.resolve(repoRoot, String(args.input)) : "";
const outPath = path.resolve(repoRoot, String(args.out ?? "apps/puffer-desktop/tests/fuzz/benchmarks/juice_shop_full_suite.local.json"));

const payload = inputPath ? JSON.parse(fs.readFileSync(inputPath, "utf8")) : await readChallenges(baseUrl);
const challenges = Array.isArray(payload.data) ? payload.data : [];
const shards = challenges.map((challenge) => ({
  id: safeShardId(challenge),
  area: safeArea(challenge.category),
  title: String(challenge.name ?? challenge.key ?? ""),
  goal: String(challenge.description ?? challenge.name ?? challenge.key ?? ""),
  target_challenges: [String(challenge.key)],
  difficulty: Number(challenge.difficulty ?? 0),
  disabled_env: challenge.disabledEnv ?? null,
  allowed_paths: pathHints(challenge),
  offline_actions: seedActions(challenge)
}));
const suite = {
  version: 1,
  name: "juice-shop-live-native-challenge-score",
  description: "Generated from a live OWASP Juice Shop /api/Challenges response. Uses native challenge-score deltas as the evaluator.",
  source: {
    base_url: inputPath ? "" : baseUrl,
    input: inputPath ? relative(inputPath) : "",
    total_challenges: shards.length,
    score_semantics: "Each shard passes when the runner observes its target challenge key newly solved after bounded replay."
  },
  shards
};
fs.mkdirSync(path.dirname(outPath), { recursive: true });
fs.writeFileSync(outPath, `${JSON.stringify(suite, null, 2)}\n`);
process.stdout.write(`JUICE_SHOP_SUITE_FROM_LIVE_OK ${relative(outPath)} shards=${shards.length}\n`);

async function readChallenges(url) {
  const response = await fetch(`${url}/api/Challenges`);
  if (!response.ok) throw new Error(`Juice Shop challenges endpoint failed with ${response.status}`);
  return response.json();
}

function pathHints(challenge) {
  const text = normalize(`${challenge.key} ${challenge.name} ${challenge.description} ${challenge.category}`);
  const paths = new Set(["/"]);
  if (text.includes("score board") || text.includes("scoreboard")) paths.add("/#/score-board");
  if (text.includes("login") || text.includes("password") || text.includes("authentication")) paths.add("/#/login");
  if (text.includes("register") || text.includes("registration")) paths.add("/#/register");
  if (text.includes("search") || text.includes("xss")) paths.add("/#/search");
  if (text.includes("feedback") || text.includes("contact")) paths.add("/#/contact");
  if (text.includes("basket") || text.includes("cart")) paths.add("/#/basket");
  if (text.includes("ftp") || text.includes("file") || text.includes("confidential") || text.includes("acquisition")) paths.add("/ftp");
  if (text.includes("metrics")) paths.add("/metrics");
  if (text.includes("administration") || text.includes("admin section")) paths.add("/#/administration");
  paths.add("/api/Challenges");
  return [...paths];
}

function seedActions(challenge) {
  const key = String(challenge.key ?? "");
  if (key === "directoryListingChallenge") return [{ type: "goto", path: "/ftp/acquisitions.md" }, { type: "wait", ms: 500 }];
  if (key === "scoreBoardChallenge") return [{ type: "goto", path: "/#/score-board" }, { type: "wait", ms: 1000 }];
  if (key === "adminSectionChallenge") return [{ type: "goto", path: "/assets/public/images/padding/19px.png" }, { type: "wait", ms: 1000 }];
  if (key === "localXssChallenge") return [{ type: "goto", path: "/#/search?q=%3Ciframe%20src%3D%22javascript%3Aalert%28%60xss%60%29%22%3E" }, { type: "wait", ms: 1000 }];
  if (key === "loginAdminChallenge") {
    return [{ type: "request", method: "POST", path: "/rest/user/login", json: { email: "admin@juice-sh.op", password: "admin123" } }, { type: "wait", ms: 1000 }];
  }
  if (key === "weakPasswordChallenge") {
    return [{ type: "request", method: "POST", path: "/rest/user/login", json: { email: "admin@juice-sh.op", password: "admin123" } }, { type: "wait", ms: 1000 }];
  }
  if (key === "emptyUserRegistration") return [{ type: "request", method: "POST", path: "/api/Users", json: {} }, { type: "wait", ms: 1000 }];
  if (key === "zeroStarsChallenge") return [{ type: "submitFeedback", comment: "puffer fuzz feedback", rating: 0 }, { type: "wait", ms: 1000 }];
  if (key === "exposedMetricsChallenge") return [{ type: "goto", path: "/metrics" }, { type: "wait", ms: 500 }];
  return [];
}

function safeShardId(challenge) {
  return String(challenge.key ?? challenge.name ?? challenge.id).replace(/Challenge$/, "").replace(/[^a-zA-Z0-9_-]+/g, "-").replace(/^-+|-+$/g, "") || `challenge-${challenge.id}`;
}

function safeArea(value) {
  return String(value ?? "unknown").toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "unknown";
}

function normalize(value) {
  return String(value ?? "").toLowerCase();
}

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
