import type { ProviderSummary } from "./types";

type ProviderVisual = {
  accent: string;
  icon: string;
};

const SERVICE_ICON_BASE = "/service-icons";

const PROVIDER_ACCENTS: Record<string, string> = {
  anthropic: "#d97706",
  openai: "#10a37f",
  "anthropic-bedrock": "#d97706",
  "anthropic-vertex": "#d97706",
  cerebras: "#7c3aed",
  groq: "#f97316",
  "kimi-coding": "#0ea5e9",
  "llama-cpp": "#dc2626",
  lmstudio: "#1e293b",
  "minimax-cn": "#1d4ed8",
  minimax: "#1d4ed8",
  ollama: "#0f172a",
  openrouter: "#06b6d4",
  "vercel-ai-gateway": "#0f172a",
  vllm: "#16a34a",
  worldagent: "#1f6feb",
  xai: "#0f172a"
};

const PROVIDER_ICONS: Record<string, string> = {
  anthropic: "llm",
  "anthropic-bedrock": "llm",
  "anthropic-vertex": "llm",
  cerebras: "ai",
  groq: "ai",
  "kimi-coding": "ai",
  "llama-cpp": "llm",
  lmstudio: "llm",
  "minimax-cn": "ai",
  minimax: "ai",
  ollama: "llm",
  openai: "openai",
  openrouter: "llm",
  "vercel-ai-gateway": "vercel",
  vllm: "llm",
  worldagent: "ai",
  xai: "ai"
};

/** Returns the visual treatment for one provider card. */
export function providerVisual(provider: ProviderSummary): ProviderVisual {
  const icon = PROVIDER_ICONS[provider.id] ?? "ai";
  return {
    accent: PROVIDER_ACCENTS[provider.id] ?? "#475569",
    icon: `${SERVICE_ICON_BASE}/${icon}.svg`
  };
}
