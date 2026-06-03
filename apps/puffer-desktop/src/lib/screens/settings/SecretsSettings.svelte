<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { deleteSecret, importChromeSecrets, saveSecret } from "../../api/desktop";
  import type { SecretSummary, SettingsSnapshot } from "../../types";

  type Props = {
    snapshot: SettingsSnapshot | null;
    daemonReachable: boolean;
    onRefresh: () => void;
  };

  const SECRET_PAGE_SIZE = 30;

  let props: Props = $props();
  let form = $state({
    label: "",
    description: "",
    value: "",
    username: "",
    origin: ""
  });
  let saving = $state(false);
  let importing = $state(false);
  let deletingId = $state<string | null>(null);
  let error = $state<string | null>(null);
  let saved = $state<string | null>(null);
  let searchQuery = $state("");
  let visibleSecretCount = $state(SECRET_PAGE_SIZE);
  let secretListSentinel: HTMLDivElement | null = $state(null);
  let secretIdsKey = $state("");

  let secrets = $derived(props.snapshot?.secrets?.items ?? []);
  let searchTerms = $derived(searchQuery.trim().toLowerCase().split(/\s+/).filter(Boolean));
  let filteredSecrets = $derived(
    searchTerms.length === 0
      ? secrets
      : secrets.filter((secret) => secretMatchesSearch(secret, searchTerms))
  );
  let visibleSecrets = $derived(filteredSecrets.slice(0, visibleSecretCount));
  let hasMoreSecrets = $derived(visibleSecrets.length < filteredSecrets.length);
  let remainingSecrets = $derived(Math.max(0, filteredSecrets.length - visibleSecrets.length));
  let disabled = $derived(!props.daemonReachable || saving || importing || deletingId !== null);

  $effect(() => {
    const nextKey = secretWindowKey(filteredSecrets, searchTerms);
    if (nextKey === secretIdsKey) return;
    secretIdsKey = nextKey;
    visibleSecretCount = Math.min(SECRET_PAGE_SIZE, filteredSecrets.length);
  });

  $effect(() => {
    const sentinel = secretListSentinel;
    if (!sentinel || !hasMoreSecrets) return;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          loadMoreSecrets();
        }
      },
      { root: null, rootMargin: "360px 0px", threshold: 0.01 }
    );
    observer.observe(sentinel);
    return () => observer.disconnect();
  });

  function sourceLabel(source: string): string {
    if (source === "chrome") return "Chrome";
    if (source === "manual") return "Manual";
    if (source === "agent") return "Agent";
    return source;
  }

  function updatedLabel(secret: SecretSummary): string {
    if (!secret.updatedAtMs) return "";
    return new Date(secret.updatedAtMs).toLocaleString();
  }

  function secretDescription(secret: SecretSummary): string {
    const parts = [sourceLabel(secret.source)];
    if (secret.username) parts.push(secret.username);
    if (secret.description) {
      parts.push(secret.description);
    } else if (secret.origin) {
      parts.push(secret.origin);
    }
    const updated = updatedLabel(secret);
    if (updated) parts.push(`updated ${updated}`);
    return parts.join(" · ");
  }

  function secretMatchesSearch(secret: SecretSummary, terms: string[]): boolean {
    const haystack = [
      secret.id,
      secret.label,
      sourceLabel(secret.source),
      secret.username,
      secret.description,
      secret.origin
    ]
      .filter(Boolean)
      .join("\n")
      .toLowerCase();
    return terms.every((term) => haystack.includes(term));
  }

  function secretWindowKey(items: SecretSummary[], terms: string[]): string {
    return [
      terms.join("\0"),
      items.length,
      items[0]?.id ?? "",
      items[items.length - 1]?.id ?? ""
    ].join("\u0001");
  }

  function loadMoreSecrets() {
    if (!hasMoreSecrets) return;
    visibleSecretCount = Math.min(visibleSecretCount + SECRET_PAGE_SIZE, filteredSecrets.length);
  }

  async function saveStoredSecret() {
    const label = form.label.trim();
    if (disabled || !label || !form.value) return;
    saving = true;
    error = null;
    saved = null;
    try {
      await saveSecret({
        label,
        value: form.value,
        description: form.description.trim() || null,
        username: form.username.trim() || null,
        origin: form.origin.trim() || null
      });
      form = { label: "", description: "", value: "", username: "", origin: "" };
      saved = `Saved ${label}`;
      props.onRefresh();
    } catch (e) {
      error = (e as Error).message ?? String(e);
    } finally {
      saving = false;
    }
  }

  async function deleteStoredSecret(id: string, label: string) {
    if (disabled) return;
    deletingId = id;
    error = null;
    saved = null;
    try {
      await deleteSecret(id);
      saved = `Deleted ${label}`;
      props.onRefresh();
    } catch (e) {
      error = (e as Error).message ?? String(e);
    } finally {
      deletingId = null;
    }
  }

  async function importFromChrome() {
    if (disabled || !props.snapshot?.secrets?.chromeImportSupported) return;
    importing = true;
    error = null;
    saved = null;
    try {
      const result = await importChromeSecrets();
      const { imported, skipped, errors } = result.report;
      saved = `Synced ${imported} Chrome credential${imported === 1 ? "" : "s"}${
        skipped ? `, skipped ${skipped}` : ""
      }.`;
      if (errors.length > 0) {
        error = errors.join("; ");
      }
      props.onRefresh();
    } catch (e) {
      error = (e as Error).message ?? String(e);
    } finally {
      importing = false;
    }
  }
