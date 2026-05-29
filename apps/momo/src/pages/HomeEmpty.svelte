<!--
  Home (empty state) — XS-0.

  Layout:
    PageHeader (no accessory; mascot lives centered below)
    Vertically-centered hero block:
      Mascot (lg)
      "Connect one app, I will be more helpful"
      Inline Composer (acts as the only composer for this page)
      Dark "Connect an app" CTA (one-off dark pill — design exception
        to the secondary style; styled inline so we don't pollute Button)
      "Why need apps" caption
      3 ExplanationCards in a row
    No bottom composer (the inline one takes its place — confirmed via
    Paper XS-0 / Home Empty State frame).
-->
<script lang="ts">
  import { LayoutGrid } from "lucide-svelte";

  import PageHeader from "../components/shell/PageHeader.svelte";
  import Composer from "../components/shell/Composer.svelte";
  import Mascot from "../components/common/Mascot.svelte";
  import ExplanationCard from "../components/home/ExplanationCard.svelte";

  import { navigate } from "../router.svelte";
  import { pushToast } from "../lib/toast.svelte";

  function handleConnect(): void {
    navigate("/apps");
  }

  // The three explanation cards each navigate to a sensible destination so
  // empty-state users can probe each surface of the app. Choice reasoning:
  //   1) "Give me signal"        → /apps     (connected apps are where signal comes from)
  //   2) "Become your assistant" → /wallet   (wallet is the deepest agent surface)
  //   3) "Still works as chat"   → /apps     (apps unlock chat capabilities)
  function openSignal(): void {
    navigate("/apps");
  }
  function openAssistant(): void {
    navigate("/wallet");
  }
  function openChat(): void {
    navigate("/apps");
  }
</script>

<PageHeader
  title="Good morning, Yuna."
  subtitle="Connect one app, or chat with Momo first"
/>

<section class="empty">
  <div class="empty__hero">
    <Mascot size="lg" />
    <p class="text-section empty__copy">Connect one app,&nbsp;&nbsp;I will be more helpful</p>

    <div class="empty__composer">
      <Composer placeholder="Hi, Tomo. How's my luck today?" />
    </div>

    <button class="empty__cta" type="button" onclick={handleConnect}>
      <LayoutGrid size={16} strokeWidth={1.75} aria-hidden="true" />
      <span>Connect an app</span>
    </button>
  </div>

  <div class="empty__followup">
    <p class="empty__caption">Why need apps</p>
    <div class="empty__cards">
      <button type="button" class="empty__card-btn" onclick={openSignal}>
        <ExplanationCard
          title="Give me signal"
          body="One connected app is enough to start understanding your day."
        />
      </button>
      <button type="button" class="empty__card-btn" onclick={openAssistant}>
        <ExplanationCard
          title="Become your assistant"
          body="I can recommend actions instead of waiting for prompts."
        />
      </button>
      <button type="button" class="empty__card-btn" onclick={openChat}>
        <ExplanationCard
          title="Still works as chat"
          body="You can ask anything before integrations are ready."
        />
      </button>
    </div>
  </div>
</section>

<style>
  .empty {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--space-5);
    padding-bottom: var(--space-5);
  }

  .empty__hero {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-4);
    width: 100%;
  }

  .empty__copy {
    text-align: center;
  }

  /* Inline composer: narrower than the page-wide composer so it reads as a
     hero element. The shared Composer renders a top divider; suppress it
     here since there's no content directly above to separate from. */
  .empty__composer {
    width: 398px;
    max-width: 100%;
  }
  .empty__composer :global(.composer) {
    border-top: 0;
    padding: 0;
  }
  .empty__composer :global(.composer__attach) {
    width: var(--height-button-utility);
    height: var(--height-button-utility);
  }

  /* One-off dark CTA. Design uses black bg + white text — kept local so
     Button/Chip stay clean. */
  .empty__cta {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    height: var(--height-button-composer);
    padding: 0 var(--space-4);
    border-radius: var(--radius-pill);
    background: var(--color-text-primary);
    color: var(--color-surface-app);
    font-family: var(--font-sans);
    font-size: 13px;
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    border: 1px solid var(--color-text-primary);
    transition: filter 120ms ease;
  }
  .empty__cta:hover {
    filter: brightness(1.1);
  }

  .empty__followup {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-2);
    width: 100%;
    max-width: 520px;
  }

  .empty__caption {
    font-family: var(--font-sans);
    font-size: 14px;
    line-height: 21px;
    font-weight: var(--font-weight-regular);
    color: var(--color-text-muted);
    text-align: center;
  }

  .empty__cards {
    display: flex;
    gap: var(--space-2);
    width: 100%;
    padding-top: var(--space-1);
  }

  /* Reset the button defaults so the wrapped ExplanationCard owns the visual. */
  .empty__card-btn {
    appearance: none;
    background: transparent;
    border: 0;
    padding: 0;
    margin: 0;
    flex: 1;
    min-width: 0;
    display: flex;
    cursor: pointer;
    border-radius: 10px;
    transition: transform 120ms ease;
  }
  .empty__card-btn:hover {
    transform: translateY(-1px);
  }
  .empty__card-btn:focus-visible {
    outline: 2px solid var(--color-action-cream-border);
    outline-offset: 2px;
  }
</style>
