/** Returns the daemon provider id for a user-facing provider alias. */
export function canonicalDaemonProviderId(providerId: string): string {
  const normalized = providerId.trim().toLowerCase();
  if (normalized === "codex" || normalized === "openai") return "openai";
  if (normalized === "claude" || normalized === "anthropic") return "anthropic";
  return providerId;
}

const BUILTIN_AGENT_PROVIDER_IDS = new Set(["openai", "anthropic", "puffer"]);
const NON_AGENT_PROVIDER_IDS = new Set(["github"]);
const NON_AGENT_APIS = new Set(["", "oauth", "none", "disabled"]);

type ProviderCapability = {
  id: string;
  defaultApi?: string | null;
  modelCount?: number | null;
};

type ProviderAuthCapability = ProviderCapability & {
  authModes?: readonly string[] | null;
};

/** True when a provider id can run an agent session. */
export function isAgentProviderId(providerId: string | null | undefined): boolean {
  const trimmed = providerId?.trim();
  if (!trimmed) return false;
  const canonical = canonicalDaemonProviderId(trimmed).toLowerCase();
  return BUILTIN_AGENT_PROVIDER_IDS.has(canonical) && !NON_AGENT_PROVIDER_IDS.has(canonical);
}

/** True when a provider descriptor exposes an agent-capable model API. */
export function providerCanRunAgent(provider: ProviderCapability | null | undefined): boolean {
  if (!provider) return false;
  const id = provider.id.trim();
  if (!id) return false;
  const canonical = canonicalDaemonProviderId(id).toLowerCase();
  if (NON_AGENT_PROVIDER_IDS.has(canonical)) return false;
  if (BUILTIN_AGENT_PROVIDER_IDS.has(canonical)) return true;
  const api = provider.defaultApi?.trim().toLowerCase() ?? "";
  if (!NON_AGENT_APIS.has(api)) return true;
  return (provider.modelCount ?? 0) > 0;
}

/** True when a provider can run agent sessions without credentials. */
export function providerRunsWithoutAuth(provider: ProviderAuthCapability | null | undefined): boolean {
  const authModes = provider?.authModes;
  return (
    providerCanRunAgent(provider) &&
    Array.isArray(authModes) &&
    (authModes.length === 0 || authModes.some((mode) => mode.trim().toLowerCase() === "native"))
  );
}

/** True when a provider is usable for an agent in the current auth snapshot. */
export function providerIsAvailableForAgent(
  provider: ProviderAuthCapability | null | undefined,
  authenticatedProviderIds: Iterable<string | null | undefined>
): boolean {
  return (
    providerCanRunAgent(provider) &&
    (providerRunsWithoutAuth(provider) || providerIdInSet(provider?.id, authenticatedProviderIds))
  );
}

/** True when an id is backed by a known agent-capable provider descriptor. */
export function providerIdCanRunAgent(
  providerId: string | null | undefined,
  providers: Iterable<ProviderCapability | null | undefined> = []
): boolean {
  for (const provider of providers) {
    if (providerIdsEquivalent(provider?.id, providerId)) return providerCanRunAgent(provider);
  }
  return isAgentProviderId(providerId);
}

/** True when two provider ids refer to the same daemon provider. */
export function providerIdsEquivalent(
  left: string | null | undefined,
  right: string | null | undefined
): boolean {
  const leftTrimmed = left?.trim();
  const rightTrimmed = right?.trim();
  if (!leftTrimmed || !rightTrimmed) return false;
  return (
    canonicalDaemonProviderId(leftTrimmed).toLowerCase() ===
    canonicalDaemonProviderId(rightTrimmed).toLowerCase()
  );
}

/** True when `providerId` is present in `candidates`, allowing UI aliases. */
export function providerIdInSet(
  providerId: string | null | undefined,
  candidates: Iterable<string | null | undefined>
): boolean {
  for (const candidate of candidates) {
    if (providerIdsEquivalent(providerId, candidate)) return true;
  }
  return false;
}
