<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import Icon from "../design/Icon.svelte";
  import {
    installLocalModel,
    localModelStatus,
    type LocalModelCheck,
    type LocalModelEvent,
    type LocalModelStatus
  } from "../api/desktop";
  import { ensureLocalDaemonClient } from "../api/daemonClient";

  export let compact = false;
  export let onRefresh: () => void = () => {};

  let status: LocalModelStatus | null = null;
  let loading = false;
  let busy = false;
  let error: string | null = null;
  let jobId: string | null = null;
  let phase = "";
  let progress: string[] = [];
  let unsubscribe: (() => void) | null = null;

  $: ready = Boolean(status?.installed && status.configured && status.running);
  $: actionLabel = (() => {
    if (busy || status?.installing) return phase === "serve" ? "Starting…" : "Installing…";
    if (!status?.supported) return "Unsupported";
    if (ready) return "Qwen3.5 ready";
    if (status?.installed && status?.configured && !status?.running) return "Start Qwen3.5";
    if (status?.installed && !status?.configured) return "Register Qwen3.5";
    return "Install Qwen3.5";
  })();
  $: detailText = (() => {
    if (!status) return "Checks for macOS Apple Silicon support and local install state.";
    if (!status.supported) return status.reason;
    if (ready) return `Running at ${status.endpoint}`;
    if (status.installed && status.configured) return `Installed at ${status.installPath}; server is stopped.`;
    if (status.installed) return `Model exists at ${status.installPath}; provider registration is missing.`;
    return `Downloads ${status.size}, creates an isolated mlx-lm venv, registers the Puffer provider, and starts the server.`;
  })();
  $: checkedAtText = status?.checkedAtMs ? `Last checked ${formatCheckedAt(status.checkedAtMs)}` : "";

  onMount(() => {
    void refreshStatus(false);
    void subscribeProgress();
  });

  onDestroy(() => {
    unsubscribe?.();
  });

  async function refreshStatus(showDiagnostics = true) {
    loading = true;
    error = null;
    if (showDiagnostics) {
      progress = [
        "Checking local files, Python deps, provider YAML, and http://127.0.0.1:8088/v1/models…"
      ];
    }
    try {
      const next = await localModelStatus("qwen35");
      status = next;
      busy = next.installing;
      if (showDiagnostics) {
        progress = [
          `Status checked at ${formatCheckedAt(next.checkedAtMs)}.`,
          ...next.checks.map(formatCheck)
        ];
      }
    } catch (e) {
      error = (e as Error).message ?? String(e);
    } finally {
      loading = false;
    }
  }

  async function subscribeProgress() {
    try {
      const client = await ensureLocalDaemonClient();
      unsubscribe = client.on("local-model:qwen35:event", (payload) => {
        handleProgress(payload as LocalModelEvent);
      });
    } catch {
      // Preview mode can still render the card from status failures.
    }
  }

  function handleProgress(event: LocalModelEvent) {
    if (event.modelId !== "qwen3.5-0.8b") return;
    if (jobId && jobId !== "active" && event.jobId !== jobId) return;
    phase = event.phase;
    progress = [...progress.slice(-5), event.message];
    if (event.status) {
      status = event.status;
      busy = event.status.installing;
    }
    if (event.phase === "done") {
      busy = false;
      error = null;
      onRefresh();
      void refreshStatus(false);
    } else if (event.phase === "error") {
      busy = false;
      error = event.message;
      void refreshStatus(false);
    }
  }

  async function install() {
    if (busy || status?.installing || ready || status?.supported === false) return;
    busy = true;
    error = null;
    phase = "starting";
    progress = [status?.installed ? "Starting Qwen3.5 local server…" : "Starting Qwen3.5 install…"];
    try {
      const job = await installLocalModel("qwen35");
      jobId = job.jobId;
      status = job.status;
    } catch (e) {
      busy = false;
      error = (e as Error).message ?? String(e);
    }
  }

  function formatCheckedAt(value: number) {
    if (!value) return "unknown";
    return new Date(value).toLocaleTimeString([], {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit"
    });
  }

  function formatCheck(check: LocalModelCheck) {
    const prefix =
      check.state === "ok" ? "OK" :
      check.state === "warning" ? "WARN" :
      check.state === "error" ? "ERR" :
      "MISS";
    return `${prefix} ${check.label}: ${check.detail}`;
  }
</script>

