<script lang="ts">
  import Puffer from "../../design/Puffer.svelte";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import type { Deployment, MemoryItem } from "../../data/mockDeployments";

  type Props = {
    d: Deployment;
    memoryDrafts: MemoryItem[];
    onAddMemory: (item: MemoryItem) => void;
  };
  let { d, memoryDrafts, onAddMemory }: Props = $props();

  const diagnosticSourceRef = "f02ae81 diagnostic";
  const diagnosticTitle = "Node 20 keep-alive regression";
  const contextChips: { id: string; label: string; icon: IconName }[] = [
    { id: "logs", label: "logs", icon: "logs" },
    { id: "metrics", label: "metrics", icon: "cpu" },
    { id: "env", label: "env", icon: "key" },
    { id: "deploys", label: "deploys", icon: "rocket" }
  ];
  let draft = $state("");
  let localTurns = $state<{ id: number; prompt: string; response: string }[]>([]);
  let localTurnId = 0;
  let lastDeploymentId = $state<string | null>(null);
  let selectedContextIds = $state<string[]>([]);
  let memoryStatus = $state("");
  let diagnosticSaved = $derived(
    memoryDrafts.some((item) => item.source.kind === "ask" && item.source.ref === diagnosticSourceRef)
  );

  type Line = { type?: "cmd" | "dim"; text: string };
  let terminalLines = $derived<Line[]>([
    { type: "cmd", text: `puffer logs --service ${d.id} --status '>=500' --since 30m` },
    { text: "12:02:14Z  POST /subscription/update  504  timeout upstream" },
    { text: "12:02:18Z  POST /subscription/update  504  timeout upstream" },
    { text: "12:03:01Z  POST /subscription/update  500  ECONNRESET" },
    { type: "dim", text: "142 matching events · 118 on /subscription/update" }
  ]);

  let suggestions = $derived(
    d.alert
      ? [
          "Why is p95 latency up?",
          "Trace 5xx from the last hour",
          "Compare this release to the last healthy one",
          "Summarize what changed since yesterday"
        ]
      : [
          "What shipped here in the last 24h?",
          "Anything drifting from infra-as-code?",
          "Show top N slowest endpoints",
          "Summarize last failed deploy"
        ]
  );

  $effect(() => {
    if (lastDeploymentId === null) {
      lastDeploymentId = d.id;
      return;
    }
    if (d.id === lastDeploymentId) return;
    lastDeploymentId = d.id;
    draft = "";
    localTurns = [];
    selectedContextIds = [];
    memoryStatus = "";
  });

  function contextLabel(contexts: string[]): string {
    if (contexts.length === 0) return "logs, metrics, env, and deploy history";
    if (contexts.length === 1) return contexts[0];
    return `${contexts.slice(0, -1).join(", ")} and ${contexts.at(-1)}`;
  }

  function responseFor(prompt: string, contexts = selectedContextIds): string {
    return `I queued an investigation for ${d.name}: ${prompt}. I'll use ${contextLabel(contexts)} for this environment.`;
  }

  function appendLocalTurn(prompt: string, response: string): void {
    localTurnId += 1;
    localTurns = [
      ...localTurns,
      {
        id: localTurnId,
        prompt,
        response
      }
    ];
  }

  function submitDraft(): void {
    const prompt = draft.trim();
    if (!prompt) return;
    appendLocalTurn(prompt, responseFor(prompt));
    draft = "";
    selectedContextIds = [];
  }

  function runDiagnosticAction(prompt: string, response: string): void {
    appendLocalTurn(prompt, response);
  }

  function toggleContext(id: string): void {
    selectedContextIds = selectedContextIds.includes(id)
      ? selectedContextIds.filter((item) => item !== id)
      : [...selectedContextIds, id];
  }

  function handleComposerKeydown(event: KeyboardEvent): void {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.key !== "Enter" || event.shiftKey) return;
    event.preventDefault();
    submitDraft();
  }

  function saveDiagnosticMemory(): void {
    if (diagnosticSaved) return;
    const item: MemoryItem = {
      id: `ask-${d.id}-keepalive-${Date.now()}`,
      kind: "pitfall",
      title: diagnosticTitle,
      body:
        "POST /subscription/update p95 rose from 180ms to 480ms after f02ae81. Node 20 drops http.Agent keep-alive defaults; pin agent.keepAlive=true in lib/http.ts or roll back to 6f8c120 while patching.",
      source: { kind: "ask", ref: diagnosticSourceRef },
      confidence: "high",
      savedBy: "Puffer",
      time: "just now",
      tags: ["node-20", "keepalive", "performance"],
      uses: 0
    };
    onAddMemory(item);
    memoryStatus = `Saved "${diagnosticTitle}" to Memory for ${d.name}.`;
  }
