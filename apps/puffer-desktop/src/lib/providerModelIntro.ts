import { providerRunsWithoutAuth } from "./providerIds";
import type { ProviderSummary } from "./types";

/** Return the short model-family introduction shown in provider setup UI. */
export function providerModelIntro(provider: ProviderSummary): string {
  switch (provider.id) {
    case "anthropic":
      return "Direct Claude model access, including Pro and Max plans.";
    case "github-copilot":
      return "Coding assistance models through GitHub Copilot.";
    case "openai":
      return "Fast GPT models for capable general AI work.";
    case "google":
      return "Gemini models for quick, structured responses.";
    case "openrouter":
      return "Hosted models across popular providers.";
    case "vercel-ai-gateway":
      return "Route requests through Vercel AI Gateway.";
    case "custom":
      return "Bring any OpenAI-compatible model.";
    default:
      return providerRunsWithoutAuth(provider)
        ? "Local model endpoint for private runs."
        : "General-purpose model provider.";
  }
}
