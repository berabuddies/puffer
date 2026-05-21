import type { ProviderSummary, SettingsSnapshot } from "./types";

export const fallbackAgentProviders: ProviderSummary[] = [
  {
    id: "openai",
    displayName: "OpenAI",
    baseUrl: "https://api.openai.com",
    defaultApi: "openai-responses",
    modelCount: 1,
    authModes: ["api_key", "oauth"],
    sourceKind: "builtin-fallback",
    sourcePath: null
  },
  {
    id: "anthropic",
    displayName: "Anthropic",
    baseUrl: "https://api.anthropic.com",
    defaultApi: "anthropic-messages",
    modelCount: 1,
    authModes: ["api_key", "oauth"],
    sourceKind: "builtin-fallback",
    sourcePath: null
  },
  {
    id: "openrouter",
    displayName: "OpenRouter",
    baseUrl: "https://openrouter.ai/api/v1",
    defaultApi: "openai-completions",
    modelCount: 1,
    authModes: ["api_key"],
    sourceKind: "builtin-fallback",
    sourcePath: null
  }
];

/** Returns provider cards to show when the daemon registry is empty. */
export function providerCatalogForSetup(snapshot: SettingsSnapshot | null): ProviderSummary[] {
  if (snapshot?.providers.length) return snapshot.providers;
  return fallbackAgentProviders;
}

/** True when the current daemon returned no provider descriptors. */
export function usesFallbackProviderCatalog(snapshot: SettingsSnapshot | null): boolean {
  return Boolean(snapshot && snapshot.providers.length === 0);
}
