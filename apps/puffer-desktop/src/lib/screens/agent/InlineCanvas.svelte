<script lang="ts">
  import { onDestroy } from "svelte";
  import { updateCanvasState } from "../../api/desktop";
  import CanvasSubmitButton from "./CanvasSubmitButton.svelte";
  import InlineCanvasNode from "./InlineCanvasNode.svelte";

  type CanvasSpec = Record<string, unknown>;
  type Props = {
    spec: CanvasSpec;
    canvasId: string | null;
    sessionId: string | null;
    onSubmitCanvasState?: (message: string) => boolean | void | Promise<boolean | void>;
  };

  let { spec, canvasId, sessionId, onSubmitCanvasState }: Props = $props();

  let values = $state<Record<string, unknown>>({});
  let saveState = $state<"idle" | "saving" | "saved" | "error">("idle");
  let submitState = $state<"idle" | "submitting" | "submitted" | "error">("idle");
  let saveTimer: ReturnType<typeof setTimeout> | null = null;
  let pendingPatch: Record<string, unknown> = {};
  let lastSubmittedSignature: string | null = null;
  let statusMessage = $state<string | null>(null);
  let destroyed = false;
  const canSubmitCanvas = $derived(Boolean(onSubmitCanvasState && sessionId && canvasId));

  $effect(() => {
    values = initialValues(spec);
  });

  $effect(() => {
    if (sessionId && canvasId && Object.keys(pendingPatch).length > 0 && !saveTimer) {
      void savePendingPatch();
    }
  });

  function text(value: unknown): string {
    if (value === null || value === undefined) return "";
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean") return String(value);
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }

  function bodyNodes(): CanvasSpec[] {
    return Array.isArray(spec.body)
      ? spec.body.filter((item): item is CanvasSpec => typeof item === "object" && item !== null)
      : [];
  }

  function initialValues(root: unknown): Record<string, unknown> {
    const collected: Record<string, unknown> = {};
    collectInitial(root, collected);
    return collected;
  }

  function collectInitial(node: unknown, collected: Record<string, unknown>) {
    if (Array.isArray(node)) {
      node.forEach((item) => collectInitial(item, collected));
      return;
    }
    if (typeof node !== "object" || node === null) return;
    const record = node as CanvasSpec;
    const type = typeof record.type === "string" ? record.type : "";
    const id = typeof record.id === "string" ? record.id : "";
    if (id && ["toggle", "singleSelect", "multiSelect", "slider", "barSelect", "textInput"].includes(type)) {
      collected[id] = defaultValue(type, record);
    }
    collectInitial(record.children, collected);
    collectInitial(record.body, collected);
  }

  function defaultValue(type: string, node: CanvasSpec): unknown {
    if (node.value !== undefined) return node.value;
    if (type === "toggle") return false;
    if (type === "multiSelect") return [];
    if (type === "slider") return typeof node.min === "number" ? node.min : 0;
    if (type === "textInput") return "";
    const options = Array.isArray(node.options) ? node.options : [];
    const first = options.find((item) => typeof item === "object" && item !== null) as CanvasSpec | undefined;
    return first?.id ?? first?.label ?? null;
  }

  function setValue(id: string, value: unknown) {
    if (!id) return;
    values = { ...values, [id]: value };
    persist({ [id]: value });
  }

  function persist(patch: Record<string, unknown>) {
    pendingPatch = { ...pendingPatch, ...patch };
    submitState = "idle";
    statusMessage = null;
    if (!sessionId || !canvasId) {
      saveState = "idle";
      return;
    }
    if (saveTimer) clearTimeout(saveTimer);
    saveState = "saving";
    saveTimer = setTimeout(async () => {
      saveTimer = null;
      await savePendingPatch();
    }, 120);
  }

  async function submitCanvasState() {
    if (!onSubmitCanvasState || !canvasId || !sessionId) {
      submitState = "idle";
      statusMessage = "Canvas is still preparing.";
      return;
    }
    const signature = stableSignature(values);
    if (signature === lastSubmittedSignature) {
      submitState = "submitted";
      return;
    }
    submitState = "submitting";
    try {
      if (saveTimer) {
        clearTimeout(saveTimer);
        saveTimer = null;
      }
      const saved = await savePendingPatch();
      if (!saved) {
        submitState = "error";
        return;
      }
      const accepted = await onSubmitCanvasState(
        `Canvas ${canvasId} was updated by the user. Use CanvasState with canvasId "${canvasId}", briefly confirm the selected values back to me, and then continue from those values. Do not ask me to repeat the choices.`
      );
      if (destroyed) return;
      submitState = accepted === false ? "idle" : "submitted";
      if (accepted !== false) lastSubmittedSignature = signature;
    } catch {
      if (destroyed) return;
      submitState = "error";
      statusMessage = "Unable to submit canvas choices.";
    }
  }

  async function savePendingPatch(): Promise<boolean> {
    if (!sessionId || !canvasId || Object.keys(pendingPatch).length === 0) return saveState !== "error";
    const patchToSave = pendingPatch;
    pendingPatch = {};
    try {
      await updateCanvasState(sessionId, canvasId, patchToSave);
      if (destroyed) return false;
      saveState = "saved";
      statusMessage = null;
      return true;
    } catch (error) {
      if (destroyed) return false;
      pendingPatch = { ...patchToSave, ...pendingPatch };
      saveState = "error";
      statusMessage = errorMessage(error);
      return false;
    }
  }

  function errorMessage(error: unknown): string {
    return error instanceof Error ? error.message : String(error);
  }

  function stableSignature(value: unknown): string {
    try {
      return JSON.stringify(value);
    } catch {
      return String(Date.now());
    }
  }

  onDestroy(() => {
    destroyed = true;
    if (saveTimer) clearTimeout(saveTimer);
  });

  const meta = $derived(Array.isArray(spec.meta) ? spec.meta.map(text).filter(Boolean) : []);