</script>

<h2>Secrets</h2>
<p class="lead">Encrypted values agents can request as `PUFFER_SECRET_...` placeholders.</p>

{#if error}
  <div class="pf-settings-note warn">{error}</div>
{/if}
{#if saved}
  <div class="pf-settings-note">{saved}</div>
{/if}
{#if !props.daemonReachable}
  <div class="pf-settings-note">Preview mode - launch Puffer in the desktop app to edit secrets.</div>
{/if}

<div class="pf-settings-row">
  <div class="meta">
    <div class="label">Secret store</div>
    <div class="desc">Encrypted JSON with a platform-held key.</div>
  </div>
  <div class="pf-path-list">
    <div><span class="pf-path-label">store</span> <span class="pf-path-value">{props.snapshot?.secrets?.storeFile ?? "-"}</span></div>
    <div><span class="pf-path-label">key</span> <span class="pf-path-value">{props.snapshot?.secrets?.keySource ?? "-"}</span></div>
  </div>
</div>

<div class="pf-settings-row" style="align-items: start;">
  <div class="meta">
    <div class="label">Add secret</div>
    <div class="desc">Stored value is never rendered after save.</div>
  </div>
  <div class="pf-mcp-form">
    <div class="pf-mcp-form-grid">
      <label>
        Name
        <input
          class="sc-input"
          placeholder="GitHub token"
          value={form.label}
          disabled={disabled}
          oninput={(e) => (form.label = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <label>
        Description
        <input
          class="sc-input"
          placeholder="What this secret is for"
          value={form.description}
          disabled={disabled}
          oninput={(e) => (form.description = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <label>
        Username
        <input
          class="sc-input"
          placeholder="optional"
          value={form.username}
          disabled={disabled}
          oninput={(e) => (form.username = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <label>
        Origin
        <input
          class="sc-input"
          placeholder="https://example.com"
          value={form.origin}
          disabled={disabled}
          oninput={(e) => (form.origin = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
      <label>
        Value
        <input
          class="sc-input"
          type="password"
          autocomplete="off"
          value={form.value}
          disabled={disabled}
          oninput={(e) => (form.value = (e.currentTarget as HTMLInputElement).value)}
        />
      </label>
    </div>
    <div class="pf-secrets-actions">
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        disabled={disabled || !props.snapshot?.secrets?.chromeImportSupported}
        onclick={importFromChrome}
      >
        <Icon name="key" size={12} />{importing ? "Syncing..." : "Sync from Chrome"}
      </button>
      <button
        type="button"
        class="sc-btn"
        data-variant="default"
        data-size="sm"
        disabled={disabled || !form.label.trim() || !form.value}
        onclick={saveStoredSecret}
      >
        <Icon name="plus" size={12} />{saving ? "Saving..." : "Save secret"}
      </button>
    </div>
  </div>
</div>

<div class="pf-secret-list-toolbar">
  <label class="pf-secret-search">
    <Icon name="search" size={13} />
    <input
      type="search"
      placeholder="Search secrets"
      value={searchQuery}
      oninput={(e) => (searchQuery = (e.currentTarget as HTMLInputElement).value)}
    />
    {#if searchQuery.trim()}
      <button
        type="button"
        aria-label="Clear secret search"
        title="Clear search"
        onclick={() => (searchQuery = "")}
      >
        <Icon name="x" size={12} />
      </button>
    {/if}
  </label>
  <div class="pf-secret-result-count">
    {#if searchTerms.length > 0}
      Showing {filteredSecrets.length} of {secrets.length}
    {:else}
      {secrets.length} stored secret{secrets.length === 1 ? "" : "s"}
    {/if}
  </div>
</div>

<div class="pf-mcp-list">
  {#each visibleSecrets as secret (secret.id)}
    {@const details = secretDescription(secret)}
    <div class="pf-mcp-card pf-secret-card">
      <span class="ico"><Icon name={secret.source === "chrome" ? "globe" : "lock"} size={16} /></span>
      <div class="pf-secret-main">
        <div class="title pf-secret-title" title={secret.label}>{secret.label}</div>
        <div class="desc pf-secret-desc" title={details}>{details}</div>
      </div>
      <div class="pf-secret-id" title={secret.id}>{secret.id}</div>
      <button
        type="button"
        class="sc-btn"
        data-variant="ghost"
        data-size="sm"
        disabled={disabled || deletingId === secret.id}
        onclick={() => deleteStoredSecret(secret.id, secret.label)}
        aria-label={`Delete ${secret.label}`}
        title={`Delete ${secret.label}`}
      >
        <Icon name="trash" size={13} />{deletingId === secret.id ? "Deleting..." : "Delete"}
      </button>
    </div>
  {/each}
  {#if hasMoreSecrets}
    <div class="pf-secret-load-more" bind:this={secretListSentinel}>
      <button
        type="button"
        class="sc-btn"
        data-variant="outline"
        data-size="sm"
        onclick={loadMoreSecrets}
      >
        Load 30 more
      </button>
      <span>{remainingSecrets} more {searchTerms.length > 0 ? "matching " : ""}secret{remainingSecrets === 1 ? "" : "s"}</span>
    </div>
  {/if}
  {#if secrets.length === 0}
    <div class="pf-empty">No secrets stored.</div>
  {:else if filteredSecrets.length === 0}
    <div class="pf-empty">No secrets match "{searchQuery.trim()}".</div>
  {/if}
</div>

<style>
  .pf-secrets-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }

  .pf-secret-list-toolbar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin: 18px 0 10px;
  }

  .pf-secret-search {
    flex: 1 1 320px;
    min-width: 0;
    max-width: 520px;
    min-height: 36px;
    display: flex;
    align-items: center;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--background);
    padding: 0 10px;
    color: var(--muted-foreground);
  }

  .pf-secret-search:focus-within {
    border-color: color-mix(in oklab, var(--puffer-accent) 40%, var(--border));
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 12%, transparent);
  }

  .pf-secret-search input {
    min-width: 0;
    flex: 1;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--foreground);
    font: inherit;
    font-size: 13px;
  }

  .pf-secret-search button {
    width: 24px;
    height: 24px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--muted-foreground);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .pf-secret-search button:hover {
    background: var(--pf-selected-bg-hover);
    color: var(--foreground);
  }

  .pf-secret-result-count {
    flex: 0 0 auto;
    color: var(--muted-foreground);
    font-size: 12px;
    white-space: nowrap;
  }

  .pf-secret-card {
    grid-template-columns: 32px minmax(0, 1fr) minmax(84px, 160px) auto;
  }

  .pf-secret-main {
    min-width: 0;
  }

  .pf-secret-title,
  .pf-secret-desc {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .pf-secret-id {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    max-width: 160px;
  }

  .pf-secret-load-more {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    min-height: 56px;
    color: var(--muted-foreground);
    font-size: 12px;
  }

  .pf-secret-card .sc-btn {
    white-space: nowrap;
  }

  @media (max-width: 720px) {
    .pf-secrets-actions {
      justify-content: stretch;
    }

    .pf-secrets-actions .sc-btn {
      flex: 1;
    }

    .pf-secret-list-toolbar {
      align-items: stretch;
      flex-direction: column;
    }

    .pf-secret-search {
      max-width: none;
      width: 100%;
    }

    .pf-secret-result-count {
      white-space: normal;
    }

    .pf-secret-card {
      grid-template-columns: 32px minmax(0, 1fr);
      align-items: start;
    }

    .pf-secret-id,
    .pf-secret-card .sc-btn {
      grid-column: 2;
      justify-self: start;
      max-width: 100%;
    }

    .pf-secret-load-more {
      align-items: stretch;
      flex-direction: column;
    }
  }
</style>
