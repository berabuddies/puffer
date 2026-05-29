<script lang="ts">
  import { highlightCodeLine, type HighlightToken } from "../codeHighlight";

  type Props = {
    text: string;
    path?: string | null;
    highlight?: string | null;
  };

  let { text, path = null, highlight = null }: Props = $props();
  let tokens: HighlightToken[] = $derived(highlightCodeLine(text, path));

  function highlightParts(value: string): Array<{ text: string; hit: boolean }> {
    if (!highlight) return [{ text: value, hit: false }];
    const parts: Array<{ text: string; hit: boolean }> = [];
    let start = 0;
    while (start < value.length) {
      const index = value.indexOf(highlight, start);
      if (index === -1) {
        parts.push({ text: value.slice(start), hit: false });
        break;
      }
      if (index > start) parts.push({ text: value.slice(start, index), hit: false });
      parts.push({ text: value.slice(index, index + highlight.length), hit: true });
      start = index + highlight.length;
    }
    return parts.filter((part) => part.text.length > 0);
  }
</script>

<span class="hl-line">
  {#each tokens as token, i (`${i}-${token.kind}`)}
    <span class={`tok ${token.kind}`}>
      {#each highlightParts(token.text) as part, j (`${j}-${part.hit}`)}
        {#if part.hit}
          <span class="symbol-hit">{part.text}</span>
        {:else}
          {part.text}
        {/if}
      {/each}
    </span>
  {/each}
</span>

<style>
  .hl-line {
    white-space: pre;
  }
  .tok.keyword {
    color: #7c3aed;
    font-weight: 600;
  }
  .tok.string {
    color: #166534;
  }
  .tok.number {
    color: #b45309;
  }
  .tok.comment {
    color: #6b7280;
    font-style: italic;
  }
  .tok.property,
  .tok.attribute {
    color: #0f766e;
  }
  .tok.operator {
    color: #475569;
  }
  .tok.punctuation {
    color: #64748b;
  }
  .tok.function {
    color: #2563eb;
  }
  .tok.type {
    color: #9333ea;
  }
  .tok.tag {
    color: #be123c;
  }
  .symbol-hit {
    background: color-mix(in oklab, var(--puffer-accent, #f59e0b) 22%, transparent);
    box-shadow: 0 0 0 1px color-mix(in oklab, var(--puffer-accent, #f59e0b) 32%, transparent) inset;
    border-radius: 3px;
  }
</style>
