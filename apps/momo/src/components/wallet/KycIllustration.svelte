<!--
  KycIllustration — inline line-art SVG used as the empty-state hero on
  /wallet/kyc. The design (Paper artboard 1OM-0) shows a stylized stack of
  coins with a paper plane flying away and a few dust marks.

  We re-author the artwork inline so the page ships with zero asset
  dependencies; strokes use --color-text-secondary so the illustration
  reads as part of the quiet line-art system (DESIGN_SYSTEM "Icons are
  line-based"). All line widths are 1.5px to match the Lucide rhythm.
-->
<script lang="ts">
  interface Props {
    /** Optional title for accessibility — defaults to a descriptive label. */
    title?: string;
  }

  let { title = "Coins and a paper plane" }: Props = $props();
</script>

<svg
  class="kyc-illustration"
  viewBox="0 0 250 130"
  width="250"
  height="130"
  fill="none"
  role="img"
  aria-label={title}
  xmlns="http://www.w3.org/2000/svg"
>
  <!-- Dashed swoosh trailing from the coins up to the paper plane. -->
  <path
    d="M118 70 C 95 50, 115 22, 150 30 C 178 36, 196 18, 205 12"
    stroke="currentColor"
    stroke-width="1.2"
    stroke-linecap="round"
    stroke-dasharray="3 4"
    fill="none"
    opacity="0.55"
  />

  <!-- Coin stack — four ellipses stacked vertically. -->
  <g
    stroke="currentColor"
    stroke-width="1.4"
    stroke-linecap="round"
    stroke-linejoin="round"
    fill="none"
  >
    <!-- Bottom rim (back arc) -->
    <path d="M95 102 a22 7 0 0 0 44 0" />
    <!-- Coin 4 (bottom) -->
    <path d="M95 95 a22 7 0 0 0 44 0 v6 a22 7 0 0 1 -44 0 z" />
    <!-- Coin 3 -->
    <path d="M95 86 a22 7 0 0 0 44 0 v6 a22 7 0 0 1 -44 0 z" />
    <!-- Coin 2 -->
    <path d="M95 77 a22 7 0 0 0 44 0 v6 a22 7 0 0 1 -44 0 z" />
    <!-- Coin 1 (top, full ellipse for the rim highlight) -->
    <ellipse cx="117" cy="71" rx="22" ry="7" />
  </g>

  <!-- Coin face (front coin, slightly larger, sits in front of the stack). -->
  <g
    stroke="currentColor"
    stroke-width="1.4"
    stroke-linecap="round"
    stroke-linejoin="round"
    fill="none"
  >
    <circle cx="135" cy="86" r="20" />
    <!-- Currency glyph: a stylized ₽-ish symbol so it reads as money without
         localizing. -->
    <path d="M129 76 v22" />
    <path d="M129 76 h7 a5 5 0 0 1 0 10 h-7" />
    <path d="M125 90 h10" />
  </g>

  <!-- Soft shadow under the coin stack. -->
  <ellipse
    cx="128"
    cy="112"
    rx="34"
    ry="3"
    fill="currentColor"
    opacity="0.12"
  />

  <!-- Sparkle / "++" dust to the right of the coins. -->
  <g
    stroke="currentColor"
    stroke-width="1.2"
    stroke-linecap="round"
    fill="none"
    opacity="0.7"
  >
    <path d="M170 78 v8" />
    <path d="M166 82 h8" />
    <path d="M180 88 v6" />
    <path d="M177 91 h6" />
  </g>

  <!-- Paper plane: triangular fuselage + folded center crease. -->
  <g
    stroke="currentColor"
    stroke-width="1.4"
    stroke-linecap="round"
    stroke-linejoin="round"
    fill="none"
  >
    <path d="M198 26 L222 8 L215 36 Z" />
    <path d="M198 26 L215 36" />
    <path d="M215 36 L210 26 L222 8" />
  </g>
</svg>

<style>
  .kyc-illustration {
    display: block;
    color: var(--color-text-secondary);
  }
</style>
