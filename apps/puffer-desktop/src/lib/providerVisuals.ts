import type { ProviderSummary } from "./types";

type ProviderVisual = {
  accent: string;
  icon: string;
};

const SERVICE_ICON_BASE = "/service-icons";

const PROVIDER_ACCENTS: Record<string, string> = {
  anthropic: "#d97706",
  claude: "#d97706",
  openai: "#10a37f",
  codex: "#10a37f",
  google: "#1a73e8",
  "github-copilot": "#6e5494",
  custom: "#64748b",
  puffer: "#180524",
  "anthropic-bedrock": "#d97706",
  "anthropic-vertex": "#d97706",
  cerebras: "#7c3aed",
  groq: "#f97316",
  "kimi-coding": "#0ea5e9",
  "kimi-openai": "#0ea5e9",
  "llama-cpp": "#dc2626",
  lmstudio: "#1e293b",
  "minimax-cn": "#1d4ed8",
  minimax: "#1d4ed8",
  qwen35: "#0891b2",
  ollama: "#0f172a",
  openrouter: "#06b6d4",
  "vercel-ai-gateway": "#0f172a",
  vllm: "#16a34a",
  worldrouter: "#2563eb",
  xai: "#0f172a",
  zhipu: "#2563eb"
};

const PROVIDER_ICONS: Record<string, string> = {
  anthropic: "anthropic",
  claude: "anthropic",
  openai: "openai",
  codex: "codex",
  google: "google",
  "github-copilot": "github-copilot",
  custom: "settings",
  puffer: "brand-logo",
  "anthropic-bedrock": "anthropic",
  "anthropic-vertex": "anthropic",
  cerebras: "cerebras",
  groq: "groq",
  "kimi-coding": "kimi",
  "kimi-openai": "kimi",
  "llama-cpp": "llama-cpp",
  lmstudio: "lmstudio",
  "minimax-cn": "minimax",
  minimax: "minimax",
  qwen35: "qwen35",
  ollama: "ollama",
  openrouter: "openrouter",
  "vercel-ai-gateway": "vercel",
  vllm: "vllm",
  worldrouter: "worldrouter",
  xai: "xai",
  zhipu: "zhipu"
};

/** Returns the visual treatment for one provider card. */
export function providerVisual(provider: ProviderSummary): ProviderVisual {
  const icon = PROVIDER_ICONS[provider.id] ?? "ai";
  return {
    accent: PROVIDER_ACCENTS[provider.id] ?? "#475569",
    icon: icon === "brand-logo" ? "/brand-logo.svg" : `${SERVICE_ICON_BASE}/${icon}.svg`
  };
}
