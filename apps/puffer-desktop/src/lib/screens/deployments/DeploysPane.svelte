<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import StatePill from "./StatePill.svelte";
  import { historyFor, type DeployHistoryItem, type Deployment } from "../../data/mockDeployments";

  type Props = {
    d: Deployment;
    localHistory?: DeployHistoryItem[];
    triggerBusy?: boolean;
    onTriggerDeploy?: () => void;
  };
  let {
    d,
    localHistory = [],
    triggerBusy = false,
    onTriggerDeploy
  }: Props = $props();

  type LogRow = {
    time: string;
    level: "info" | "warn" | "error" | "cursor";
    source: string;
    message: string;
  };

  let history = $derived([...localHistory, ...historyFor(d)]);
  let selectedLogKey = $state<string | null>(null);
  let selectedLog = $derived(
    history.find((item) => `${d.id}:${item.id}` === selectedLogKey) ?? null
  );

  function selectLogs(item: DeployHistoryItem): void {
    selectedLogKey = `${d.id}:${item.id}`;
  }

  function closeLogs(): void {
    selectedLogKey = null;
  }

  function logsFor(item: DeployHistoryItem): LogRow[] {
    const base: LogRow[] = [
      { time: "+00s", level: "info", source: "deploy", message: `Starting ${item.id} for ${d.name}` },
      { time: "+04s", level: "info", source: "git", message: `Checked out ${item.branch || "main"} at ${item.commit}` },
      { time: "+11s", level: "info", source: "build", message: `Injected ${d.envCount} environment keys and ${d.integrations} integrations` }
    ];
    if (item.state === "failed") {
      return [
        ...base,
        { time: "+31s", level: "error", source: "health", message: "Health check failed before traffic shift" },
        { time: "+41s", level: "info", source: "rollback", message: "Kept previous healthy release active" }
      ];
    }
    if (item.state === "deploying") {
      return [
        ...base,
        { time: "+18s", level: "warn", source: "health", message: "Waiting for service probes to settle" },
        { time: "+22s", level: "cursor", source: "stream", message: "Streaming live deploy output" }
      ];
    }
    return [
      ...base,
      { time: "+38s", level: "info", source: "health", message: "Health checks passed" },
      { time: item.dur, level: "info", source: "traffic", message: "Promoted release to active traffic" }
    ];
  }
</script>

<div class="pf-dep-pane">
  <div class="pf-dep-pane-head">
    <div>
      <h3>Deploy history</h3>
      <p class="sub">{history.length} deploys · keeping last 50</p>
    </div>
    <button
      type="button"
      class="sc-btn"
      data-variant="outline"
      data-size="sm"
      aria-label="Trigger deploy"
      aria-busy={triggerBusy}
      disabled={triggerBusy}
      onclick={() => onTriggerDeploy?.()}
    >
      <Icon name="refresh" size={12} />{triggerBusy ? "Triggering" : "Trigger deploy"}
    </button>
  </div>
  <div class="pf-dep-history">
    {#each history as h (h.id)}
      <div class="pf-dep-history-row" data-current={h.current}>
        <span class="pf-dep-history-id mono">{h.id}</span>
        <div class="pf-dep-history-commit">
          <span class="mono">{h.commit}</span>
          <span class="sub">{h.branch} · {h.deployer}</span>
        </div>
        <StatePill state={h.state} />
        <span class="sub mono">{h.dur}</span>
        <span class="sub">{h.time}</span>
        <button
          type="button"
          class="pf-dep-ico"
          aria-label={`Logs for ${h.id}`}
          aria-pressed={selectedLog?.id === h.id}
          title={`Logs for ${h.id}`}
          onclick={() => selectLogs(h)}
        >
          <Icon name="logs" size={12} />
        </button>
      </div>
    {/each}
  </div>
  {#if selectedLog}
    <section class="pf-dep-logs-panel" aria-label={`Deploy logs for ${selectedLog.id}`}>
      <header class="pf-dep-logs-head">
        <div>
          <h4>Logs · {selectedLog.id}</h4>
          <p>{d.name} · {selectedLog.branch} · {selectedLog.deployer}</p>
        </div>
        <button type="button" class="pf-dep-ico" aria-label="Close deploy logs" onclick={closeLogs}>
          <Icon name="x" size={12} />
        </button>
      </header>
      <div class="pf-dep-logs">
        {#each logsFor(selectedLog) as line}
          <div class="pf-dep-log" data-lvl={line.level}>
            <span class="t">{line.time}</span>
            <span class="lvl">{line.level}</span>
            <span class="src">{line.source}</span>
            <span class="msg">
              {line.message}
              {#if line.level === "cursor"}
                <span class="pf-dep-log-caret" aria-hidden="true"></span>
              {/if}
            </span>
          </div>
        {/each}
      </div>
    </section>
  {/if}
</div>