</script>

<section class="inline-canvas" aria-label={`Canvas ${text(spec.title) || canvasId || ""}`}>
  <header class="inline-canvas-head">
    <div>
      <h3>{text(spec.title) || "Canvas"}</h3>
      {#if spec.summary}<p>{text(spec.summary)}</p>{/if}
    </div>
    <div class="inline-canvas-meta">
      {#each meta as item (item)}
        <span>{item}</span>
      {/each}
      {#if canvasId}<code>{canvasId}</code>{/if}
    </div>
  </header>

  <div class="inline-canvas-body">
    {#each bodyNodes() as node, index (`${text(node.type)}-${index}`)}
      <InlineCanvasNode {node} {values} onChange={setValue} />
    {/each}
    {#if onSubmitCanvasState}
      <div class="inline-canvas-selection-actions">
        <CanvasSubmitButton
          {saveState}
          {submitState}
          disabled={!canSubmitCanvas}
          message={statusMessage}
          onSubmit={submitCanvasState}
        />
      </div>
    {/if}
  </div>
</section>

<style>
  .inline-canvas {
    border-top: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    font-family: var(--font-sans);
  }
  .inline-canvas-head {
    display: flex;
    gap: 12px;
    align-items: flex-start;
    justify-content: space-between;
    padding: 12px 14px 10px;
    border-bottom: 1px solid var(--border);
  }
  .inline-canvas-head h3 {
    margin: 0;
    font-size: var(--pf-chat-detail-size);
    line-height: 1.25;
  }
  .inline-canvas-head p {
    margin: 5px 0 0;
    max-width: 760px;
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    line-height: 1.4;
  }
  .inline-canvas-meta {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 5px;
    max-width: 48%;
  }
  .inline-canvas-meta span,
  .inline-canvas-meta code {
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 2px 6px;
    color: var(--muted-foreground);
    background: color-mix(in oklab, var(--muted) 28%, transparent);
    font-family: var(--font-mono);
    font-size: var(--pf-chat-meta-size);
  }
  .inline-canvas-body {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px 14px 14px;
  }
  .inline-canvas-selection-actions {
    border-top: 1px solid var(--border);
    padding-top: 10px;
  }
  :global(.ic-section) {
    display: flex;
    flex-direction: column;
    gap: 9px;
  }
  :global(.ic-section > h4) {
    margin: 0;
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  :global(.ic-grid) {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(var(--ic-min, 190px), 1fr));
    gap: 10px;
  }
  :global(.ic-columns) {
    display: flex;
    gap: 10px;
    flex-wrap: wrap;
  }
  :global(.ic-columns > *) {
    flex: 1;
    min-width: 180px;
  }
  :global(.ic-card),
  :global(.ic-callout),
  :global(.ic-finding) {
    border: 1px solid var(--border);
    border-radius: 6px;
    background: color-mix(in oklab, var(--muted) 18%, transparent);
    padding: 10px 12px;
  }
  :global(.ic-card-title),
  :global(.ic-control-label) {
    margin-bottom: 6px;
    color: var(--foreground);
    font-size: var(--pf-chat-detail-size);
    font-weight: 600;
  }
  :global(.ic-divider) {
    width: 100%;
    border: 0;
    border-top: 1px solid var(--border);
  }
  :global(.ic-heading) {
    font-weight: 650;
  }
  :global(.ic-heading.level-1) {
    font-size: 18px;
  }
  :global(.ic-heading.level-2) {
    font-size: 15px;
  }
  :global(.ic-text) {
    color: color-mix(in oklab, var(--foreground) 82%, var(--muted-foreground));
    white-space: pre-wrap;
    line-height: 1.45;
  }
  :global(.ic-badge) {
    display: inline-flex;
    width: fit-content;
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 1px 6px;
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    font-weight: 600;
  }
  :global(.ic-metrics) {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  :global(.ic-metric) {
    color: var(--muted-foreground);
  }
  :global(.ic-metric strong) {
    color: var(--foreground);
  }
  :global(.ic-kv) {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: 4px 12px;
  }
  :global(.ic-kv span) {
    color: var(--muted-foreground);
  }
  :global(.ic-table-wrap),
  :global(.ic-heatmap) {
    overflow: auto;
  }
  :global(.ic-table) {
    width: 100%;
    border-collapse: collapse;
  }
  :global(.ic-table th),
  :global(.ic-table td) {
    border-bottom: 1px solid var(--border);
    padding: 6px 8px;
    text-align: left;
    vertical-align: top;
  }
  :global(.ic-table th) {
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    text-transform: uppercase;
  }
  :global(.ic-bars) {
    display: grid;
    gap: 7px;
  }
  :global(.ic-bar-row) {
    display: grid;
    grid-template-columns: minmax(90px, 150px) 1fr 42px;
    gap: 8px;
    align-items: center;
  }
  :global(.ic-bar-row span) {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  :global(.ic-bar-track) {
    height: 8px;
    border-radius: 999px;
    background: color-mix(in oklab, var(--muted) 45%, transparent);
    overflow: hidden;
  }
  :global(.ic-bar-fill) {
    height: 100%;
    border-radius: inherit;
    background: var(--puffer-accent);
  }
  :global(.ic-control) {
    display: grid;
    gap: 6px;
    min-width: 0;
  }
  :global(.ic-toggle) {
    display: flex;
    align-items: center;
    gap: 8px;
  }
  :global(.ic-choice-list) {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }
  :global(.ic-choice) {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 5px 8px;
    cursor: pointer;
    background: var(--background);
  }
  :global(.ic-choice.selected) {
    border-color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 12%, transparent);
  }
  :global(.ic-slider-row) {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 48px;
    gap: 10px;
    align-items: center;
  }
  :global(.ic-text-input) {
    min-width: 0;
    height: 30px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--background);
    color: var(--foreground);
    padding: 0 8px;
    font: inherit;
  }
  :global(.ic-code) {
    margin: 0;
    max-height: 260px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--background);
    padding: 8px 10px;
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
    white-space: pre-wrap;
  }
  :global(.ic-finding) {
    border-left: 3px solid var(--border);
  }
  :global(.ic-finding.severity-high),
  :global(.ic-finding.severity-critical) {
    border-left-color: var(--destructive);
  }
  :global(.ic-finding-head) {
    display: flex;
    gap: 8px;
    align-items: baseline;
  }
  :global(.ic-finding-head span) {
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    text-transform: uppercase;
  }
  :global(.ic-finding-head em) {
    margin-left: auto;
    color: var(--muted-foreground);
    font-style: normal;
    font-size: var(--pf-chat-meta-size);
  }
  :global(.ic-location) {
    margin: 4px 0;
    color: var(--puffer-accent);
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
  }
  :global(.ic-callout) {
    display: grid;
    gap: 4px;
    border-left: 3px solid var(--puffer-accent);
  }
  :global(.ic-heatmap table) {
    border-spacing: 4px;
  }
  :global(.ic-heatmap th) {
    color: var(--muted-foreground);
    text-align: right;
    font-weight: 500;
  }
  :global(.ic-heatmap td) {
    width: 24px;
    height: 22px;
    border-radius: 4px;
    background: color-mix(in oklab, var(--puffer-accent) 20%, var(--muted));
  }
  :global(.ic-unknown) {
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
  }
</style>
