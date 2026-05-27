<script lang="ts">
  /**
   * Markdown renderer ported from
   * `apps/puffer-desktop/src/lib/components/MessageBody.svelte` (v1).
   *
   * Adapted for v2 / Svelte 5:
   *   - `export let` → `$props()` runes
   *   - `$:` reactive labels → `$derived(...)` (only one in v1: `blocks`)
   *   - **Local-file-path detection dropped.** v1 distinguished
   *     `file://...` and `/abs/path:line` URLs and routed them through an
   *     `onOpenFile` callback (a desktop affordance for jumping into the
   *     editor pane). v2 has no editor pane, so only `http(s)://` URLs
   *     survive as clickable links and they open in the OS browser via
   *     `@tauri-apps/plugin-opener`.
   *
   * Test hook (no production behaviour change): if `window.__openUrl` is
   * defined, the click handler prefers it over the real `openUrl`. This
   * is how `tests/chat/assistant-bubble.spec.ts` spies on URL clicks
   * without intercepting Tauri's WS channel (`invoke` is unreachable in
   * vite-dev / Playwright mode anyway — calling the real `openUrl`
   * outside Tauri throws). The shim lives on `window` because Playwright's
   * `addInitScript` is the easiest way to install it before module load.
   */
  import { openUrl } from "@tauri-apps/plugin-opener";

  type InlineSegment = {
    kind: "text" | "code";
    text: string;
    strong?: boolean;
    emphasis?: boolean;
    strike?: boolean;
    href?: string;
  };

  type ListItem = {
    text: string;
    checked: boolean | null;
  };

  type MessageBlock =
    | { kind: "paragraph"; text: string }
    | { kind: "heading"; level: 1 | 2 | 3 | 4 | 5 | 6; text: string }
    | { kind: "list"; ordered: boolean; items: ListItem[] }
    | { kind: "quote"; text: string }
    | { kind: "code"; language: string | null; text: string }
    | { kind: "table"; headers: string[]; rows: string[][] }
    | { kind: "rule" };

  interface Props {
    body?: string;
  }
  let { body = "" }: Props = $props();

  // v1 matched `https?://`, `file://`, and bare `/abs/path` URLs. v2 drops
  // the latter two — only http(s) links are ever rendered as anchors.
  const urlPattern = /^https?:\/\/[^\s<]+$/;

  async function handleUrlClick(event: MouseEvent, href: string): Promise<void> {
    if (!urlPattern.test(href)) return;
    event.preventDefault();
    // Test shim: window.__openUrl is installed by Playwright via
    // page.exposeFunction (see assistant-bubble.spec.ts) so tests can
    // assert the click intent without invoking the real Tauri plugin
    // (which throws outside a Tauri runtime).
    const w = window as unknown as {
      __openUrl?: (url: string) => Promise<void> | void;
    };
    if (typeof w.__openUrl === "function") {
      await w.__openUrl(href);
      return;
    }
    try {
      await openUrl(href);
    } catch {
      // Plugin throws if we're not running under Tauri (e.g. browser
      // preview); fall through silently — the click was already prevented.
    }
  }

  function appendText(
    parts: InlineSegment[],
    text: string,
    flags: Omit<InlineSegment, "kind" | "text"> = {}
  ): void {
    if (!text) return;
    const prev = parts[parts.length - 1];
    if (
      prev?.kind === "text" &&
      prev.strong === flags.strong &&
      prev.emphasis === flags.emphasis &&
      prev.strike === flags.strike &&
      prev.href === flags.href
    ) {
      prev.text += text;
      return;
    }
    parts.push({ kind: "text", text, ...flags });
  }

  function findClosing(source: string, marker: string, start: number): number {
    let index = start;
    while (index < source.length) {
      const found = source.indexOf(marker, index);
      if (found === -1) return -1;
      if (found === 0 || source[found - 1] !== "\\") return found;
      index = found + marker.length;
    }
    return -1;
  }

  function parseInline(
    text: string,
    flags: Omit<InlineSegment, "kind" | "text"> = {}
  ): InlineSegment[] {
    const parts: InlineSegment[] = [];
    let index = 0;

    while (index < text.length) {
      const rest = text.slice(index);

      if (rest.startsWith("`")) {
        const close = findClosing(text, "`", index + 1);
        if (close !== -1) {
          parts.push({
            kind: "code",
            text: text.slice(index + 1, close)
          });
          index = close + 1;
          continue;
        }
      }

      if (rest.startsWith("[")) {
        const labelEnd = findClosing(text, "]", index + 1);
        if (labelEnd !== -1 && text[labelEnd + 1] === "(") {
          const hrefEnd = findClosing(text, ")", labelEnd + 2);
          if (hrefEnd !== -1) {
            const label = text.slice(index + 1, labelEnd);
            const href = text.slice(labelEnd + 2, hrefEnd).trim();
            const nested = parseInline(label, { ...flags, href });
            parts.push(...nested);
            index = hrefEnd + 1;
            continue;
          }
        }
      }

      const strongMarker = rest.startsWith("**") ? "**" : rest.startsWith("__") ? "__" : null;
      if (strongMarker) {
        const close = findClosing(text, strongMarker, index + 2);
        if (close !== -1) {
          parts.push(
            ...parseInline(text.slice(index + 2, close), {
              ...flags,
              strong: true
            })
          );
          index = close + 2;
          continue;
        }
      }

      if (rest.startsWith("~~")) {
        const close = findClosing(text, "~~", index + 2);
        if (close !== -1) {
          parts.push(
            ...parseInline(text.slice(index + 2, close), {
              ...flags,
              strike: true
            })
          );
          index = close + 2;
          continue;
        }
      }

      const emphasisMarker = rest.startsWith("*") ? "*" : rest.startsWith("_") ? "_" : null;
      if (emphasisMarker) {
        const close = findClosing(text, emphasisMarker, index + 1);
        if (close !== -1 && close > index + 1) {
          parts.push(
            ...parseInline(text.slice(index + 1, close), {
              ...flags,
              emphasis: true
            })
          );
          index = close + 1;
          continue;
        }
      }

      const nextMarkers = ["`", "[", "**", "__", "~~", "*", "_"]
        .map((marker) => {
          const found = text.indexOf(marker, index + 1);
          return found === -1 ? text.length : found;
        })
        .reduce((left, right) => Math.min(left, right), text.length);
      appendText(parts, text.slice(index, nextMarkers), flags);
      index = nextMarkers;
    }

    return parts.length > 0 ? parts : [{ kind: "text", text, ...flags }];
  }

  function taskState(text: string): { checked: boolean | null; text: string } {
    const task = text.match(/^\[([ xX])\]\s+(.*)$/);
    if (!task) return { checked: null, text };
    return { checked: task[1].toLowerCase() === "x", text: task[2] };
  }

  function splitTableRow(line: string): string[] {
    return line
      .trim()
      .replace(/^\|/, "")
      .replace(/\|$/, "")
      .split("|")
      .map((cell) => cell.trim());
  }

  function isTableSeparator(line: string): boolean {
    return /^\s*\|?\s*:?-{3,}:?\s*(\|\s*:?-{3,}:?\s*)+\|?\s*$/.test(line);
  }

  function parseBlocks(source: string): MessageBlock[] {
    const blocks: MessageBlock[] = [];
    const lines = source.replace(/\r\n?/g, "\n").split("\n");
    let paragraphLines: string[] = [];
    let quoteLines: string[] = [];
    let listItems: ListItem[] = [];
    let listOrdered = false;

    function flushParagraph(): void {
      if (paragraphLines.length === 0) return;
      blocks.push({
        kind: "paragraph",
        text: paragraphLines.join("\n").trim()
      });
      paragraphLines = [];
    }

    function flushQuote(): void {
      if (quoteLines.length === 0) return;
      blocks.push({
        kind: "quote",
        text: quoteLines.join("\n").trim()
      });
      quoteLines = [];
    }

    function flushList(): void {
      if (listItems.length === 0) return;
      blocks.push({
        kind: "list",
        ordered: listOrdered,
        items: [...listItems]
      });
      listItems = [];
    }

    for (let index = 0; index < lines.length; index += 1) {
      const line = lines[index];
      const trimmed = line.trim();
      const codeFence = line.match(/^```([\w.+-]+)?\s*$/);

      if (codeFence) {
        flushParagraph();
        flushQuote();
        flushList();
        const codeLines: string[] = [];
        let innerIndex = index + 1;
        while (innerIndex < lines.length && !lines[innerIndex].startsWith("```")) {
          codeLines.push(lines[innerIndex]);
          innerIndex += 1;
        }
        blocks.push({
          kind: "code",
          language: codeFence[1] ?? null,
          text: codeLines.join("\n")
        });
        index = innerIndex;
        continue;
      }

      if (index + 1 < lines.length && line.includes("|") && isTableSeparator(lines[index + 1])) {
        flushParagraph();
        flushQuote();
        flushList();
        const headers = splitTableRow(line);
        const rows: string[][] = [];
        index += 2;
        while (index < lines.length && lines[index].includes("|") && lines[index].trim() !== "") {
          rows.push(splitTableRow(lines[index]));
          index += 1;
        }
        index -= 1;
        blocks.push({ kind: "table", headers, rows });
        continue;
      }

      if (/^#{1,6}\s+/.test(line)) {
        flushParagraph();
        flushQuote();
        flushList();
        const heading = line.match(/^(#{1,6})\s+(.*?)\s*#*\s*$/);
        if (heading) {
          blocks.push({
            kind: "heading",
            level: heading[1].length as 1 | 2 | 3 | 4 | 5 | 6,
            text: heading[2]
          });
          continue;
        }
      }

      if (/^([-*_])(\s*\1){2,}\s*$/.test(trimmed)) {
        flushParagraph();
        flushQuote();
        flushList();
        blocks.push({ kind: "rule" });
        continue;
      }

      if (trimmed === "") {
        flushParagraph();
        flushQuote();
        flushList();
        continue;
      }

      const orderedItem = line.match(/^\s*\d+\.\s+(.*)$/);
      const unorderedItem = line.match(/^\s*[-*+]\s+(.*)$/);
      if (orderedItem || unorderedItem) {
        flushParagraph();
        flushQuote();
        const ordered = Boolean(orderedItem);
        const rawText = (orderedItem?.[1] ?? unorderedItem?.[1] ?? "").trim();
        const item = taskState(rawText);
        if (listItems.length > 0 && ordered !== listOrdered) {
          flushList();
        }
        listOrdered = ordered;
        listItems.push(item);
        continue;
      }

      if (line.startsWith("> ")) {
        flushParagraph();
        flushList();
        quoteLines.push(line.slice(2));
        continue;
      }

      flushQuote();
      paragraphLines.push(line.trim());
    }

    flushParagraph();
    flushQuote();
    flushList();

    return blocks;
  }

  let blocks = $derived(parseBlocks(body));
</script>

{#snippet inline(text: string)}
  {#each parseInline(text) as segment}
    {#if segment.kind === "code"}
      <code>{segment.text}</code>
    {:else if segment.href && urlPattern.test(segment.href)}
      <a
        href={segment.href}
        target="_blank"
        rel="noreferrer"
        class:strong={segment.strong}
        class:emphasis={segment.emphasis}
        class:strike={segment.strike}
        onclick={(event) => handleUrlClick(event, segment.href!)}
      >
        {segment.text}
      </a>
    {:else}
      <span
        class:strong={segment.strong}
        class:emphasis={segment.emphasis}
        class:strike={segment.strike}
      >
        {segment.text}
      </span>
    {/if}
  {/each}
{/snippet}

<div class="message-body">
  {#each blocks as block}
    {#if block.kind === "paragraph"}
      <p>{@render inline(block.text)}</p>
    {:else if block.kind === "heading"}
      <svelte:element this={`h${block.level}`} class="heading">
        {@render inline(block.text)}
      </svelte:element>
    {:else if block.kind === "list"}
      <svelte:element this={block.ordered ? "ol" : "ul"} class="list">
        {#each block.items as item}
          <li class:task={item.checked !== null}>
            {#if item.checked !== null}
              <input type="checkbox" checked={item.checked} disabled aria-label="task state" />
            {/if}
            <span>{@render inline(item.text)}</span>
          </li>
        {/each}
      </svelte:element>
    {:else if block.kind === "quote"}
      <blockquote>{@render inline(block.text)}</blockquote>
    {:else if block.kind === "table"}
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              {#each block.headers as header}
                <th>{@render inline(header)}</th>
              {/each}
            </tr>
          </thead>
          <tbody>
            {#each block.rows as row}
              <tr>
                {#each block.headers as _, cellIndex}
                  <td>{@render inline(row[cellIndex] ?? "")}</td>
                {/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if block.kind === "rule"}
      <hr />
    {:else}
      <div class="code-block">
        {#if block.language}
          <span class="language">{block.language}</span>
        {/if}
        <pre>{block.text}</pre>
      </div>
    {/if}
  {/each}
</div>

<style>
  .message-body {
    display: grid;
    gap: 0.6rem;
    font-size: 14px;
    line-height: 20px;
    font-family: var(--font-system);
    color: var(--color-text-primary);
  }

  p,
  blockquote,
  pre,
  .heading {
    margin: 0;
  }

  p,
  li,
  blockquote,
  td,
  th {
    line-height: 1.5;
  }

  p {
    white-space: pre-wrap;
    word-wrap: break-word;
  }

  .heading {
    font-weight: var(--font-weight-medium, 600);
    line-height: 1.25;
  }

  h1.heading {
    font-size: 1.28rem;
  }

  h2.heading {
    font-size: 1.16rem;
  }

  h3.heading,
  h4.heading,
  h5.heading,
  h6.heading {
    font-size: 1.04rem;
  }

  .list {
    margin: 0;
    padding-left: 1.35rem;
    display: grid;
    gap: 0.3rem;
  }

  li.task {
    list-style: none;
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    margin-left: -1.35rem;
  }

  input[type="checkbox"] {
    width: 0.9rem;
    height: 0.9rem;
  }

  blockquote {
    padding: 0.6rem 0.8rem;
    border-left: 3px solid var(--color-card-border);
    background: var(--color-surface-rail);
    color: var(--color-text-secondary);
    white-space: pre-wrap;
  }

  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.9em;
    padding: 0.06rem 0.3rem;
    border-radius: 4px;
    background: var(--color-surface-rail);
  }

  .strong {
    font-weight: var(--font-weight-medium, 600);
  }

  .emphasis {
    font-style: italic;
  }

  .strike {
    text-decoration: line-through;
  }

  a {
    color: var(--color-action-cream-text, #137168);
    text-decoration: underline;
    text-underline-offset: 0.16em;
    cursor: pointer;
  }

  .table-wrap {
    overflow: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.94em;
  }

  th,
  td {
    padding: 0.42rem 0.55rem;
    border: 1px solid var(--color-card-border);
    text-align: left;
    vertical-align: top;
  }

  th {
    background: var(--color-surface-rail);
    font-weight: var(--font-weight-medium, 600);
  }

  hr {
    width: 100%;
    border: 0;
    border-top: 1px solid var(--color-card-border);
    margin: 0.2rem 0;
  }

  .code-block {
    display: grid;
    gap: 0.3rem;
  }

  .language {
    color: var(--color-text-secondary);
    font-size: 0.74rem;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  pre {
    padding: 0.8rem 0.9rem;
    border-radius: 6px;
    background: var(--color-surface-rail);
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 0.88rem;
    line-height: 1.55;
    white-space: pre-wrap;
    overflow: auto;
  }
</style>
