<script lang="ts">
  import InlineCanvasNode from "./InlineCanvasNode.svelte";

  type CanvasNode = Record<string, unknown>;
  type CanvasOption = { id?: string; label?: string };

  type Props = {
    node: CanvasNode;
    values: Record<string, unknown>;
    onChange: (id: string, value: unknown) => void;
  };

  let { node, values, onChange }: Props = $props();

  function text(value: unknown): string {
    if (value === null || value === undefined) return "";
    if (typeof value === "string") return value;
    if (typeof value === "number" || typeof value === "boolean") return String(value);
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }

  function children(value: unknown): CanvasNode[] {
    return Array.isArray(value)
      ? value.filter((item): item is CanvasNode => typeof item === "object" && item !== null)
      : [];
  }

  function options(value: unknown): CanvasOption[] {
    return Array.isArray(value)
      ? value
          .filter((item): item is CanvasNode => typeof item === "object" && item !== null)
          .map((item) => ({
            id: typeof item.id === "string" ? item.id : undefined,
            label: typeof item.label === "string" ? item.label : undefined
          }))
      : [];
  }

  function rows(value: unknown): unknown[][] {
    return Array.isArray(value) ? value.filter((row): row is unknown[] => Array.isArray(row)) : [];
  }

  function nodeId(): string {
    return typeof node.id === "string" ? node.id : "";
  }

  function optionId(option: CanvasOption): string {
    return option.id ?? option.label ?? "";
  }

  function optionLabel(option: CanvasOption): string {
    return option.label ?? option.id ?? "";
  }

  function isSelected(option: CanvasOption): boolean {
    const id = nodeId();
    const value = values[id];
    const current = optionId(option);
    if (node.type === "multiSelect") {
      return Array.isArray(value) && value.map(text).includes(current);
    }
    return text(value) === current;
  }

  function toggleMulti(option: CanvasOption, checked: boolean) {
    const id = nodeId();
    if (!id) return;
    const current = new Set(Array.isArray(values[id]) ? values[id].map(text) : []);
    const value = optionId(option);
    if (checked) current.add(value);
    else current.delete(value);
    onChange(id, Array.from(current));
  }

  function toneClass(value: unknown): string {
    const tone = text(value);
    return tone ? `tone-${tone}` : "";
  }

  function numeric(value: unknown, fallback: number): number {
    return typeof value === "number" && Number.isFinite(value) ? value : fallback;
  }

  function items(value: unknown): CanvasNode[] {
    return children(value);
  }

  const componentType = $derived(text(node.type));
  const id = $derived(nodeId());
</script>

