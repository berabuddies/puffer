/**
 * Cross-page collector for onboarding selections.
 *
 * Onboarding pages (Where / Role) are separate routes, so per-page local
 * `$state` can't carry the choices forward to Done. This module-level `$state`
 * does — mirroring `sessionStore.svelte.ts` / `projectStore.svelte.ts`.
 *
 * `commitProfile()` persists the collected profile to puffer's user-level
 * global memory via the 1431 backend (`write_user_profile`). It is
 * fire-and-forget: a failure toasts but never blocks the Done -> /home hop.
 */
import { request } from "./wsClient";
import { pushToast } from "./toast.svelte";

const profile = $state<{ country: string | null; role: string | null }>({
  country: null,
  role: null
});

export function setCountry(country: string): void {
  profile.country = country;
}

export function setRole(role: string): void {
  profile.role = role;
}

export async function commitProfile(): Promise<void> {
  try {
    await request("write_user_profile", {
      country: profile.country,
      role: profile.role
    });
  } catch {
    pushToast("Couldn't save your profile — you can set it later.", "error");
  }
}