<article class="pf-local-model-card" data-compact={compact} data-ready={ready}>
  <div class="pf-local-model-icon" aria-hidden="true">
    <Icon name="cpu" size={18} />
  </div>
  <div class="pf-local-model-body">
    <div class="pf-local-model-topline">
      <h3>Qwen3.5 local model</h3>
      <span class="pf-local-model-pill" data-ready={ready}>
        {ready ? "Ready" : status?.installed ? "Installed" : status?.recommended ? "Recommended" : "Optional"}
      </span>
    </div>
    <p>
      Qwen3.5-0.8B runs on-device with MLX for private, always-on behavior analysis. It is
      configured as a local Puffer provider, not as the main frontier coding model.
    </p>
    <div class="pf-local-model-meta">
      <span>{status?.size ?? "~992MB"}</span>
      <span>{status?.endpoint ?? "127.0.0.1:8088/v1"}</span>
      {#if status?.providerPath}
        <span title={status.providerPath}>provider yaml</span>
      {/if}
    </div>
    <div class="pf-local-model-detail" data-error={Boolean(error)}>
      {error ?? detailText}
    </div>
    {#if checkedAtText}
      <div class="pf-local-model-checked" title={status?.installLogPath ?? status?.serveLogPath}>
        {checkedAtText}
      </div>
    {/if}
    {#if progress.length}
      <div class="pf-local-model-progress" aria-live="polite">
        {#each progress as line, index (`${index}-${line}`)}
          <div>{line}</div>
        {/each}
      </div>
    {/if}
  </div>
  <div class="pf-local-model-actions">
    <button
      type="button"
      class="sc-btn"
      data-variant={ready ? "outline" : "default"}
      data-size="sm"
      disabled={loading || busy || status?.installing || ready || status?.supported === false}
      onclick={install}
    >
      {#if busy || status?.installing}
        <Icon name="refresh" size={13} />
      {:else if ready}
        <Icon name="check" size={13} />
      {:else}
        <Icon name="sparkles" size={13} />
      {/if}
      {actionLabel}
    </button>
    <button
      type="button"
      class="sc-btn"
      data-variant="ghost"
      data-size="sm"
      disabled={loading || busy || status?.installing}
      onclick={() => refreshStatus(true)}
      title="Check Qwen3.5 install status"
    >
      {loading ? "Checking…" : "Check status"}
    </button>
  </div>
</article>

<style>
  .pf-local-model-card {
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 24%, var(--border));
    border-radius: 14px;
    background:
      radial-gradient(circle at top left, color-mix(in oklab, var(--puffer-accent) 16%, transparent), transparent 34%),
      color-mix(in oklab, var(--background) 94%, var(--muted));
    display: grid;
    grid-template-columns: 38px minmax(0, 1fr) auto;
    gap: 12px;
    align-items: start;
    padding: 14px;
    min-height: 176px;
    box-shadow: 0 14px 30px rgba(15, 23, 42, 0.05);
  }
  .pf-local-model-card[data-compact="true"] {
    margin-top: 18px;
  }
  .pf-local-model-icon {
    width: 38px;
    height: 38px;
    border-radius: 12px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: color-mix(in oklab, var(--puffer-accent) 12%, var(--background));
    color: var(--puffer-accent);
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 30%, var(--border));
  }
  .pf-local-model-body {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .pf-local-model-topline {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }
  .pf-local-model-topline h3 {
    margin: 0;
    color: var(--foreground);
    font-size: 14px;
    line-height: 1.2;
  }
  .pf-local-model-pill {
    border-radius: 999px;
    padding: 2px 8px;
    font-size: 11px;
    color: var(--muted-foreground);
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 86%, var(--muted));
  }
  .pf-local-model-pill[data-ready="true"] {
    color: oklch(0.42 0.15 145);
    border-color: color-mix(in oklab, oklch(0.7 0.18 145) 38%, var(--border));
    background: color-mix(in oklab, oklch(0.7 0.18 145) 14%, var(--background));
  }
  .pf-local-model-body p {
    margin: 0;
    max-width: 700px;
    color: var(--muted-foreground);
    font-size: 12.5px;
    line-height: 18px;
  }
  .pf-local-model-meta {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  .pf-local-model-meta span {
    border-radius: 999px;
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 90%, var(--muted));
    color: var(--muted-foreground);
    font-size: 11px;
    font-family: var(--font-mono);
    padding: 2px 7px;
  }
  .pf-local-model-detail {
    font-size: 11.5px;
    color: var(--muted-foreground);
    line-height: 1.45;
  }
  .pf-local-model-detail[data-error="true"] {
    color: var(--destructive, #c03232);
  }
  .pf-local-model-checked {
    color: color-mix(in oklab, var(--muted-foreground) 80%, var(--foreground));
    font-family: var(--font-mono);
    font-size: 10.5px;
  }
  .pf-local-model-progress {
    border-radius: 8px;
    border: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 88%, black 2%);
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 11px;
    line-height: 1.45;
    padding: 8px;
    max-height: 116px;
    overflow: auto;
  }
  .pf-local-model-actions {
    display: flex;
    align-items: flex-start;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }
  @media (max-width: 760px) {
    .pf-local-model-card {
      grid-template-columns: 38px minmax(0, 1fr);
    }
    .pf-local-model-actions {
      grid-column: 1 / -1;
      justify-content: flex-start;
    }
  }
</style>