</script>

<div class="pf-dep-pane pf-dep-ask">
  <div class="pf-dep-ask-thread">
    <div class="pf-dep-ask-thread-inner">
      <div class="pf-msg" data-role="user">
        <div class="pf-msg-avatar">Y</div>
        <div class="pf-msg-body">
          <div class="pf-msg-meta"><span class="name">you</span><span class="time">12:07</span></div>
          <div class="pf-msg-text"><p>Why is p95 latency up since the last deploy?</p></div>
        </div>
      </div>

      <div class="pf-msg" data-role="agent">
        <div class="pf-msg-avatar"><Puffer size={26} state="thinking" /></div>
        <div class="pf-msg-body">
          <div class="pf-msg-meta"><span class="name">Puffer</span><span class="time">12:07</span></div>
          <div class="pf-msg-text"><p>Pulling traces from the last 30m and diffing the build against the previous healthy release.</p></div>
          <div style="display: flex; flex-direction: column; gap: 8px; margin-top: 12px;">
            <div class="pf-tool">
              <div class="pf-tool-head">
                <span class="pf-tool-icon"><Icon name="logs" size={13} /></span>
                <span class="pf-tool-name">query_logs</span>
                <span class="pf-tool-arg">service={d.id} status&gt;=500 since=30m</span>
                <span class="pf-tool-status"><span class="dot"></span>done</span>
              </div>
              <div class="pf-tool-body">
                <div class="terminal">
                  {#each terminalLines as line, i (i)}
                    <div class={line.type === "cmd" ? "prompt" : line.type === "dim" ? "dim" : ""}>
                      {line.type === "cmd" ? `$ ${line.text}` : line.text}
                    </div>
                  {/each}
                </div>
              </div>
            </div>
            <div class="pf-tool">
              <div class="pf-tool-head">
                <span class="pf-tool-icon"><Icon name="cpu" size={13} /></span>
                <span class="pf-tool-name">read_metric</span>
                <span class="pf-tool-arg">p95_latency · route=/subscription/update · 1h</span>
                <span class="pf-tool-status"><span class="dot"></span>done</span>
              </div>
            </div>
            <div class="pf-tool">
              <div class="pf-tool-head">
                <span class="pf-tool-icon"><Icon name="git" size={13} /></span>
                <span class="pf-tool-name">diff_commit</span>
                <span class="pf-tool-arg">f02ae81 vs 6f8c120</span>
                <span class="pf-tool-status"><span class="dot"></span>done</span>
              </div>
            </div>
          </div>
        </div>
      </div>

      <div class="pf-msg" data-role="agent">
        <div class="pf-msg-avatar"><Puffer size={26} state="idle" /></div>
        <div class="pf-msg-body">
          <div class="pf-msg-meta"><span class="name">Puffer</span><span class="time">12:08</span></div>
          <div class="pf-msg-text">
            <p>The regression isolates to <code>POST /subscription/update</code> — p95 jumped from <code>180ms</code> → <code>480ms</code> at <code>12:02 UTC</code>, exactly when <code>f02ae81</code> shipped.</p>
            <p>That commit bumped Node 18 → 20. Node 20 drops <code>http.Agent</code> keep-alive defaults, so the downstream call to <code>billing-core</code> now renegotiates TCP on every invoice fetch. I've seen this one before on <code>puffer-web</code> — saving it to memory as a recurring pitfall.</p>
            <p>Two ways out: open a PR to pin <code>agent.keepAlive=true</code> in <code>lib/http.ts</code>, or roll back to <code>6f8c120</code> while we patch forward.</p>
          </div>
          <div class="pf-dep-ask-actions">
            <button
              type="button"
              class="sc-btn"
              data-variant="default"
              data-size="sm"
              onclick={() =>
                runDiagnosticAction(
                  "Open fix PR",
                  `I drafted the fix path for ${d.name}: set agent.keepAlive=true in lib/http.ts, add a regression around subscription update latency, and open the PR against the active branch.`
                )}
            >
              <Icon name="wrench" size={12} />Open fix PR
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="outline"
              data-size="sm"
              onclick={() =>
                runDiagnosticAction(
                  "Roll back to 6f8c120",
                  `I staged the rollback plan for ${d.name}: redeploy 6f8c120 now, keep f02ae81 in review, and patch forward once keep-alive is restored.`
                )}
            >
              <Icon name="chevL" size={12} />Roll back to 6f8c120
            </button>
            <button
              type="button"
              class="sc-btn"
              data-variant="ghost"
              data-size="sm"
              aria-pressed={diagnosticSaved}
              disabled={diagnosticSaved}
              onclick={saveDiagnosticMemory}
            >
              <Icon name="bolt" size={12} />{diagnosticSaved ? "Saved to memory" : "Save to memory"}
            </button>
            {#if memoryStatus}
              <span class="pf-dep-ask-status" role="status" aria-live="polite">{memoryStatus}</span>
            {/if}
          </div>
        </div>
      </div>

      {#each localTurns as turn (turn.id)}
        <div class="pf-msg" data-role="user">
          <div class="pf-msg-avatar">Y</div>
          <div class="pf-msg-body">
            <div class="pf-msg-meta"><span class="name">you</span><span class="time">now</span></div>
            <div class="pf-msg-text"><p>{turn.prompt}</p></div>
          </div>
        </div>

        <div class="pf-msg" data-role="agent">
          <div class="pf-msg-avatar"><Puffer size={26} state="idle" /></div>
          <div class="pf-msg-body">
            <div class="pf-msg-meta"><span class="name">Puffer</span><span class="time">now</span></div>
            <div class="pf-msg-text"><p>{turn.response}</p></div>
          </div>
        </div>
      {/each}
    </div>
  </div>

  <div class="pf-dep-ask-composer">
    <div class="pf-dep-ask-chips">
      {#each suggestions as s, i (i)}
        <button type="button" class="pf-dep-debug-chip" onclick={() => (draft = s)}>
          <Icon name="sparkles" size={11} color="var(--puffer-accent)" />
          <span>{s}</span>
        </button>
      {/each}
    </div>
    <div class="pf-composer">
      <textarea
        placeholder={`Ask about ${d.name}...`}
        bind:value={draft}
        aria-label="Ask Puffer"
        onkeydown={handleComposerKeydown}
      ></textarea>
      <div class="pf-composer-foot">
        {#each contextChips as chip (chip.id)}
          <button
            type="button"
            class="pf-chip"
            aria-pressed={selectedContextIds.includes(chip.id)}
            onclick={() => toggleContext(chip.id)}
          >
            <Icon name={chip.icon} size={11} />{chip.label}
          </button>
        {/each}
        <span class="spacer"></span>
        <span style="font-size: 11px; color: var(--muted-foreground); font-family: var(--font-mono);">Enter to send</span>
        <button type="button" class="pf-send-btn" disabled={!draft.trim()} aria-label="Send" onclick={submitDraft}>
          <Icon name="arrowUp" size={15} />
        </button>
      </div>
    </div>
  </div>
</div>
