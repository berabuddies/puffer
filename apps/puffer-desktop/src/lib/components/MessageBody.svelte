<script lang="ts">
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

  export let body = "";
  export let onOpenFile: ((path: string, line?: number | null) => void) | undefined = undefined;

  const urlPattern = /^(https?:\/\/[^\s<]+|file:\/\/[^\s<]+|\/[^\s<]+)$/;

  function fileTarget(href: string): { path: string; line: number | null } | null {
    let value = href;
    if (value.startsWith("file://")) {
      try {
        value = decodeURIComponent(new URL(value).pathname);
      } catch {
        value = value.slice("file://".length);
      }
    }
    if (!value.startsWith("/")) return null;
    const match = value.match(/^(.*?):(\d+)(?::\d+)?$/);
    if (match) {
      return {
        path: match[1],
        line: Number(match[2])
      };
    }
    return { path: value, line: null };
  }

  function openFileLink(event: MouseEvent, href: string) {
    const target = fileTarget(href);
    if (!target) return;
    event.preventDefault();
    onOpenFile?.(target.path, target.line);
  }

  function appendText(
    parts: InlineSegment[],
    text: string,
    flags: Omit<InlineSegment, "kind" | "text"> = {}
  ) {
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

    function flushParagraph() {
      if (paragraphLines.length === 0) return;
      blocks.push({
        kind: "paragraph",
        text: paragraphLines.join("\n").trim()
      });
      paragraphLines = [];
    }

    function flushQuote() {
      if (quoteLines.length === 0) return;
      blocks.push({
        kind: "quote",
        text: quoteLines.join("\n").trim()
      });
      quoteLines = [];
    }

    function flushList() {
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

  $: blocks = parseBlocks(body);
</script>

{#snippet inline(text: string)}
  {#each parseInline(text) as segment}
    {#if segment.kind === "code"}
      <code>{segment.text}</code>
    {:else if segment.href && urlPattern.test(segment.href)}
      {@const localFile = fileTarget(segment.href)}
      <a
        href={segment.href}
        target={localFile ? undefined : "_blank"}
        rel={localFile ? undefined : "noreferrer"}
        class:local-file={Boolean(localFile)}
        class:strong={segment.strong}
        class:emphasis={segment.emphasis}
        class:strike={segment.strike}
        onclick={(event) => openFileLink(event, segment.href!)}
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
    gap: 0.95rem;
    font-size: 1rem;
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
    line-height: 1.78;
  }

  p {
    white-space: pre-wrap;
  }

  .heading {
    color: var(--text);
    font-weight: 760;
    line-height: 1.25;
    letter-spacing: 0;
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
    gap: 0.42rem;
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
    accent-color: var(--accent);
  }

  blockquote {
    padding: 0.9rem 1rem;
    border-left: 3px solid rgba(20, 99, 86, 0.24);
    background: rgba(222, 238, 232, 0.38);
    border-radius: 0;
    color: var(--text-muted);
    white-space: pre-wrap;
  }

  code {
    font-family: var(--font-mono);
    font-size: 0.9em;
    padding: 0.08rem 0.32rem;
    border-radius: 0;
    background: rgba(247, 243, 235, 0.92);
    box-shadow: 0 1px 0 rgba(255, 255, 255, 0.55) inset;
  }

  .strong {
    font-weight: 720;
  }

  .emphasis {
    font-style: italic;
  }

  .strike {
    text-decoration: line-through;
  }

  a {
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 0.16em;
  }
  a.local-file {
    color: var(--muted-foreground);
    text-decoration-color: color-mix(in oklab, var(--muted-foreground) 35%, transparent);
    text-decoration-thickness: 1px;
  }
  a.local-file:hover {
    color: var(--foreground);
    text-decoration-color: var(--muted-foreground);
  }

  .table-wrap {
    overflow: auto;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 0.94rem;
  }

  th,
  td {
    padding: 0.42rem 0.55rem;
    border: 1px solid rgba(47, 75, 69, 0.14);
    text-align: left;
    vertical-align: top;
  }

  th {
    background: rgba(222, 238, 232, 0.42);
    font-weight: 720;
  }

  hr {
    width: 100%;
    border: 0;
    border-top: 1px solid rgba(47, 75, 69, 0.18);
    margin: 0.2rem 0;
  }

  .code-block {
    display: grid;
    gap: 0.45rem;
  }

  .language {
    color: var(--text-muted);
    font-size: 0.74rem;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }

  pre {
    padding: 0.95rem 1rem;
    border-radius: 0;
    background: rgba(247, 243, 235, 0.82);
    font-family: var(--font-mono);
    font-size: 0.88rem;
    line-height: 1.68;
    white-space: pre-wrap;
    overflow: auto;
    box-shadow: 0 1px 0 rgba(255, 255, 255, 0.55) inset;
  }
</style>
