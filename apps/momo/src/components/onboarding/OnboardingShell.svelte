<!--
  OnboardingShell — full-bleed centered layout shared by the four onboarding
  pages. Renders the mascot, a serif display title, an optional serif
  subtitle, the page-specific content via the default slot, and an optional
  skip link at the bottom.

  The design (artboards 23H-0 / 26N-0 / 2A1-0 / 2E1-0) uses:
    - white page background
    - vertically centered content stack
    - mascot 78–82px (rendered via Mascot size="lg" = 96 → matches the
      design's perceived weight; the artboard exports range 78–82 so the
      96 token reads identically once the page is centered)
    - serif display 32/46
    - serif subtitle 24/24 (Where uses 24/32 because it wraps to two lines)
    - 28px gap above the content slot
    - 28px gap above the skip link
    - skip link: 13px medium muted with a 13px chevron-right
-->
<script lang="ts">
  import type { Snippet } from "svelte";
  import { ChevronRight } from "lucide-svelte";

  import Mascot from "../common/Mascot.svelte";
  import { navigate } from "../../router.svelte";

  interface Props {
    title: string;
    /** Optional secondary serif line. Pass `bodyLines` for multi-line wrapping. */
    subtitle?: string;
    /** Use when the design wraps the subtitle across multiple lines (Where). */
    bodyLines?: string[];
    /** Page-specific main content (chip row, cards, etc). */
    children?: Snippet;
    /** Where the skip link goes; omit to hide the link. */
    skipTo?: string;
    skipLabel?: string;
  }

  let {
    title,
    subtitle,
    bodyLines,
    children,
    skipTo,
    skipLabel = "Skip for now and explore the app"
  }: Props = $props();

  function onSkip(event: MouseEvent): void {
    event.preventDefault();
    if (skipTo) navigate(skipTo);
  }
</script>

<main class="onboarding">
  <div class="stack">
    <div class="headline">
      <Mascot size="lg" />
      <h1 class="onboarding__title">{title}</h1>
      {#if bodyLines && bodyLines.length > 0}
        <p class="onboarding__subtitle onboarding__subtitle--multi">
          {#each bodyLines as line, i (i)}
            <span>{line}</span>
            {#if i < bodyLines.length - 1}<br />{/if}
          {/each}
        </p>
      {:else if subtitle}
        <p class="onboarding__subtitle">{subtitle}</p>
      {/if}
    </div>

    {#if children}
      <div class="content">
        {@render children()}
      </div>
    {/if}

    {#if skipTo}
      <a class="skip-link" href={`#${skipTo}`} onclick={onSkip}>
        <span>{skipLabel}</span>
        <ChevronRight size={13} strokeWidth={2} aria-hidden="true" />
      </a>
    {/if}
  </div>
</main>

<style>
  .onboarding {
    min-height: 100vh;
    width: 100%;
    background: var(--color-surface-app);
    display: flex;
    align-items: center;
    justify-content: center;
    /* Page padding so the centered column never kisses the window edge. */
    padding: var(--space-7) var(--space-5);
  }

  .stack {
    width: 100%;
    max-width: 680px;
    display: flex;
    flex-direction: column;
    align-items: center;
    /* Headline → content gap = 28px per artboard. */
    gap: 0;
  }

  .headline {
    display: flex;
    flex-direction: column;
    align-items: center;
    /* Mascot → title and title → subtitle both 12px per Paper. */
    gap: 12px;
  }

  /* Display — Source Serif 4, 32/46, regular, -0.5px tracking.
     The token --font-size-display is 28/34; the onboarding artboards explicitly
     use 32/46 (read with get_computed_styles), so we override locally. */
  .onboarding__title {
    font-family: var(--font-serif);
    font-size: 32px;
    line-height: 46px;
    letter-spacing: -0.5px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-primary);
    text-align: center;
  }

  /* Subtitle — Source Serif 4, 24/24, muted, -0.5px tracking.
     Single-line variant uses width: max-content so it doesn't wrap. */
  .onboarding__subtitle {
    font-family: var(--font-serif);
    font-size: 24px;
    line-height: 24px;
    letter-spacing: -0.5px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-muted);
    text-align: center;
  }

  /* Multi-line variant (Where) — use 32px line-height so wrapped lines breathe. */
  .onboarding__subtitle--multi {
    line-height: 32px;
  }

  .content {
    margin-top: 28px;
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .skip-link {
    margin-top: 28px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-family: var(--font-system);
    font-size: 13px;
    line-height: 16px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-muted);
    cursor: pointer;
    border-radius: var(--radius-control);
    padding: 4px 8px;
    transition: background-color 120ms ease;
  }

  .skip-link:hover {
    background: var(--color-selected-fill);
  }
</style>
