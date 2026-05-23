<!--
  Contact Detail (artboard 1QI-0).

  Layout:
    [← back pill]
    centered:
      [120 avatar]
      [name — display serif]
      [role · org — body compact]
      [email · last-active — body compact muted]
      [Send email | Send Lark]  (32px buttons; sized per artboard 1U0-0)
      [Next meeting card][Last touch card]
    composer at bottom.

  If `id` doesn't resolve a contact we show a soft empty state with a back link.
-->
<script lang="ts">
  import { ArrowLeft } from "lucide-svelte";

  import Button from "../components/common/Button.svelte";
  import Composer from "../components/shell/Composer.svelte";
  import ContactAvatar from "../components/contact/ContactAvatar.svelte";
  import { findContact } from "../data/contacts";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  interface Props {
    id?: string;
  }

  let { id }: Props = $props();

  let contact = $derived(id ? findContact(id) : undefined);
  let placeholder = $derived(
    contact ? `Show the latest 10 messages with ${contact.name}` : "Send a message"
  );
  let metaSecondary = $derived(
    contact ? [contact.role, contact.org].filter(Boolean).join(" · ") : ""
  );
  let metaTertiary = $derived(
    contact ? [contact.email, contact.lastActive].filter(Boolean).join(" · ") : ""
  );

  function goBack(): void {
    navigate("/contact");
  }

  function sendEmail(): void {
    if (!contact) return;
    pushToast(`Compose email to ${contact.name}`, "info");
  }

  function sendLark(): void {
    if (!contact) return;
    pushToast(`Open Lark chat with ${contact.name}`, "info");
  }

  function openNextMeeting(): void {
    if (!contact) return;
    pushToast(`Meeting: ${contact.nextMeeting ?? "Nothing scheduled"}`, "info");
  }

  function openLastTouch(): void {
    if (!contact) return;
    pushToast(`Last touch: ${contact.lastTouch ?? "No recent activity"}`, "info");
  }

</script>

<header class="detail-header">
  <button
    class="back-btn"
    type="button"
    aria-label="Back to contacts"
    onclick={goBack}
  >
    <ArrowLeft size={16} strokeWidth={1.75} aria-hidden="true" />
  </button>
</header>

{#if contact}
  <section class="detail">
    <div class="detail__hero">
      <ContactAvatar label={contact.avatarLabel} size="lg" />
      <h1 class="text-display detail__name">{contact.name}</h1>
      {#if metaSecondary}
        <p class="text-body-compact detail__meta">{metaSecondary}</p>
      {/if}
      {#if metaTertiary}
        <p class="text-body-compact detail__meta detail__meta--muted">{metaTertiary}</p>
      {/if}
    </div>

    <div class="detail__actions">
      <Button variant="primary" size="md" label="Send email" onclick={sendEmail} />
      <Button variant="secondary" size="md" label="Send Lark" onclick={sendLark} />
    </div>

    <div class="detail__cards">
      <button type="button" class="info-card" onclick={openNextMeeting}>
        <span class="text-eyebrow">Next meeting</span>
        <span class="text-task-title">{contact.nextMeeting ?? "Nothing scheduled"}</span>
      </button>
      <button type="button" class="info-card" onclick={openLastTouch}>
        <span class="text-eyebrow">Last touch</span>
        <span class="text-task-title">{contact.lastTouch ?? "No recent activity"}</span>
      </button>
    </div>
  </section>
{:else}
  <section class="empty">
    <p class="text-body-compact">Contact not found.</p>
    <button class="empty__link" type="button" onclick={goBack}>
      Back to Contact
    </button>
  </section>
{/if}

<div class="detail-spacer"></div>

<Composer {placeholder} />

<style>
  .detail-header {
    padding-bottom: var(--space-5);
  }

  .back-btn {
    width: var(--height-icon-block);
    height: var(--height-icon-block);
    border-radius: var(--radius-pill);
    background: var(--color-surface-app);
    border: var(--border-input);
    color: var(--color-text-primary);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    transition: background-color 120ms ease;
  }
  .back-btn:hover {
    background: var(--color-surface-rail);
  }

  .detail {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-5);
  }

  .detail__hero {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    width: 100%;
  }

  /* Tightens the gap between name → role/org → email so the stack reads
     as a single hero block (matches artboard 1TV-0's 4px text gap). */
  .detail__name {
    margin-top: var(--space-2);
    text-align: center;
  }
  .detail__meta {
    text-align: center;
    margin: 0;
  }
  .detail__meta + .detail__meta {
    margin-top: -2px; /* sit on the 4px gap from the artboard */
  }
  .detail__meta--muted {
    color: var(--color-text-muted);
  }

  .detail__actions {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .detail__cards {
    display: flex;
    width: 100%;
    gap: var(--space-4);
  }

  .info-card {
    appearance: none;
    text-align: left;
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
    padding: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: var(--border-hairline);
    cursor: pointer;
    transition: background-color 120ms ease, border-color 120ms ease;
  }
  .info-card:hover {
    background: var(--color-surface-rail);
  }
  .info-card:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-7) 0;
    color: var(--color-text-muted);
  }
  .empty__link {
    color: var(--color-text-primary);
    text-decoration: underline;
    background: none;
    border: 0;
    cursor: pointer;
    padding: 0;
    font: inherit;
  }

  .detail-spacer {
    flex: 1;
    min-height: var(--space-5);
  }
</style>
