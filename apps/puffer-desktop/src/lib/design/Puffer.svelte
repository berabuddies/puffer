<script lang="ts">
  import type { Snippet } from "svelte";

  type Props = {
    size?: number;
    state?: "idle" | "thinking" | "running" | "awaiting" | "review";
    accent?: string;
  };

  let { size = 32, state = "idle", accent }: Props = $props();

  const spikes = (() => {
    const arr: { x: number; y: number; len: number }[] = [];
    const n = 12;
    for (let i = 0; i < n; i++) {
      const angle = (i / n) * Math.PI * 2;
      arr.push({
        x: Math.cos(angle),
        y: Math.sin(angle),
        len: 3.2 + (i % 3) * 0.5
      });
    }
    return arr;
  })();

  const gradId = `pf-grad-${Math.random().toString(36).slice(2, 9)}`;
  let stroke = $derived(accent ?? "var(--puffer-accent)");
</script>

<span class="pf-puffer" data-state={state} style="width: {size}px; height: {size}px;">
  <span class="pulse-ring"></span>
  <svg viewBox="-20 -20 40 40">
    <defs>
      <radialGradient id={gradId} cx="40%" cy="35%" r="65%">
        <stop offset="0%" stop-color="white" stop-opacity="0.6" />
        <stop offset="55%" stop-color={stroke} stop-opacity="0.95" />
        <stop offset="100%" stop-color={stroke} stop-opacity="1" />
      </radialGradient>
    </defs>
    <g class="body">
      {#each spikes as s, i (i)}
        <line
          class="spike"
          x1={s.x * 9}
          y1={s.y * 9}
          x2={s.x * (9 + s.len)}
          y2={s.y * (9 + s.len)}
          stroke={stroke}
          stroke-width="1.4"
          stroke-linecap="round"
          opacity="0.55"
        />
      {/each}
      <circle cx="0" cy="0" r="9" fill="url(#{gradId})" />
      <circle cx="-2.5" cy="-3" r="1.2" fill="white" opacity="0.85" />
    </g>
  </svg>
</span>
