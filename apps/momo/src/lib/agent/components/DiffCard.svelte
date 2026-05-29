<script lang="ts">
  import Icon from "./Icon.svelte";
  import HighlightedLine from "./HighlightedLine.svelte";
  import type { DiffTimelineItem } from "../types";

  type Props = { item: DiffTimelineItem; defaultCollapsed?: boolean };
  let { item, defaultCollapsed = true }: Props = $props();

  type Row = { k: "ctx" | "add" | "del"; n: number | null; t: string };
  type Hunk = { header: string | null; rows: Row[] };

  function parseHunks(patch: string): Hunk[] {
    const hunks: Hunk[] = [];
    let current: Hunk | null = null;
    let newLine = 0;
    const lines = patch.split("\n");
    for (const line of lines) {
      if (line.startsWith("diff ") || line.startsWith("index ") || line.startsWith("---") || line.startsWith("+++")) {
        continue;
      }
      if (line.startsWith("@@")) {
        if (current) hunks.push(current);
        current = { header: line, rows: [] };
        const m = line.match(/\+(\d+)/);
        newLine = m ? parseInt(m[1], 10) : 0;
        continue;
      }
      if (!current) current = { header: null, rows: [] };
      if (line.startsWith("+") && !line.startsWith("+++")) {
        current.rows.push({ k: "add", n: newLine++, t: line.slice(1) });
      } else if (line.startsWith("-") && !line.startsWith("---")) {
        current.rows.push({ k: "del", n: null, t: line.slice(1) });
      } else {
        current.rows.push({ k: "ctx", n: newLine++, t: line.startsWith(" ") ? line.slice(1) : line });
      }
    }
    if (current) hunks.push(current);
    return hunks;
  }

  function stats(patch: string) {
    let add = 0;
    let del = 0;
    for (const line of patch.split("\n")) {
      if (line.startsWith("+") && !line.startsWith("+++")) add++;
      else if (line.startsWith("-") && !line.startsWith("---")) del++;
    }
    return { add, del };
  }

  let allHunks = $derived(parseHunks(item.diff.patch));
  let s = $derived(stats(item.diff.patch));
  function initialCollapsed(): boolean {
    return defaultCollapsed;
  }

  let collapsed = $state(initialCollapsed());

  let visibleHunks = $derived(allHunks);
</script>

<div class="pf-tool" data-collapsed={collapsed}>
  <button
    type="button"
    class="pf-tool-head"
    onclick={() => (collapsed = !collapsed)}
    aria-expanded={!collapsed}
    aria-label={collapsed ? "Expand diff" : "Collapse diff"}
  >
    <span class="pf-tool-icon"><Icon name="git" size={13} /></span>
    <span class="pf-tool-name">edit_file</span>
    <span class="pf-tool-arg" title={item.diff.title}>
      {item.diff.title} · +{s.add} −{s.del}
    </span>
    <span class="pf-tool-status" data-state="done"><span class="dot"></span>done</span>
    <span class="pf-tool-chevron" aria-hidden="true">
      <Icon name={collapsed ? "chevR" : "chevD"} size={11} />
    </span>
  </button>
  {#if !collapsed}
  <div class="pf-tool-body">
    <div class="pf-diff">
      {#each visibleHunks as h, hi (hi)}
        {#if h.header}
          <div class="hunk-hdr">{h.header}</div>
        {/if}
        {#each h.rows as r, ri (ri)}
          <div class="row {r.k}">
            <span class="gutter">{r.n ?? ""}</span>
            <span class="code"><HighlightedLine text={r.t || " "} path={item.diff.title} /></span>
          </div>
        {/each}
      {/each}
    </div>
  </div>
  {/if}
</div>

<style>
  .pf-tool-head {
    width: 100%;
    text-align: left;
    background: color-mix(in oklab, var(--muted) 50%, var(--background));
    border: 0;
    font: inherit;
    cursor: pointer;
  }
  .pf-tool-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    color: var(--muted-foreground);
    flex-shrink: 0;
    margin-left: 4px;
  }
  .pf-tool-head:hover .pf-tool-chevron {
    color: var(--foreground);
  }
  .pf-tool-more {
    all: unset;
    display: inline-flex;
    margin: 6px 12px 10px;
    padding: 2px 8px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--puffer-accent);
    cursor: pointer;
    border-radius: 4px;
  }
  .pf-tool-more:hover {
    background: color-mix(in oklab, var(--puffer-accent) 12%, transparent);
  }
</style>
