import type { ModelDescriptorInfo } from "../api/desktop";

export const storyModels: ModelDescriptorInfo[] = [
  {
    id: "qwen3-coder-plus",
    displayName: "Qwen3 Coder Plus",
    provider: "worldrouter",
    api: "openai-completions",
    contextWindow: 200000,
    maxOutputTokens: 65536,
    supportsReasoning: true,
    supportsTools: true,
    isDefault: true
  },
  {
    id: "gpt-5.4",
    displayName: "GPT-5.4",
    provider: "worldrouter",
    api: "openai-completions",
    contextWindow: 200000,
    maxOutputTokens: 65536,
    supportsReasoning: true,
    supportsTools: true
  },
  {
    id: "claude-sonnet-4-6",
    displayName: "Claude Sonnet 4.6",
    provider: "worldrouter",
    api: "openai-completions",
    contextWindow: 200000,
    maxOutputTokens: 65536,
    supportsReasoning: true,
    supportsTools: true
  },
  {
    id: "vision-only-preview",
    displayName: "Vision Only Preview",
    provider: "worldrouter",
    api: "openai-completions",
    contextWindow: 128000,
    maxOutputTokens: 8192,
    supportsReasoning: false,
    supportsTools: false
  }
];
