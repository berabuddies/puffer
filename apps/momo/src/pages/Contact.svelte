<!--
  Contact list (artboard 16O-0).
  Page layout: section heading → list of contact rows (16px gap) → composer.

  Dev toggle: appending `#empty=1` to the URL renders ContactEmpty inline
  (we don't own routes.ts; this lets us preview the empty state without
  touching the route table).
-->
<script lang="ts">
  import { onMount } from "svelte";

  import Composer from "../components/shell/Composer.svelte";
  import ContactRow from "../components/contact/ContactRow.svelte";
  import ContactEmpty from "./ContactEmpty.svelte";
  import { contacts as seedContacts } from "../data/contacts";
  import type { Contact } from "../data/types";
  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  // Dev-only empty-state preview. We use sessionStorage so the toggle
  // survives a soft navigation back to /contact but does not bleed across
  // browser sessions (and so dev links in other pages can prime it).
  // We can't add a /contact/empty route — routes.ts is owned by another
  // agent — so this local flag is the only way to preview the empty state.
  const EMPTY_KEY = "puffer.dev.contact.empty";

  let filter = $state<string>("");
  let isEmpty = $state<boolean>(false);

  function readEmptyFlag(): void {
    if (typeof window === "undefined") return;
    try {
      isEmpty = window.sessionStorage.getItem(EMPTY_KEY) === "1";
    } catch {
      isEmpty = false;
    }
  }

  function writeEmptyFlag(value: boolean): void {
    if (typeof window === "undefined") return;
    try {
      if (value) window.sessionStorage.setItem(EMPTY_KEY, "1");
      else window.sessionStorage.removeItem(EMPTY_KEY);
    } catch {
      /* no-op */
    }
    isEmpty = value;
  }

  onMount(() => {
    readEmptyFlag();
  });

  let visibleContacts = $derived(
    filter.trim().length === 0
      ? seedContacts
      : seedContacts.filter((c) => {
          const q = filter.trim().toLowerCase();
          return (
            c.name.toLowerCase().includes(q) ||
            c.email.toLowerCase().includes(q) ||
            c.role.toLowerCase().includes(q)
          );
        })
  );

  function openContact(contact: Contact): void {
    navigate(`/contact/${contact.id}`);
  }

  function sendEmail(contact: Contact): void {
    pushToast(`Compose email to ${contact.name}`, "info");
  }

  function sendLark(contact: Contact): void {
    pushToast(`Open Lark chat with ${contact.name}`, "info");
  }

  // Dev toggles
  function showEmpty(): void {
    writeEmptyFlag(true);
  }
  function showFull(): void {
    writeEmptyFlag(false);
    navigate("/contact");
  }
</script>

<header class="contact-header">
  <div class="contact-header__title-row">
    <h1 class="text-section">Contact</h1>
    <div class="dev-toggles" aria-label="Dev state toggles">
      <button type="button" class="dev-link" onclick={showEmpty}>Show empty</button>
      <button type="button" class="dev-link" onclick={showFull}>Show full</button>
    </div>
  </div>
  {#if !isEmpty}
    <div class="contact-header__filter">
      <input
        type="search"
        class="filter-input"
        placeholder="Search contacts…"
        bind:value={filter}
        aria-label="Filter contacts"
      />
    </div>
  {/if}
</header>

{#if isEmpty}
  <ContactEmpty />
{:else}
  <section class="contact-list">
    {#each visibleContacts as contact (contact.id)}
      <ContactRow
        {contact}
        onopen={openContact}
        onemail={sendEmail}
        onlark={sendLark}
      />
    {/each}
    {#if visibleContacts.length === 0}
      <div class="empty-divider">
        <span>No contacts match "{filter}"</span>
      </div>
    {/if}
  </section>
{/if}

<div class="contact-spacer"></div>

<Composer placeholder="Send a message to chaofan" />

<style>
  .contact-header {
    padding-bottom: var(--space-5);
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
  }

  .contact-header__title-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .contact-header__filter {
    display: flex;
  }

  .filter-input {
    width: 100%;
    max-width: 320px;
    height: var(--height-button-card);
    padding: 0 var(--space-3);
    border-radius: var(--radius-pill);
    border: var(--border-input);
    background: var(--color-surface-app);
    color: var(--color-text-primary);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    transition: border-color 120ms ease;
  }
  .filter-input:focus {
    outline: none;
    border-color: var(--color-text-primary);
  }

  .contact-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .empty-divider {
    padding: var(--space-5) 0;
    text-align: center;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: var(--font-size-body);
    line-height: var(--line-height-body);
    border-top: var(--border-hairline);
    border-bottom: var(--border-hairline);
  }

  .dev-toggles {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .dev-link {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 2px 6px;
    color: var(--color-text-muted);
    font-family: var(--font-system);
    font-size: 11px;
    line-height: 14px;
    font-weight: var(--font-weight-medium);
    cursor: pointer;
    border-radius: var(--radius-control);
    transition: background-color 120ms ease, color 120ms ease;
  }
  .dev-link:hover {
    background: var(--color-surface-rail);
    color: var(--color-text-primary);
  }

  /* Push the composer to the bottom of the column when the list is short. */
  .contact-spacer {
    flex: 1;
    min-height: var(--space-5);
  }
</style>
