<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import { deleteSecret, importChromeSecrets, saveSecret } from "../../api/desktop";
  import type { SecretSummary, SettingsSnapshot } from "../../types";

  type Props = {
    snapshot: SettingsSnapshot | null;
    daemonReachable: boolean;
    onRefresh: () => void;
  };

  let props: Props = $props();
  let form = $state({
    label: "",
    value: "",
    username: "",
    origin: ""
  });
  let saving = $state(false);
  let importing = $state(false);
  let deletingId = $state<string | null>(null);
  let error = $state<string | null>(null);
  let saved = $state<string | null>(null);

  let secrets = $derived(props.snapshot?.secrets?.items ?? []);
  let disabled = $derived(!props.daemonReachable || saving || importing || deletingId !== null);

  function sourceLabel(source: string): string {
    if (source === "chrome") return "Chrome";
    if (source === "manual") return "Manual";
    return source;
  }

  function updatedLabel(secret: SecretSummary): string {
    if (!secret.updatedAtMs) return "";
    return new Date(secret.updatedAtMs).toLocaleString();
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
        username: form.username.trim() || null,
        origin: form.origin.trim() || null
      });
      form = { label: "", value: "", username: "", origin: "" };
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
      saved = `Imported ${imported} Chrome credential${imported === 1 ? "" : "s"}${
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
        Label
        <input
          class="sc-input"
          placeholder="GitHub token"
          value={form.label}
          disabled={disabled}
          oninput={(e) => (form.label = (e.currentTarget as HTMLInputElement).value)}
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
        <Icon name="key" size={12} />{importing ? "Importing..." : "Import from Chrome"}
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

<div class="pf-mcp-list">
  {#each secrets as secret (secret.id)}
    <div class="pf-mcp-card">
      <span class="ico"><Icon name={secret.source === "chrome" ? "globe" : "lock"} size={16} /></span>
      <div>
        <div class="title">{secret.label}</div>
        <div class="desc">
          {sourceLabel(secret.source)}
          {#if secret.username} · {secret.username}{/if}
          {#if secret.origin} · {secret.origin}{/if}
          {#if updatedLabel(secret)} · updated {updatedLabel(secret)}{/if}
        </div>
      </div>
      <div class="pf-secret-id">{secret.id}</div>
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
  {#if secrets.length === 0}
    <div class="pf-empty">No secrets stored.</div>
  {/if}
</div>

<style>
  .pf-secrets-actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }

  .pf-secret-id {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  @media (max-width: 720px) {
    .pf-secrets-actions {
      justify-content: stretch;
    }

    .pf-secrets-actions .sc-btn {
      flex: 1;
    }
  }
</style>
