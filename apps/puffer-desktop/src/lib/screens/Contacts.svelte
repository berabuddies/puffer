<script lang="ts">
  import "../design/tasks.css";

  import { onDestroy, onMount } from "svelte";
  import {
    deleteContact,
    inferContacts,
    loadContacts,
    saveContact,
    subscribeContactInferEvents,
    type ContactInferTracePayload
  } from "../api/desktop";
  import MessageBody from "../components/MessageBody.svelte";
  import { contactIdsKey, normalizeContactIds } from "../contactIds";
  import Icon from "../design/Icon.svelte";
  import ToolCard from "./agent/ToolCard.svelte";
  import type {
    ContactProposal,
    ContactsSnapshot,
    MessageTimelineItem,
    SavedContact,
    ToolTimelineItem
  } from "../types";

  type ContactDialogMode = "create" | "edit";
  type ContactInferTraceItem = MessageTimelineItem | ToolTimelineItem;
  type ContactInferTraceRole = Extract<ContactInferTracePayload, { type: "message" }>["role"];

  const CONTACT_PAGE_SIZE = 40;

  let loading = $state(false);
  let inferring = $state(false);
  let saving = $state(false);
  let deletingId = $state<string | null>(null);
  let error = $state<string | null>(null);
  let inferError = $state<string | null>(null);
  let notice = $state("");
  let query = $state("");
  let snapshot = $state<ContactsSnapshot>({ contacts: [], candidates: [], proposals: [] });
  let proposals = $state<ContactProposal[]>([]);
  let inferTraceCollapsed = $state(true);
  let inferTraceItems = $state<ContactInferTraceItem[]>([]);
  let inferTraceUnsubscribe: (() => void) | null = null;
  let selectedContactId = $state<string | null>(null);
  let showContactDialog = $state(false);
  let showInferDialog = $state(false);
  let contactDialogMode = $state<ContactDialogMode>("create");
  let contactSelectionDismissed = $state(false);
  let editingId = $state<string | null>(null);
  let name = $state("");
  let description = $state("");
  let avatar = $state("");
  let contactIdsText = $state("");
  let visibleContactCount = $state(CONTACT_PAGE_SIZE);
  let contactListSentinel: HTMLDivElement | null = $state(null);
  let contactWindowKey = "";

  let visibleContacts = $derived(filteredContacts());
  let renderedContacts = $derived(visibleContacts.slice(0, visibleContactCount));
  let hasMoreContacts = $derived(renderedContacts.length < visibleContacts.length);
  let remainingContactCount = $derived(Math.max(0, visibleContacts.length - renderedContacts.length));
  let selectedContact = $derived(selectedContactValue());
  let totalContactIds = $derived(
    snapshot.contacts.reduce((sum, contact) => sum + normalizeContactIds(contact.contact_ids).length, 0)
  );
  let describedCount = $derived(snapshot.contacts.filter((contact) => contact.description.trim()).length);
  let avatarCount = $derived(snapshot.contacts.filter((contact) => contact.avatar?.trim()).length);

  onMount(() => {
    void refresh();
  });

  onDestroy(() => {
    inferTraceUnsubscribe?.();
    inferTraceUnsubscribe = null;
  });

  $effect(() => {
    const nextKey = contactRenderWindowKey(visibleContacts);
    if (nextKey === contactWindowKey) return;
    contactWindowKey = nextKey;
    visibleContactCount = initialContactRenderCount();
  });

  $effect(() => {
    const sentinel = contactListSentinel;
    if (!sentinel || !hasMoreContacts || typeof IntersectionObserver === "undefined") return;
    const root = sentinel.closest(".pf-tasks-list");
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          loadMoreContacts();
        }
      },
      { root, rootMargin: "360px 0px", threshold: 0.01 }
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  });

  $effect(() => {
    if (visibleContactCount <= visibleContacts.length) return;
    visibleContactCount = Math.max(CONTACT_PAGE_SIZE, visibleContacts.length);
  });

  $effect(() => {
    if (visibleContacts.length === 0) {
      selectedContactId = null;
      contactSelectionDismissed = false;
      return;
    }
    if (selectedContactId && !visibleContacts.some((contact) => contact.id === selectedContactId)) {
      selectedContactId = contactSelectionDismissed ? null : visibleContacts[0].id;
      return;
    }
    if (!selectedContactId && !contactSelectionDismissed) {
      selectedContactId = visibleContacts[0].id;
    }
  });

  async function refresh() {
    loading = true;
    error = null;
    try {
      applyContactsSnapshot(await loadContacts(80));
    } catch (err) {
      error = `Could not load contacts: ${messageOf(err)}`;
    } finally {
      loading = false;
    }
  }

  function applyContactsSnapshot(next: Partial<ContactsSnapshot>) {
    snapshot = {
      contacts: next.contacts ?? snapshot.contacts,
      candidates: next.candidates ?? snapshot.candidates,
      proposals: next.proposals ?? []
    };
    proposals = snapshot.proposals;
  }

  function filteredContacts(): SavedContact[] {
    const terms = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
    if (terms.length === 0) return snapshot.contacts;
    return snapshot.contacts.filter((contact) => {
      const haystack = [
        contact.id,
        contact.name,
        contact.description,
        contact.avatar ?? "",
        contact.contact_ids.join(" ")
      ].join(" ").toLowerCase();
      return terms.every((term) => haystack.includes(term));
    });
  }

  function contactRenderWindowKey(contacts: SavedContact[]): string {
    return [
      query.trim().toLowerCase(),
      contacts.map((contact) => contact.id).join("\u0001")
    ].join("\u0002");
  }

  function initialContactRenderCount(): number {
    const selectedIndex = selectedContactId
      ? visibleContacts.findIndex((contact) => contact.id === selectedContactId)
      : -1;
    const minimum = selectedIndex >= 0 ? selectedIndex + 1 : CONTACT_PAGE_SIZE;
    return Math.min(Math.max(CONTACT_PAGE_SIZE, minimum), visibleContacts.length);
  }

  function loadMoreContacts() {
    visibleContactCount = Math.min(visibleContacts.length, visibleContactCount + CONTACT_PAGE_SIZE);
  }

  function selectedContactValue(): SavedContact | null {
    if (!selectedContactId) return null;
    return visibleContacts.find((contact) => contact.id === selectedContactId) ?? null;
  }

  function openCreateDialog(proposal?: ContactProposal) {
    contactDialogMode = "create";
    editingId = null;
    name = proposal?.name ?? "";
    description = proposal?.description ?? "";
    avatar = proposal?.avatar ?? "";
    contactIdsText = (proposal?.contact_ids ?? []).join("\n");
    showContactDialog = true;
  }

  function openEditDialog(contact: SavedContact) {
    contactDialogMode = "edit";
    editingId = contact.id;
    name = contact.name;
    description = contact.description;
    avatar = contact.avatar ?? "";
    contactIdsText = contact.contact_ids.join("\n");
    showContactDialog = true;
  }

  function closeContactDialog() {
    showContactDialog = false;
  }

  function openInferDialog() {
    showInferDialog = true;
    inferError = null;
  }

  function closeInferDialog() {
    showInferDialog = false;
  }

  function closeContactDialogFromBackdrop(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      closeContactDialog();
    }
  }

  function closeInferDialogFromBackdrop(event: MouseEvent) {
    if (event.target === event.currentTarget) {
      closeInferDialog();
    }
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (showInferDialog) {
      closeInferDialog();
      return;
    }
    if (showContactDialog) {
      closeContactDialog();
    }
  }

  async function runInference() {
    if (inferring) return;
    const traceId = newTraceId();
    inferring = true;
    inferError = null;
    inferTraceCollapsed = true;
    inferTraceItems = [];
    inferTraceUnsubscribe?.();
    inferTraceUnsubscribe = null;
    try {
      inferTraceUnsubscribe = await subscribeContactInferEvents(traceId, applyInferTraceEvent);
      const result = await inferContacts(30, traceId);
      applyContactsSnapshot(result);
      notice = proposals.length === 0
        ? "No proposals returned."
        : `Inferred ${proposals.length} proposal${proposals.length === 1 ? "" : "s"}.`;
    } catch (err) {
      inferError = `Could not infer contacts: ${messageOf(err)}`;
    } finally {
      inferTraceUnsubscribe?.();
      inferTraceUnsubscribe = null;
      inferring = false;
    }
  }

  function applyInferTraceEvent(event: ContactInferTracePayload) {
    if (event.type === "message") {
      upsertInferTraceItem({
        id: event.id,
        kind: event.role === "user" ? "user" : event.role === "system" ? "system" : "assistant",
        title: event.title || traceRoleTitle(event.role),
        summary: traceSummary(event.body),
        body: event.body,
        meta: [traceRoleTitle(event.role)],
        createdAtMs: event.createdAtMs ?? null
      });
      return;
    }

    const inputText = traceValueText(event.input);
    const outputText = event.status === "running" ? "" : traceValueText(event.output);
    upsertInferTraceItem({
      id: event.id,
      kind: "tool",
      title: event.title || `Tool call: ${event.toolName}`,
      summary: event.summary || traceSummary(outputText || inputText),
      body: outputText,
      meta: [],
      createdAtMs: event.createdAtMs ?? null,
      toolName: event.toolName,
      status: event.status,
      input: inputText,
      output: outputText,
      inputJson: traceRecord(event.input)
    });
  }

  function upsertInferTraceItem(item: ContactInferTraceItem) {
    const existingIndex = inferTraceItems.findIndex((existing) => existing.id === item.id);
    if (existingIndex === -1) {
      inferTraceItems = [...inferTraceItems, item];
      return;
    }
    const next = [...inferTraceItems];
    next[existingIndex] = item;
    inferTraceItems = next;
  }

  function newTraceId(): string {
    if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
      return crypto.randomUUID();
    }
    return `${Date.now()}-${Math.random().toString(16).slice(2)}`;
  }

  function traceRoleTitle(role: ContactInferTraceRole): string {
    if (role === "user") return "User";
    if (role === "system") return "System";
    return "Assistant";
  }

  function traceSummary(value: string): string {
    const compact = value.replace(/\s+/g, " ").trim();
    if (!compact) return "No content.";
    return compact.length > 120 ? `${compact.slice(0, 119)}...` : compact;
  }

  function traceRecord(value: unknown): Record<string, unknown> | null {
    if (typeof value !== "object" || value === null || Array.isArray(value)) return null;
    return value as Record<string, unknown>;
  }

  function traceValueText(value: unknown): string {
    if (value === null || value === undefined) return "";
    if (typeof value === "string") return value;
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }

  function useProposal(proposal: ContactProposal) {
    showInferDialog = false;
    openCreateDialog(proposal);
  }

  async function submitContact(event?: SubmitEvent) {
    event?.preventDefault();
    const ids = parsedContactIds();
    const trimmedName = name.trim();
    if (!trimmedName || ids.length === 0 || saving) return;
    saving = true;
    error = null;
    try {
      const next = await saveContact({
        id: editingId ?? undefined,
        name: trimmedName,
        description: description.trim(),
        avatar: avatar.trim() || null,
        contact_ids: ids
      });
      applyContactsSnapshot(next);
      contactSelectionDismissed = false;
      selectedContactId = editingId ?? findSavedContact(next.contacts, trimmedName, ids)?.id ?? next.contacts[0]?.id ?? null;
      notice = editingId ? `Updated ${trimmedName}.` : `Created ${trimmedName}.`;
      showContactDialog = false;
    } catch (err) {
      error = `Could not save contact: ${messageOf(err)}`;
    } finally {
      saving = false;
    }
  }

  async function removeContact(contact: SavedContact) {
    if (deletingId !== null || saving) return;
    deletingId = contact.id;
    error = null;
    try {
      const next = await deleteContact(contact.id);
      applyContactsSnapshot(next);
      contactSelectionDismissed = false;
      selectedContactId = next.contacts[0]?.id ?? null;
      notice = `Deleted ${contact.name}.`;
    } catch (err) {
      error = `Could not delete contact: ${messageOf(err)}`;
    } finally {
      deletingId = null;
    }
  }

  function parsedContactIds(): string[] {
    return normalizeContactIds(contactIdsText.split(/[,\n]/));
  }

  function findSavedContact(contacts: SavedContact[], contactName: string, ids: string[]): SavedContact | null {
    const wanted = contactIdsKey(ids);
    return contacts.find((contact) => contact.name === contactName && contactIdsKey(contact.contact_ids) === wanted) ?? null;
  }

  function selectContact(id: string) {
    contactSelectionDismissed = false;
    selectedContactId = id;
  }

  function closeSelectedContact() {
    contactSelectionDismissed = true;
    selectedContactId = null;
  }

  function contactIdCountLabel(contact: SavedContact): string {
    const count = normalizeContactIds(contact.contact_ids).length;
    return count === 1 ? "1 id" : `${count} ids`;
  }

  function primaryContactId(contact: SavedContact): string {
    return normalizeContactIds(contact.contact_ids)[0] ?? "no-id";
  }

  function remainingContactIdLabel(contact: SavedContact): string {
    const count = normalizeContactIds(contact.contact_ids).length;
    if (count <= 1) return "primary";
    return `+${count - 1} more`;
  }

  function contactDescription(contact: SavedContact): string {
    return contact.description.trim() || "No description.";
  }

  function avatarLabel(contact: SavedContact): string {
    return contact.avatar?.trim() ? "Avatar saved" : "No avatar";
  }

  function avatarSource(value?: string | null): string | null {
    const trimmed = value?.trim();
    return trimmed ? trimmed : null;
  }

  function contactInitials(value: string): string {
    const parts = value
      .trim()
      .split(/\s+/)
      .filter(Boolean)
      .slice(0, 2);
    const initials = parts.map((part) => part[0]?.toUpperCase() ?? "").join("");
    return initials || "?";
  }

  function messageOf(err: unknown): string {
    return err instanceof Error ? err.message : String(err);
  }
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<div class="pf-tasks pf-contacts">
  <div class="pf-tasks-top">
    <div class="pf-tasks-title">
      <h1>Contacts {snapshot.contacts.length}</h1>
      <span>{notice}</span>
    </div>
    <div class="pf-tasks-top-right">
      <button
        type="button"
        class="sc-btn"
        data-variant="solid"
        data-size="sm"
        aria-haspopup="dialog"
        aria-expanded={showContactDialog}
        onclick={() => openCreateDialog()}
      >
        <Icon name="plus" size={12} />New
      </button>
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        aria-haspopup="dialog"
        aria-expanded={showInferDialog}
        onclick={openInferDialog}
      >
        <Icon name="sparkles" size={12} />Infer
      </button>
      <label class="pf-tasks-search">
        <Icon name="search" size={12} />
        <input
          aria-label="Search contacts"
          placeholder="Search contacts"
          bind:value={query}
        />
      </label>
      <button
        type="button"
        class="sc-btn"
        data-variant="ghost"
        data-size="sm"
        aria-label="Refresh contacts"
        aria-busy={loading}
        disabled={loading}
        onclick={() => void refresh()}
      >
        <Icon name="refresh" size={12} />{loading ? "Refreshing" : "Refresh"}
      </button>
    </div>
  </div>

  <div class="pf-tasks-summary" aria-label="Contact summary">
    <div>
      <strong>{snapshot.contacts.length}</strong>
      <span>saved</span>
    </div>
    <div>
      <strong>{visibleContacts.length}</strong>
      <span>shown</span>
    </div>
    <div>
      <strong>{totalContactIds}</strong>
      <span>ids</span>
    </div>
    <div>
      <strong>{describedCount}</strong>
      <span>notes</span>
    </div>
    <div>
      <strong>{avatarCount}</strong>
      <span>avatars</span>
    </div>
    <div>
      <strong>{proposals.length}</strong>
      <span>proposals</span>
    </div>
  </div>

  {#if error}
    <div class="pf-tasks-error">{error}</div>
  {/if}

  <div class="pf-tasks-workspace" data-inspector={selectedContact !== null}>
    <section class="pf-tasks-list-panel">
      <div class="pf-tasks-list-head">
        <strong>{visibleContacts.length} shown</strong>
        <span>saved contacts</span>
      </div>
      <div class="pf-tasks-list" aria-label="Contact list">
        {#if loading && snapshot.contacts.length === 0}
          <div class="pf-tasks-empty">Loading contacts...</div>
        {:else if visibleContacts.length === 0}
          <div class="pf-tasks-empty">
            {snapshot.contacts.length === 0 ? "No saved contacts yet." : "No contacts match the current search."}
          </div>
        {:else}
          {#each renderedContacts as contact (contact.id)}
            {@const contactAvatar = avatarSource(contact.avatar)}
            <article
              class="pf-task-row"
              data-selected={selectedContactId === contact.id}
            >
              <button
                type="button"
                class="pf-task-row-main"
                aria-pressed={selectedContactId === contact.id}
                onclick={() => selectContact(contact.id)}
              >
                <span class="pf-contact-row-content">
                  <span class="pf-contact-avatar" data-empty={contactAvatar === null} aria-hidden="true">
                    {#if contactAvatar}
                      <img src={contactAvatar} alt="" loading="lazy" />
                    {:else}
                      <span>{contactInitials(contact.name)}</span>
                    {/if}
                  </span>
                  <span class="pf-contact-row-copy">
                    <span class="pf-task-row-title">
                      <span class="pf-task-source">contact</span>
                      <strong>{contact.name}</strong>
                      <span class="pf-task-status">{contactIdCountLabel(contact)}</span>
                    </span>
                    <span class="pf-task-row-summary">{contactDescription(contact)}</span>
                    <span class="pf-task-meta">
                      <code>{primaryContactId(contact)}</code>
                      <span>{remainingContactIdLabel(contact)}</span>
                      <span>{avatarLabel(contact)}</span>
                    </span>
                  </span>
                </span>
              </button>
              <div class="pf-task-actions">
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="outline"
                  data-size="sm"
                  disabled={saving || deletingId !== null}
                  onclick={() => openEditDialog(contact)}
                >
                  <Icon name="edit" size={12} />Edit
                </button>
                <button
                  type="button"
                  class="sc-btn"
                  data-variant="ghost"
                  data-size="sm"
                  disabled={saving || deletingId !== null}
                  onclick={() => void removeContact(contact)}
                >
                  <Icon name="trash" size={12} />{deletingId === contact.id ? "Deleting" : "Delete"}
                </button>
              </div>
            </article>
          {/each}
          {#if hasMoreContacts}
            <div class="pf-task-lazy-sentinel" bind:this={contactListSentinel}>
              <button
                type="button"
                class="sc-btn"
                data-variant="outline"
                data-size="sm"
                onclick={loadMoreContacts}
              >
                Load {remainingContactCount} more contact{remainingContactCount === 1 ? "" : "s"}
              </button>
            </div>
          {/if}
        {/if}
      </div>
    </section>

    {#if selectedContact}
      {@const detailContact = selectedContact}
      {@const detailAvatar = avatarSource(detailContact.avatar)}
      <aside class="pf-task-detail" aria-label="Selected contact">
        <header class="pf-task-detail-head">
          <div class="pf-task-detail-titlebar">
            <div class="pf-task-detail-kicker">
              <span class="pf-task-source">contact</span>
              <span class="pf-task-status">{contactIdCountLabel(detailContact)}</span>
            </div>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-label="Close selected contact"
              onclick={closeSelectedContact}
            >
              <Icon name="x" size={12} />
            </button>
          </div>
          <div class="pf-contact-detail-heading">
            <span class="pf-contact-avatar" data-size="lg" data-empty={detailAvatar === null} aria-hidden="true">
              {#if detailAvatar}
                <img src={detailAvatar} alt="" />
              {:else}
                <span>{contactInitials(detailContact.name)}</span>
              {/if}
            </span>
            <div>
              <h2>{detailContact.name}</h2>
              <p>{contactDescription(detailContact)}</p>
            </div>
          </div>
        </header>

        <section class="pf-task-detail-section">
          <div class="pf-task-detail-section-head">
            <strong>Identities</strong>
            <span>{contactIdCountLabel(detailContact)}</span>
          </div>
          <div class="pf-contact-id-list">
            {#each normalizeContactIds(detailContact.contact_ids) as id (id)}
              <code>{id}</code>
            {/each}
          </div>
        </section>

        <section class="pf-task-detail-section">
          <div class="pf-task-detail-section-head">
            <strong>Record</strong>
            <span>{detailContact.id}</span>
          </div>
          <dl class="pf-task-detail-meta">
            <div>
              <dt>Local ID</dt>
              <dd><code>{detailContact.id}</code></dd>
            </div>
            <div>
              <dt>Avatar</dt>
              <dd>{avatarLabel(detailContact)}</dd>
            </div>
          </dl>
        </section>

        <section class="pf-task-detail-section">
          <div class="pf-contact-detail-actions">
            <button
              type="button"
              class="sc-btn"
              data-variant="solid"
              data-size="sm"
              disabled={saving || deletingId !== null}
              onclick={() => openEditDialog(detailContact)}
            >
              <Icon name="edit" size={12} />Edit
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              disabled={saving || deletingId !== null}
              onclick={() => void removeContact(detailContact)}
            >
              <Icon name="trash" size={12} />{deletingId === detailContact.id ? "Deleting" : "Delete"}
            </button>
          </div>
        </section>
      </aside>
    {/if}
  </div>

  {#if showContactDialog}
    <div
      class="pf-task-config-backdrop"
      role="presentation"
      onclick={closeContactDialogFromBackdrop}
      onkeydown={() => {}}
    >
      <div
        class="pf-task-config pf-contact-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pf-contact-dialog-title"
      >
        <header class="pf-task-config-head">
          <div>
            <h2 id="pf-contact-dialog-title">{contactDialogMode === "edit" ? "Edit contact" : "Create contact"}</h2>
            <span>{contactDialogMode === "edit" ? "Update saved identity routing" : "Save a grouped connector identity"}</span>
          </div>
          <button
            type="button"
            class="sc-btn"
            data-variant="ghost"
            data-size="sm"
            aria-label="Close contact editor"
            onclick={closeContactDialog}
          >
            <Icon name="x" size={12} />
          </button>
        </header>

        <form class="pf-contact-dialog-body" onsubmit={(event) => void submitContact(event)}>
          <section class="pf-task-config-section">
            <div class="pf-contact-form-grid">
              <label>
                <span>Name</span>
                <input bind:value={name} required disabled={saving} />
              </label>
              <label>
                <span>Avatar</span>
                <input bind:value={avatar} placeholder="optional URL" disabled={saving} />
              </label>
            </div>
            <label class="pf-contact-field">
              <span>Description</span>
              <textarea class="pf-contact-description-input" bind:value={description} rows="4" disabled={saving}></textarea>
            </label>
            <label class="pf-contact-field">
              <span>Contact IDs</span>
              <textarea class="pf-contact-ids-input" bind:value={contactIdsText} rows="6" placeholder="telegram@alice&#10;google@alice@example.com" disabled={saving}></textarea>
            </label>
            <div class="pf-task-config-actions">
              <button
                type="button"
                class="sc-btn"
                data-variant="ghost"
                data-size="sm"
                onclick={closeContactDialog}
              >
                Cancel
              </button>
              <button
                type="submit"
                class="sc-btn"
                data-variant="solid"
                data-size="sm"
                disabled={saving || !name.trim() || parsedContactIds().length === 0}
              >
                <Icon name="check" size={12} />{saving ? "Saving" : contactDialogMode === "edit" ? "Update" : "Create"}
              </button>
            </div>
          </section>
        </form>
      </div>
    </div>
  {/if}

  {#if showInferDialog}
    <div
      class="pf-task-config-backdrop"
      role="presentation"
      onclick={closeInferDialogFromBackdrop}
      onkeydown={() => {}}
    >
      <div
        class="pf-task-config pf-contact-infer-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="pf-contact-infer-title"
      >
        <header class="pf-task-config-head">
          <div>
            <h2 id="pf-contact-infer-title">Infer contacts</h2>
            <span>
              {proposals.length} proposal{proposals.length === 1 ? "" : "s"}
            </span>
          </div>
          <div class="pf-contact-modal-actions">
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-busy={inferring}
              disabled={inferring}
              onclick={() => void runInference()}
            >
              <Icon name="sparkles" size={12} />{inferring ? "Inferring" : "Rerun"}
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-label="Close inferred contacts"
              onclick={closeInferDialog}
            >
              <Icon name="x" size={12} />
            </button>
          </div>
        </header>

        <section class="pf-task-config-section pf-contact-infer-body">
          {#if inferring || inferTraceItems.length > 0}
            <section class="pf-contact-trace" data-collapsed={inferTraceCollapsed}>
              <button
                type="button"
                class="pf-contact-trace-toggle"
                aria-expanded={!inferTraceCollapsed}
                onclick={() => (inferTraceCollapsed = !inferTraceCollapsed)}
              >
                <Icon name={inferTraceCollapsed ? "chevR" : "chevD"} size={12} />
                <span>{inferring ? "Inferring contacts" : "Inference trace"}</span>
                <small>{inferTraceItems.length} event{inferTraceItems.length === 1 ? "" : "s"}</small>
              </button>
              {#if !inferTraceCollapsed}
                <div class="pf-contact-trace-list">
                  {#each inferTraceItems as item (item.id)}
                    {#if item.kind === "tool"}
                      <ToolCard item={item} defaultCollapsed={false} />
                    {:else}
                      <article class="pf-contact-trace-message" data-kind={item.kind}>
                        <header>
                          <span>{item.title}</span>
                          <small>{item.kind}</small>
                        </header>
                        <MessageBody body={item.body} />
                      </article>
                    {/if}
                  {/each}
                  {#if inferring && inferTraceItems.length === 0}
                    <div class="pf-tasks-empty">Starting contact inference...</div>
                  {/if}
                </div>
              {/if}
            </section>
          {/if}

          {#if inferError}
            <div class="pf-tasks-error">{inferError}</div>
          {:else if proposals.length > 0}
            <div class="pf-contact-proposal-list">
              {#each proposals as proposal, index (`${proposal.name}-${index}`)}
                {@const proposalAvatar = avatarSource(proposal.avatar)}
                <article class="pf-contact-proposal">
                  <div class="pf-contact-proposal-main">
                    <span class="pf-contact-avatar" data-empty={proposalAvatar === null} aria-hidden="true">
                      {#if proposalAvatar}
                        <img src={proposalAvatar} alt="" loading="lazy" />
                      {:else}
                        <span>{contactInitials(proposal.name)}</span>
                      {/if}
                    </span>
                    <div>
                      <strong>{proposal.name}</strong>
                      <p>{proposal.description || "No description."}</p>
                      <div class="pf-contact-id-list">
                        {#each normalizeContactIds(proposal.contact_ids) as id (id)}
                          <code>{id}</code>
                        {/each}
                      </div>
                    </div>
                  </div>
                  <button
                    type="button"
                    class="sc-btn"
                    data-variant="outline"
                    data-size="sm"
                    disabled={inferring}
                    onclick={() => useProposal(proposal)}
                  >
                    <Icon name="plus" size={12} />Use
                  </button>
                </article>
              {/each}
            </div>
          {:else if inferring}
            <div class="pf-tasks-empty">Waiting for contacts...</div>
          {:else if proposals.length === 0}
            <div class="pf-tasks-empty">No inferred contacts yet.</div>
          {/if}
        </section>
      </div>
    </div>
  {/if}
</div>

<style>
  .pf-contacts .pf-task-source {
    background: color-mix(in oklab, var(--puffer-accent) 12%, var(--background));
    color: var(--puffer-accent);
  }

  .pf-contacts .pf-task-status {
    background: color-mix(in oklab, var(--muted) 72%, var(--background));
    color: var(--muted-foreground);
  }

  .pf-contact-id-list {
    min-width: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .pf-contact-id-list code,
  .pf-contact-proposal code {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    border-radius: 5px;
    background: color-mix(in oklab, var(--muted) 34%, transparent);
    color: var(--foreground);
    padding: 2px 6px;
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .pf-contact-row-content,
  .pf-contact-proposal-main,
  .pf-contact-detail-heading {
    min-width: 0;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 10px;
  }

  .pf-contact-row-copy,
  .pf-contact-detail-heading > div,
  .pf-contact-proposal-main > div {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .pf-contact-avatar {
    width: 34px;
    height: 34px;
    flex-shrink: 0;
    border: 1px solid color-mix(in oklab, var(--border) 78%, transparent);
    border-radius: 999px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    overflow: hidden;
    background: color-mix(in oklab, var(--puffer-accent) 10%, var(--muted));
    color: var(--foreground);
    font-size: 11px;
    font-weight: 650;
  }

  .pf-contact-avatar[data-size="lg"] {
    width: 46px;
    height: 46px;
    font-size: 13px;
  }

  .pf-contact-avatar img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }

  .pf-contact-avatar[data-empty="true"] {
    color: var(--muted-foreground);
  }

  .pf-contact-detail-actions,
  .pf-contact-modal-actions {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: 8px;
  }

  .pf-contact-dialog {
    width: min(720px, calc(100vw - 32px));
  }

  .pf-contact-dialog-body {
    min-height: 0;
    overflow: auto;
  }

  .pf-contact-form-grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 10px;
  }

  .pf-contact-form-grid label,
  .pf-contact-field {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 5px;
  }

  .pf-contact-form-grid label > span,
  .pf-contact-field > span {
    color: var(--muted-foreground);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }

  .pf-contact-dialog textarea {
    min-height: unset;
  }

  .pf-contact-description-input {
    min-height: 94px !important;
    font-family: inherit !important;
    font-size: 12px !important;
  }

  .pf-contact-ids-input {
    min-height: 132px !important;
  }

  .pf-contact-infer-dialog {
    width: min(860px, calc(100vw - 32px));
    max-height: min(760px, calc(100vh - 32px));
  }

  .pf-contact-infer-body {
    min-height: 0;
    overflow: auto;
  }

  .pf-contact-trace {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: color-mix(in oklab, var(--muted) 18%, transparent);
  }

  .pf-contact-trace-toggle {
    width: 100%;
    min-height: 38px;
    border: 0;
    border-radius: 7px;
    background: transparent;
    color: var(--foreground);
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    gap: 8px;
    align-items: center;
    padding: 8px 10px;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }

  .pf-contact-trace-toggle:hover {
    background: color-mix(in oklab, var(--muted) 34%, transparent);
  }

  .pf-contact-trace-toggle span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 650;
  }

  .pf-contact-trace-toggle small,
  .pf-contact-trace-message small {
    color: var(--muted-foreground);
    font-size: 11px;
  }

  .pf-contact-trace-list {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 0 8px 8px;
  }

  .pf-contact-trace-list :global(.pf-tool) {
    border: 1px solid var(--border);
    border-radius: 7px;
    overflow: hidden;
    background: var(--background);
  }

  .pf-contact-trace-message {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--background);
    padding: 10px;
  }

  .pf-contact-trace-message header {
    min-width: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    margin-bottom: 7px;
  }

  .pf-contact-trace-message header span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
    font-weight: 650;
  }

  .pf-contact-proposal-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-contact-proposal {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 7px;
    padding: 10px;
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 12px;
    align-items: start;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
  }

  .pf-contact-proposal-main > div {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .pf-contact-proposal strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-weight: 650;
  }

  .pf-contact-proposal p {
    margin: 0;
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 1.45;
  }

  @media (max-width: 860px) {
    .pf-contact-form-grid,
    .pf-contact-proposal {
      grid-template-columns: minmax(0, 1fr);
    }

    .pf-contact-detail-actions,
    .pf-contact-modal-actions {
      align-items: stretch;
      flex-direction: column;
    }
  }
</style>
