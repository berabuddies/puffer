/** Returns the daemon provider id for a user-facing provider alias. */
export function canonicalDaemonProviderId(providerId: string): string {
  const normalized = providerId.trim().toLowerCase();
  if (normalized === "codex" || normalized === "openai") return "openai";
  if (normalized === "claude" || normalized === "anthropic") return "anthropic";
  return providerId;
}

const AGENT_PROVIDER_IDS = new Set(["openai", "anthropic", "puffer"]);

/** True when a provider id can run an agent session. */
export function isAgentProviderId(providerId: string | null | undefined): boolean {
  const trimmed = providerId?.trim();
  if (!trimmed) return false;
  return AGENT_PROVIDER_IDS.has(canonicalDaemonProviderId(trimmed).toLowerCase());
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
