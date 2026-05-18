/** Returns the daemon provider id for a user-facing provider alias. */
export function canonicalDaemonProviderId(providerId: string): string {
  const normalized = providerId.trim().toLowerCase();
  if (normalized === "codex") return "openai";
  if (normalized === "claude") return "anthropic";
  return providerId;
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
