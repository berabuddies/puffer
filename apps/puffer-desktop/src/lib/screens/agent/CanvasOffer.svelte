<script lang="ts">
  import type { UserQuestionTimelineItem } from "../../types";

  // A "canvas-offer" AskUserQuestion reskinned as a one-click Canvas button.
  // Contract (see the canvas-offer agent guidance): one question whose
  // options[0] = use-Canvas (affirmative), options[1] = decline. Clicking
  // resolves the question with that option's label, so the agent — which asked
  // "render as Canvas?" — proceeds to call the Canvas tool on the affirmative.
  let {
    item,
    disabled = false,
    onResolve
  }: {
    item: UserQuestionTimelineItem;
    disabled?: boolean;
    onResolve: (
      questionId: string,
      answers: Record<string, string | string[]>,
      annotations?: Record<string, Record<string, string>>
    ) => void;
  } = $props();

  const q = $derived(item.questions[0]);
  const goLabel = $derived(q?.options?.[0]?.label ?? "用 Canvas 打开");
  const skipOption = $derived(q?.options?.[1]);

  function answer(label: string) {
    if (disabled || !q) return;
    onResolve(item.id, { [q.question]: label }, {});
  }
</script>

<div class="pf-canvas-offer">
  <span class="pf-canvas-offer__hint">{q?.question ?? "用 Canvas 呈现这个结果?"}</span>
  <div class="pf-canvas-offer__btns">
    <button
      class="pf-canvas-offer__go"
      {disabled}
      onclick={() => answer(goLabel)}
    >
      ✨ {goLabel}
    </button>
    {#if skipOption}
      <button class="pf-canvas-offer__skip" {disabled} onclick={() => answer(skipOption.label)}>
        {skipOption.label}
      </button>
    {/if}
  </div>
</div>

<style>
  .pf-canvas-offer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 12px 14px;
    border: 1px solid var(--pf-border, rgba(127, 127, 127, 0.25));
    border-radius: 10px;
    background: var(--pf-surface, rgba(127, 127, 127, 0.06));
  }
  .pf-canvas-offer__hint {
    font-size: 0.85rem;
    opacity: 0.8;
  }
  .pf-canvas-offer__btns {
    display: flex;
    gap: 10px;
    align-items: center;
  }
  .pf-canvas-offer__go {
    padding: 7px 14px;
    border-radius: 8px;
    border: 1px solid transparent;
    background: var(--pf-accent, #0891b2);
    color: #fff;
    font-weight: 600;
    cursor: pointer;
  }
  .pf-canvas-offer__skip {
    padding: 7px 12px;
    border-radius: 8px;
    border: 1px solid var(--pf-border, rgba(127, 127, 127, 0.3));
    background: transparent;
    color: inherit;
    opacity: 0.7;
    cursor: pointer;
  }
  .pf-canvas-offer__go:disabled,
  .pf-canvas-offer__skip:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
