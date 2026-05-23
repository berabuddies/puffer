<!--
  Contact (empty state) — rendered inline by Contact.svelte when the URL
  hash contains `empty=1`. Minimal placeholder, mirrors the calm hero
  vocabulary of HomeEmpty so the empty-state set feels like a family.

  We don't add a route for this page (routes.ts is owned by another agent);
  the dev "Show empty" toggle on Contact.svelte flips the hash.
-->
<script lang="ts">
  import { Users } from "lucide-svelte";

  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  function goConnect(): void {
    pushToast("Opening app integrations…", "info");
    navigate("/apps");
  }
</script>

<section class="empty">
  <div class="empty__art" aria-hidden="true">
    <Users size={48} strokeWidth={1.25} />
  </div>
  <h2 class="empty__title text-display">No contacts yet</h2>
  <p class="empty__body">
    Connect a messaging or calendar app so Momo can surface the people in your day.
  </p>
  <button type="button" class="empty__cta" onclick={goConnect}>
    Connect an app
  </button>
</section>

<style>
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-3);
    text-align: center;
    padding: var(--space-7) var(--space-4);
  }

  .empty__art {
    width: 88px;
    height: 88px;
    border-radius: 50%;
    background: var(--color-surface-rail);
    color: var(--color-text-muted);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    margin-bottom: var(--space-3);
  }

  .empty__title {
    margin: 0;
  }

  .empty__body {
    max-width: 360px;
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    color: var(--color-text-secondary);
  }

  .empty__cta {
    margin-top: var(--space-3);
    height: var(--height-button-composer);
    padding: 0 var(--space-4);
    border-radius: var(--radius-pill);
    background: var(--color-text-primary);
    color: var(--color-surface-app);
    border: 0;
    font-family: var(--font-sans);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    transition: filter 120ms ease;
  }
  .empty__cta:hover {
    filter: brightness(1.1);
  }
</style>
