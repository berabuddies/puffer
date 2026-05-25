import fs from "node:fs";

export function readOpenRouterApiKey(env = process.env) {
  const direct = String(env.OPENROUTER_API_KEY ?? "").trim();
  if (direct) return direct;
  const keyFile = String(env.PUFFER_OPENROUTER_API_KEY_FILE ?? "").trim();
  if (!keyFile) return "";
  return fs.readFileSync(keyFile, "utf8").trim();
}

export function hasOpenRouterCredential(env = process.env) {
  if (String(env.OPENROUTER_API_KEY ?? "").trim()) return true;
  const keyFile = String(env.PUFFER_OPENROUTER_API_KEY_FILE ?? "").trim();
  return Boolean(keyFile && fs.existsSync(keyFile) && fs.statSync(keyFile).size > 0);
}
