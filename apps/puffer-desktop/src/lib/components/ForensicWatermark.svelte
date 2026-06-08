<script lang="ts">
  // __APP_VERSION__ / __COMMIT_HASH__ are injected by vite.config.ts `define`
  // (declared globally in vite-env.d.ts).
  let { username = "unknown" }: { username?: string | null } = $props();

  const version = typeof __APP_VERSION__ !== "undefined" ? __APP_VERSION__ : "0.0.0";
  const commit = typeof __COMMIT_HASH__ !== "undefined" ? __COMMIT_HASH__ : "unknown";

  // Forensic label — recoverable from a screenshot, imperceptible in normal use.
  const label = $derived(`v${version} · ${commit} · ${username || "unknown"}`);

  // One faint horizontal text per tile; the whole layer is rotated by CSS so the
  // SVG never has to rotate (avoids clipping). Tile gaps keep it sparse, not busy.
  const FONT_PX = 12.5;
  const GAP_X = 90; // horizontal space after each label
  const GAP_Y = 96; // vertical space between rows
  // Conservative monospace-ish width estimate so long usernames don't clip.
  const tileW = $derived(Math.ceil(label.length * FONT_PX * 0.62) + GAP_X);

  function tileUrl(text: string, w: number): string {
    const esc = text
      .replace(/&/g, "&amp;")
      .replace(/</g, "&lt;")
      .replace(/>/g, "&gt;");
    const svg =
      `<svg xmlns='http://www.w3.org/2000/svg' width='${w}' height='${GAP_Y}'>` +
      `<text x='0' y='${Math.round(GAP_Y * 0.66)}' ` +
      `font-family='-apple-system,BlinkMacSystemFont,Segoe UI,Roboto,sans-serif' ` +
      `font-size='${FONT_PX}' font-weight='500' fill='%23808080' fill-opacity='0.024'>` +
      esc +
      `</text></svg>`;
    return `url("data:image/svg+xml,${encodeURIComponent(svg)}")`;
  }

  const bg = $derived(tileUrl(label, tileW));
</script>

<div class="pf-watermark" aria-hidden="true" style:background-image={bg}></div>

<style>
  .pf-watermark {
    position: fixed;
    /* Oversize + center so a -28deg rotation still covers the whole viewport
       with no exposed corners. */
    top: -50%;
    left: -50%;
    width: 200%;
    height: 200%;
    transform: rotate(-28deg);
    transform-origin: center center;
    background-repeat: repeat;
    pointer-events: none;
    /* Above all functional UI (modals top out ~100) so every screenshot,
       on any screen, carries the mark. Faint + non-interactive = invisible UX. */
    z-index: 2147483600;
    /* Hint the compositor: a single static, promoted layer — no repaint cost. */
    will-change: transform;
    contain: strict;
    user-select: none;
  }
</style>
