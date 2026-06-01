<script lang="ts">
  import type {
    DraftProxyEndpoint,
    NetworkProxySettings,
    ProxyScheme,
    SanitizedProxyEndpoint,
    SettingsSnapshot
  } from "../../types";
  import { saveProxySettings, testProxy } from "../../api/desktop";
  import Icon from "../../design/Icon.svelte";
  import { focusTrap } from "../../focusTrap";
  import { normalizeProxyBypass, validateProxyBypassEntries } from "./proxyBypass";
  import {
    normalizeProxySettingsForSave,
    proxySwitchChecked,
    proxySwitchDisabled,
    removeProxyEndpoint,
    setProxyEnabled
  } from "./proxyList";
  import { proxyStatusLabel, proxyStatusState, proxyStatusTitle } from "./proxyStatus";

  type Props = {
    snapshot: SettingsSnapshot | null;
    onSaved: (snapshot: SettingsSnapshot) => void;
  };

  let props: Props = $props();

  const defaultBypass = ["localhost", "127.0.0.1", "::1", "10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];
  const emptyProxy: NetworkProxySettings = {
    enabled: false,
    selected: null,
    bypass: defaultBypass,
    proxies: [],
    lastTest: null
  };
  const schemes: ProxyScheme[] = ["http", "https", "socks5", "socks5h"];

  let saving = $state(false);
  let testingId = $state<string | null>(null);
  let error = $state<string | null>(null);
  let bypassDraft = $state(defaultBypass.join("\n"));
  let lastSyncedBypass = $state(defaultBypass.join("\n"));
  let lastTest = $state<NetworkProxySettings["lastTest"]>(null);
  let editing = $state<DraftProxyEndpoint | null>(null);
  let draftPassword = $state("");
  let draftPort = $state("");

  let proxy = $derived(props.snapshot?.networkProxy ?? emptyProxy);
  let editingExisting = $derived(proxy.proxies.some((item) => item.id === editing?.id));
  let editingTitle = $derived(editingExisting ? "Edit proxy" : "Add proxy");
  let editingValidation = $derived(editing ? proxyValidationLabel(editing) : null);
  let visibleStatus = $derived.by(() =>
    Object.fromEntries(
      proxy.proxies.map((item) => [
        item.id,
        {
          label: proxyStatusLabel(item.id, testingId, lastTest),
          state: proxyStatusState(item.id, testingId, lastTest),
          title: proxyStatusTitle(item.id, lastTest)
        }
      ])
    )
  );
  $effect(() => {
    if (!props.snapshot?.networkProxy) return;
    const nextBypass = props.snapshot.networkProxy.bypass.join("\n");
    if (nextBypass !== lastSyncedBypass) {
      bypassDraft = nextBypass;
      lastSyncedBypass = nextBypass;
    }
    lastTest = props.snapshot.networkProxy.lastTest;
  });

  function nextProxyId() {
    const existing = new Set(proxy.proxies.map((item) => item.id));
    const base = `proxy-${Date.now()}`;
    if (!existing.has(base)) return base;
    let suffix = 2;
    while (existing.has(`${base}-${suffix}`)) suffix += 1;
    return `${base}-${suffix}`;
  }

  function endpointUri(endpoint: DraftProxyEndpoint) {
    return `${endpoint.scheme}://${endpoint.host.trim()}:${endpoint.port || 0}`;
  }

  function validProxyPort(port: number) {
    return Number.isInteger(port) && port >= 1 && port <= 65535;
  }

  function portFromDraft(value: string) {
    const trimmed = value.trim();
    if (!/^\d+$/.test(trimmed)) return 0;
    const parsed = Number(trimmed);
    return Number.isInteger(parsed) ? parsed : 0;
  }

  function updateDraftPort(value: string) {
    draftPort = value;
    editing = { ...editing!, port: portFromDraft(value) };
  }

  function proxyValidationLabel(endpoint: DraftProxyEndpoint) {
    if (!endpoint.host.trim()) return "Host is required.";
    if (!validProxyPort(endpoint.port)) return "Use a port from 1 to 65535.";
    return null;
  }

  function toSaveProxySettingsInput(next: NetworkProxySettings) {
    const normalized = normalizeProxySettingsForSave(next);
    return {
      enabled: normalized.enabled,
      selected: normalized.selected,
      bypass: normalized.bypass,
      proxies: normalized.proxies.map((item) => ({
        id: item.id,
        scheme: item.scheme,
        host: item.host,
        port: item.port,
        username: item.username,
        password: null,
        keepPassword: item.hasPassword
      }))
    };
  }

  function endpointToDraft(item: SanitizedProxyEndpoint): DraftProxyEndpoint {
    return {
      id: item.id,
      scheme: item.scheme,
      host: item.host,
      port: item.port,
      username: item.username,
      password: null,
      keepPassword: item.hasPassword
    };
  }

  function draftToSanitized(endpoint: DraftProxyEndpoint, existing?: SanitizedProxyEndpoint): SanitizedProxyEndpoint {
    return {
      id: endpoint.id,
      scheme: endpoint.scheme,
      host: endpoint.host.trim(),
      port: endpoint.port,
      username: endpoint.username?.trim() || null,
      hasPassword: Boolean(endpoint.password?.length || endpoint.keepPassword || existing?.hasPassword),
      uri: endpointUri(endpoint)
    };
  }

  async function persist(next: NetworkProxySettings) {
    saving = true;
    error = null;
    try {
      const input = toSaveProxySettingsInput(next);
      const saved = await saveProxySettings(input);
      lastTest = saved.networkProxy.lastTest;
      props.onSaved(saved);
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      saving = false;
    }
  }

  function addProxy() {
    error = null;
    draftPassword = "";
    draftPort = "7890";
    editing = {
      id: nextProxyId(),
      scheme: "socks5",
      host: "127.0.0.1",
      port: 7890,
      username: null,
      password: null,
      keepPassword: false
    };
  }

  function editProxy(item: SanitizedProxyEndpoint) {
    error = null;
    draftPassword = "";
    draftPort = String(item.port);
    editing = endpointToDraft(item);
  }

  function deleteProxy(item: SanitizedProxyEndpoint) {
    if (editing?.id === item.id) closeEditor();
    if (lastTest?.proxyId === item.id) lastTest = null;
    void persist(removeProxyEndpoint(proxy, item.id));
  }

  function closeEditor() {
    editing = null;
    draftPassword = "";
    draftPort = "";
  }

  async function saveEditingProxy() {
    if (!editing) return;
    const nextDraft: DraftProxyEndpoint = {
      ...editing,
      id: editing.id.trim() || nextProxyId(),
      host: editing.host.trim(),
      port: portFromDraft(draftPort),
      username: editing.username?.trim() || null,
      password: draftPassword.trim() ? draftPassword : null,
      keepPassword: !draftPassword.trim() && Boolean(editing.keepPassword)
    };
    const existing = proxy.proxies.find((item) => item.id === nextDraft.id);
    const validation = proxyValidationLabel(nextDraft);
    if (validation) {
      error = validation;
      return;
    }
    const nextItem = draftToSanitized(nextDraft, existing);
    const nextProxies = proxy.proxies.some((item) => item.id === nextItem.id)
      ? proxy.proxies.map((item) => (item.id === nextItem.id ? nextItem : item))
      : [...proxy.proxies, nextItem];
    const nextSelected = proxy.selected ?? nextItem.id;
    const input = {
      enabled: proxy.enabled,
      selected: nextSelected,
      bypass: proxy.bypass,
      proxies: nextProxies.map((item) => ({
        id: item.id,
        scheme: item.scheme,
        host: item.host,
        port: item.port,
        username: item.username,
        password: item.id === nextDraft.id ? nextDraft.password : null,
        keepPassword: item.id === nextDraft.id ? Boolean(nextDraft.keepPassword) : item.hasPassword
      }))
    };
    saving = true;
    error = null;
    try {
      const saved = await saveProxySettings(input);
      lastTest = saved.networkProxy.lastTest;
      props.onSaved(saved);
      closeEditor();
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      saving = false;
    }
  }

  async function testSavedProxy(proxyId: string) {
    testingId = proxyId;
    error = null;
    try {
      lastTest = await testProxy({ proxyId });
    } catch (err) {
      error = err instanceof Error ? err.message : String(err);
    } finally {
      testingId = null;
    }
  }

  function updateBypassDraft(value: string) {
    bypassDraft = value;
    error = null;
  }

  function saveBypass() {
    const nextBypass = normalizeProxyBypass(bypassDraft);
    const validation = validateProxyBypassEntries(nextBypass);
    if (validation) {
      error = validation;
      return;
    }
    void persist({ ...proxy, bypass: nextBypass });
  }

  function resetBypassDefaults() {
    bypassDraft = defaultBypass.join("\n");
    void persist({ ...proxy, bypass: defaultBypass });
  }
</script>

<h2>Network</h2>
<p class="lead">Provider proxy configuration for model, discovery, and OAuth requests.</p>

{#if error}
  <div class="pf-settings-note warn">{error}</div>
{/if}

<div class="pf-settings-row">
  <div class="meta">
    <div class="label">Proxy</div>
    <div class="desc">Route provider traffic through the selected proxy.</div>
  </div>
  <input
    type="checkbox"
    class="sc-switch pf-network-switch"
    checked={proxySwitchChecked(proxy)}
    disabled={saving || proxySwitchDisabled(proxy)}
    onchange={(e) => persist(setProxyEnabled(proxy, (e.currentTarget as HTMLInputElement).checked))}
  />
</div>

<section class="pf-network-section" aria-label="Proxy list">
  <div class="pf-network-section-head">
    <div>
      <h3>Proxy list</h3>
      <p>Saved endpoints available for provider requests.</p>
    </div>
    <button type="button" class="sc-btn pf-network-add" data-variant="outline" data-size="sm" disabled={saving} onclick={addProxy}>
      <Icon name="plus" size={12} />Add proxy
    </button>
  </div>
  <div class="pf-network-list">
    {#if proxy.proxies.length === 0}
      <div class="pf-empty">No proxies added.</div>
    {:else}
      {#each proxy.proxies as item (item.id)}
        <article class="pf-network-proxy-card" data-selected={proxy.selected === item.id}>
          <label class="pf-network-proxy-main">
            <input
              type="radio"
              name="proxy"
              checked={proxy.selected === item.id}
              disabled={saving}
              onchange={() => persist({ ...proxy, selected: item.id })}
            />
            <span>
              <strong>{item.uri}</strong>
              {#if visibleStatus[item.id].label}
                <small
                  class="pf-network-status"
                  data-state={visibleStatus[item.id].state}
                  title={visibleStatus[item.id].title}
                >
                  {visibleStatus[item.id].label}
                </small>
              {/if}
              {#if item.username}
                <small class="pf-network-username">{item.username}</small>
              {/if}
            </span>
          </label>
          <div class="pf-network-proxy-actions">
            <button type="button" class="sc-btn" data-variant="outline" data-size="sm" disabled={testingId !== null} onclick={() => testSavedProxy(item.id)}>
              <Icon name="test" size={12} />{testingId === item.id ? "Testing..." : "Test"}
            </button>
            <button type="button" class="sc-btn" data-variant="outline" data-size="sm" disabled={saving} onclick={() => editProxy(item)}>
              <Icon name="edit" size={12} />Edit
            </button>
            <button type="button" class="sc-btn" data-variant="destructive" data-size="sm" disabled={saving || testingId !== null} onclick={() => deleteProxy(item)}>
              <Icon name="trash" size={12} />Delete
            </button>
          </div>
        </article>
      {/each}
    {/if}
  </div>
</section>

<section class="pf-network-section" aria-label="Bypass">
  <div class="pf-network-section-head">
    <div>
      <h3>Bypass</h3>
      <p>Hosts, IPs, and CIDR ranges that should skip the proxy.</p>
    </div>
  </div>
  <textarea
    class="sc-input pf-network-bypass"
    bind:value={bypassDraft}
    oninput={(e) => updateBypassDraft((e.currentTarget as HTMLTextAreaElement).value)}
  ></textarea>
  <div class="pf-network-actions">
    <button type="button" class="sc-btn" data-variant="outline" data-size="sm" disabled={saving} onclick={resetBypassDefaults}>
      Reset defaults
    </button>
    <button type="button" class="sc-btn" data-variant="default" data-size="sm" disabled={saving} onclick={saveBypass}>
      Save bypass
    </button>
  </div>
</section>

{#if editing}
  <div class="pf-modal-scrim pf-network-proxy-scrim" role="presentation" onclick={closeEditor} onkeydown={() => {}}>
    <div
      class="pf-modal pf-network-proxy-modal"
      role="dialog"
      aria-label={editingTitle}
      aria-modal="true"
      tabindex="-1"
      use:focusTrap
      onclick={(event) => event.stopPropagation()}
      onkeydown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          closeEditor();
        }
      }}
    >
      <form
        class="pf-network-proxy-form"
        onsubmit={(event) => {
          event.preventDefault();
          void saveEditingProxy();
        }}
      >
        <div class="pf-modal-head">
          <div class="pf-modal-title-group">
            <div class="pf-modal-title">{editingTitle}</div>
          </div>
          <button type="button" class="pf-modal-close" aria-label="Close" disabled={saving} onclick={closeEditor}>
            <Icon name="x" size={14} />
          </button>
        </div>
        <div class="pf-modal-body pf-network-proxy-body">
          <div class="pf-connector-form pf-network-form">
            <label>
              Scheme
              <select class="sc-input" value={editing.scheme} disabled={saving} onchange={(e) => (editing = { ...editing!, scheme: (e.currentTarget as HTMLSelectElement).value as ProxyScheme })}>
                {#each schemes as scheme}
                  <option value={scheme}>{scheme}</option>
                {/each}
              </select>
            </label>
            <div class="pf-network-form-grid">
              <label>
                Host
                <input class="sc-input" value={editing.host} disabled={saving} data-autofocus oninput={(e) => (editing = { ...editing!, host: (e.currentTarget as HTMLInputElement).value })} />
              </label>
              <label>
                Port
                <input
                  class="sc-input"
                  inputmode="numeric"
                  pattern="[0-9]*"
                  value={draftPort}
                  disabled={saving}
                  oninput={(e) => updateDraftPort((e.currentTarget as HTMLInputElement).value)}
                />
              </label>
            </div>
            <div class="pf-network-form-grid">
              <label>
                Username
                <input class="sc-input" value={editing.username ?? ""} disabled={saving} oninput={(e) => (editing = { ...editing!, username: (e.currentTarget as HTMLInputElement).value || null })} />
              </label>
              <label>
                Password
                <input class="sc-input" type="password" value={draftPassword} disabled={saving} placeholder={editing.keepPassword ? "Stored password unchanged" : ""} oninput={(e) => (draftPassword = (e.currentTarget as HTMLInputElement).value)} />
              </label>
            </div>
            {#if editingValidation}
              <div class="pf-connector-validation">{editingValidation}</div>
            {/if}
          </div>
        </div>
        <div class="pf-modal-foot">
          <div class="pf-modal-foot-btns">
            <button type="button" class="sc-btn" data-variant="outline" data-size="sm" disabled={saving} onclick={closeEditor}>
              Cancel
            </button>
            <button type="submit" class="sc-btn" data-variant="default" data-size="sm" disabled={saving || Boolean(editingValidation)}>
              {saving ? "Saving..." : "Save proxy"}
            </button>
          </div>
        </div>
      </form>
    </div>
  </div>
{/if}

<style>
  .pf-network-section {
    margin-top: 22px;
  }

  .pf-network-section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
    margin-bottom: 10px;
  }

  .pf-network-section-head h3 {
    margin: 0;
  }

  .pf-network-section-head p {
    margin: 3px 0 0;
    color: var(--muted-foreground);
    font-size: 12.5px;
    line-height: 1.45;
  }

  .pf-network-list {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-network-switch {
    justify-self: end;
  }

  .pf-network-proxy-card {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    gap: 12px;
    align-items: center;
    padding: 12px 14px;
  }

  .pf-network-proxy-card[data-selected="true"] {
    border-color: color-mix(in oklab, var(--puffer-accent) 42%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 5%, var(--background));
  }

  .pf-network-proxy-main {
    min-width: 0;
    display: flex;
    align-items: center;
    gap: 10px;
    cursor: pointer;
  }

  .pf-network-proxy-main span {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 2px;
  }

  .pf-network-proxy-main strong {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 13px;
    font-family: var(--font-mono);
  }

  .pf-network-status,
  .pf-network-username {
    color: var(--muted-foreground);
    font-size: 11.5px;
  }

  .pf-network-status {
    font-family: var(--font-mono);
  }

  .pf-network-status[data-state="connected"] {
    color: oklch(0.46 0.14 145);
  }

  .pf-network-status[data-state="failed"] {
    color: var(--pf-run-failed);
  }

  .pf-network-status[data-state="checking"] {
    color: var(--puffer-accent);
  }

  .pf-network-proxy-actions,
  .pf-network-actions {
    display: flex;
    gap: 8px;
    align-items: center;
    flex-wrap: wrap;
  }

  .pf-network-add {
    margin-left: auto;
    flex-shrink: 0;
  }

  .pf-network-bypass {
    width: 100%;
    min-height: 116px;
    resize: vertical;
    font-family: var(--font-mono);
    line-height: 1.45;
  }

  .pf-network-actions {
    justify-content: flex-end;
    margin-top: 10px;
  }

  .pf-network-proxy-scrim {
    padding: 28px 16px;
  }

  .pf-network-proxy-modal {
    width: min(560px, calc(100vw - 32px));
    max-height: min(720px, calc(100vh - 56px));
  }

  .pf-network-proxy-form {
    display: flex;
    flex: 1 1 auto;
    min-height: 0;
    flex-direction: column;
  }

  .pf-network-proxy-body {
    padding: 14px 16px;
  }

  .pf-network-form {
    display: flex;
    flex-direction: column;
    gap: 10px;
    min-width: 0;
    width: 100%;
  }

  .pf-network-form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--muted-foreground);
    font-size: 11.5px;
  }

  .pf-network-form-grid {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
    gap: 10px;
    min-width: 0;
  }

  .pf-network-form-grid label {
    min-width: 0;
  }

  @media (max-width: 760px) {
    .pf-network-proxy-card {
      grid-template-columns: 1fr;
    }

    .pf-network-proxy-actions,
    .pf-network-actions {
      justify-content: flex-start;
    }

    .pf-network-section-head {
      align-items: flex-start;
    }

    .pf-network-form-grid {
      grid-template-columns: minmax(0, 1fr);
    }

    .pf-network-proxy-scrim {
      align-items: stretch;
      padding: 18px 12px;
    }

    .pf-network-proxy-modal {
      width: 100%;
      max-height: calc(100vh - 36px);
    }

    .pf-network-proxy-modal .pf-modal-foot-btns {
      width: 100%;
    }
  }
</style>
