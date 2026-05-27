<!--
  ContactRow — whole-row clickable card. Mirrors the task-card recipe
  (16px padding, 16px gap, 16px radius, 1px card border, white bg) with:
    [40 avatar] [name + (role · email)] [Send email | Lark]
  Send-email / Lark buttons stopPropagation so they fire their own intent
  rather than navigating to the detail view.
-->
<script lang="ts">
  import type { Contact } from "../../data/types";
  import Button from "../common/Button.svelte";
  import ContactAvatar from "./ContactAvatar.svelte";

  interface Props {
    contact: Contact;
    onopen?: (contact: Contact) => void;
    onemail?: (contact: Contact) => void;
    onlark?: (contact: Contact) => void;
  }

  let { contact, onopen, onemail, onlark }: Props = $props();

  function handleOpen(): void {
    onopen?.(contact);
  }

  function handleKey(event: KeyboardEvent): void {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      handleOpen();
    }
  }

  function handleEmail(event: MouseEvent): void {
    event.stopPropagation();
    onemail?.(contact);
  }

  function handleLark(event: MouseEvent): void {
    event.stopPropagation();
    onlark?.(contact);
  }
</script>

<div
  class="row"
  role="button"
  tabindex="0"
  onclick={handleOpen}
  onkeydown={handleKey}
  aria-label="Open contact {contact.name}"
>
  <ContactAvatar label={contact.avatarLabel} size="sm" />

  <div class="row__body">
    <div class="row__text">
      <span class="text-task-title">{contact.name}</span>
      <span class="text-body-compact">{contact.role} · {contact.email}</span>
    </div>

    <div class="row__actions">
      <Button variant="primary" size="sm" label="Send email" onclick={handleEmail} />
      <Button variant="secondary" size="sm" label="Lark" onclick={handleLark} />
    </div>
  </div>
</div>

<style>
  .row {
    display: flex;
    align-items: center;
    width: 100%;
    padding: var(--space-4);
    gap: var(--space-4);
    border-radius: var(--radius-card);
    background: var(--color-surface-app);
    border: var(--border-hairline);
    cursor: pointer;
    transition: background-color 120ms ease, border-color 120ms ease;
    text-align: left;
  }
  .row:hover {
    background: var(--color-surface-rail);
  }
  .row:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }

  .row__body {
    display: flex;
    flex: 1;
    min-width: 0;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
  }

  .row__text {
    display: flex;
    flex-direction: column;
    min-width: 0;
    gap: 2px;
  }

  .row__actions {
    display: flex;
    align-items: center;
    flex-shrink: 0;
    gap: var(--space-2);
  }
</style>
