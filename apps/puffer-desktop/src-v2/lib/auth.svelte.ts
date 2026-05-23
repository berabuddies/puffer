/**
 * Tiny onboarding-state helper.
 *
 * The v2 shell decides between the onboarding flow and the main app by
 * reading a single localStorage flag. We wrap it here so the read/write
 * shape stays consistent (and so we can swap the backing store later
 * without touching callers).
 *
 * The functions are SSR-safe — every access guards against `window` being
 * undefined so Vite's SSR/import-time graph never throws.
 */

const STORAGE_KEY = "puffer.onboarded";

function safeStorage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    // Private mode / disabled storage — treat as "no record".
    return null;
  }
}

/** True if the user has completed (or skipped past) the onboarding flow. */
export function isOnboarded(): boolean {
  const store = safeStorage();
  if (!store) return false;
  return store.getItem(STORAGE_KEY) === "true";
}

/** Mark the user as onboarded. Idempotent — safe to call on every /home hit. */
export function markOnboarded(): void {
  const store = safeStorage();
  if (!store) return;
  if (store.getItem(STORAGE_KEY) === "true") return;
  store.setItem(STORAGE_KEY, "true");
}

/** Clear the flag — useful for dev tooling / a future "reset" affordance. */
export function resetOnboarding(): void {
  const store = safeStorage();
  if (!store) return;
  store.removeItem(STORAGE_KEY);
}