{#if componentType === "section"}
  <section class="ic-section">
    {#if node.title}<h4>{text(node.title)}</h4>{/if}
    {#each children(node.children) as child, index (`${text(child.type)}-${index}`)}
      <InlineCanvasNode node={child} {values} {onChange} />
    {/each}
  </section>
{:else if componentType === "grid"}
  <div class="ic-grid" style={`--ic-min:${numeric(node.min, 190)}px`}>
    {#each children(node.children) as child, index (`${text(child.type)}-${index}`)}
      <InlineCanvasNode node={child} {values} {onChange} />
    {/each}
  </div>
{:else if componentType === "columns"}
  <div class="ic-columns">
    {#each children(node.children) as child, index (`${text(child.type)}-${index}`)}
      <InlineCanvasNode node={child} {values} {onChange} />
    {/each}
  </div>
{:else if componentType === "card"}
  <div class="ic-card">
    {#if node.title}<div class="ic-card-title">{text(node.title)}</div>{/if}
    {#each children(node.children) as child, index (`${text(child.type)}-${index}`)}
      <InlineCanvasNode node={child} {values} {onChange} />
    {/each}
  </div>
{:else if componentType === "divider"}
  <hr class="ic-divider" />
{:else if componentType === "heading"}
  <div class={`ic-heading level-${numeric(node.level, 2)}`}>{text(node.text)}</div>
{:else if componentType === "text"}
  <div class="ic-text">{text(node.value ?? node.text)}</div>
{:else if componentType === "badge"}
  <span class={`ic-badge ${toneClass(node.tone)}`}>{text(node.text)}</span>
{:else if componentType === "metrics"}
  <div class="ic-metrics">
    {#each items(node.items) as item, index (`metric-${index}`)}
      <span class={`ic-metric ${toneClass(item.tone)}`}><strong>{text(item.value)}</strong> {text(item.label)}</span>
    {/each}
  </div>
{:else if componentType === "kv"}
  <div class="ic-kv">
    {#each rows(node.rows) as row, index (`kv-${index}`)}
      <span>{text(row[0])}</span>
      <strong>{text(row[1])}</strong>
    {/each}
  </div>
{:else if componentType === "table"}
  <div class="ic-table-wrap">
    <table class="ic-table">
      <thead>
        <tr>
          {#each Array.isArray(node.columns) ? node.columns : [] as column, index (`col-${index}`)}
            <th>{text(column)}</th>
          {/each}
        </tr>
      </thead>
      <tbody>
        {#each rows(node.rows) as row, rowIndex (`row-${rowIndex}`)}
          <tr>
            {#each row as cell, cellIndex (`cell-${cellIndex}`)}
              <td>{text(cell)}</td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{:else if componentType === "bar"}
  <div class="ic-bars">
    {#each items(node.items) as item, index (`bar-${index}`)}
      {@const max = numeric(item.max, Math.max(1, numeric(item.value, 0)))}
      <div class="ic-bar-row">
        <span>{text(item.label)}</span>
        <div class="ic-bar-track">
          <div class={`ic-bar-fill ${toneClass(item.tone)}`} style={`width:${Math.min(100, Math.max(0, numeric(item.value, 0) / max * 100))}%`}></div>
        </div>
        <strong>{text(item.value)}</strong>
      </div>
    {/each}
  </div>
{:else if componentType === "toggle"}
  <label class="ic-control ic-toggle">
    <input type="checkbox" checked={values[id] === true} onchange={(event) => onChange(id, event.currentTarget.checked)} />
    <span>{text(node.label ?? node.id)}</span>
  </label>
{:else if componentType === "singleSelect" || componentType === "barSelect" || componentType === "multiSelect"}
  <div class="ic-control">
    <div class="ic-control-label">{text(node.label ?? node.id)}</div>
    <div class:ic-bar-select={componentType === "barSelect"} class="ic-choice-list">
      {#each options(node.options) as option (`${id}-${optionId(option)}`)}
        <label class:selected={isSelected(option)} class="ic-choice">
          <input
            type={componentType === "multiSelect" ? "checkbox" : "radio"}
            name={`ic-${id}`}
            checked={isSelected(option)}
            onchange={(event) =>
              componentType === "multiSelect"
                ? toggleMulti(option, event.currentTarget.checked)
                : onChange(id, optionId(option))}
          />
          <span>{optionLabel(option)}</span>
        </label>
      {/each}
    </div>
  </div>
{:else if componentType === "slider"}
  <label class="ic-control">
    <span class="ic-control-label">{text(node.label ?? node.id)}</span>
    <span class="ic-slider-row">
      <input
        type="range"
        min={numeric(node.min, 0)}
        max={numeric(node.max, 100)}
        step={numeric(node.step, 1)}
        value={numeric(values[id], numeric(node.value, numeric(node.min, 0)))}
        oninput={(event) => onChange(id, Number(event.currentTarget.value))}
      />
      <strong>{text(values[id])}</strong>
    </span>
  </label>
{:else if componentType === "textInput"}
  <label class="ic-control">
    <span class="ic-control-label">{text(node.label ?? node.id)}</span>
    <input
      class="ic-text-input"
      type="text"
      placeholder={text(node.placeholder)}
      value={text(values[id])}
      oninput={(event) => onChange(id, event.currentTarget.value)}
    />
  </label>
{:else if componentType === "finding"}
  <article class={`ic-finding severity-${text(node.severity || "info")}`}>
    <div class="ic-finding-head">
      <span>{text(node.severity || "info")}</span>
      <strong>{text(node.title)}</strong>
      {#if node.status}<em>{text(node.status)}</em>{/if}
    </div>
    {#if Array.isArray(node.locations)}
      <div class="ic-location">{node.locations.map(text).join("  ")}</div>
    {/if}
    {#if node.body}<div class="ic-text">{text(node.body)}</div>{/if}
    {#if node.evidence}<pre class="ic-code">{text(node.evidence)}</pre>{/if}
  </article>
{:else if componentType === "code" || componentType === "diff"}
  <pre class="ic-code">{text(node.text)}</pre>
{:else if componentType === "callout"}
  <div class={`ic-callout ${toneClass(node.tone)}`}>
    {#if node.title}<strong>{text(node.title)}</strong>{/if}
    {#if node.body}<span>{text(node.body)}</span>{/if}
  </div>
{:else if componentType === "heatmap"}
  <div class="ic-heatmap">
    <table>
      <tbody>
        {#each items(node.rows) as row, rowIndex (`heat-${rowIndex}`)}
          <tr>
            <th>{text(row.label)}</th>
            {#each Array.isArray(row.cells) ? row.cells : [] as cell, cellIndex (`heat-cell-${cellIndex}`)}
              <td data-v={text(cell)} title={text(cell)}></td>
            {/each}
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{:else}
  <div class="ic-unknown">Unsupported Canvas node: {componentType || "unknown"}</div>
{/if}
