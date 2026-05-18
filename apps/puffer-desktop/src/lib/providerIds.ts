/** Returns the daemon provider id for a user-facing provider alias. */
export function canonicalDaemonProviderId(providerId: string): string {
  const normalized = providerId.trim().toLowerCase();
  if (normalized === "codex") return "openai";
  if (normalized === "claude") return "anthropic";
  return providerId;
}
