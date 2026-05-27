<!--
  AnswerForm — inline form rendered when the agent emits `user-question-request`.

  Single-question, single-select MVP. v1's QuestionPrompt.svelte (459
  lines) supports multi-question + multi-select + "other" custom input;
  the wallet use case is "Pick a recipient" / "Confirm amount" so single-
  select covers it. Multi-question / multi-select stay deferred.

  Visual borrow from OptionsCard.svelte: asymmetric border-radius
  (4/16/16/16 — agent-side cards have the top-left corner sharpened),
  white surface, 1px var(--color-input-border), padding 13/15. The rows
  are interactive radio buttons rather than static rows.

  Test addressability (DO NOT remove without updating
  apps/momo/tests/chat/answer-form.spec.ts):
    - root form:          [data-testid="answer-form"]
    - root form root:     [data-answered="true|false"]
    - per option button:  [data-option-label="<label>"]
    - selected option:    [data-selected="true|false"]
    - submit button:      [data-testid="answer-form-submit"]
-->
<script lang="ts">
  import type { AskUserQuestionItem } from "../../lib/agentClient";

  interface Props {
    questions: AskUserQuestionItem[];
    answered: boolean;
    onSubmit: (answers: Record<string, string | string[]>) => void;
  }
  let { questions, answered, onSubmit }: Props = $props();

  // MVP renders the FIRST question only. Multi-question batches are
  // a deferred follow-up per the plan's Out-of-Scope table.
  let activeQuestion = $derived<AskUserQuestionItem | undefined>(questions[0]);

  // Per-question selection keyed by question index (stringified) so the
  // submit payload matches the v1 wire shape — `answers["0"]` is question
  // 0's pick. Storing the label (not an index into options) keeps the
  // payload self-describing for the agent worker on the other end.
  let selected = $state<Record<string, string>>({});

  function pick(label: string): void {
    if (answered) return;
    selected = { ...selected, "0": label };
  }

  let canSubmit = $derived(!answered && Boolean(selected["0"]));

  function submit(): void {
    if (!canSubmit) return;
    onSubmit({ ...selected });
  }
</script>

{#if activeQuestion}
  <form
    class="answer-form"
    data-testid="answer-form"
    data-answered={answered}
    onsubmit={(e) => {
      e.preventDefault();
      submit();
    }}
  >
    <p class="answer-form__heading">{activeQuestion.question}</p>
    <div class="answer-form__options">
      {#each activeQuestion.options as opt}
        <button
          type="button"
          class="answer-form__opt"
          data-option-label={opt.label}
          data-selected={selected["0"] === opt.label ? "true" : "false"}
          disabled={answered}
          onclick={() => pick(opt.label)}
        >
          <span class="answer-form__opt-label">{opt.label}</span>
          {#if opt.description}
            <span class="answer-form__opt-desc">{opt.description}</span>
          {/if}
        </button>
      {/each}
    </div>
    {#if !answered}
      <button
        type="submit"
        class="answer-form__submit"
        data-testid="answer-form-submit"
        disabled={!canSubmit}
      >
        Send answer
      </button>
    {/if}
  </form>
{/if}

<style>
  .answer-form {
    display: flex;
    flex-direction: column;
    width: 100%;
    max-width: 540px;
    background: var(--color-surface-app);
    border: 1px solid var(--color-input-border);
    border-radius: 4px 16px 16px 16px;
    padding: 13px 15px;
    gap: 12px;
  }

  .answer-form__heading {
    margin: 0;
    font-family: var(--font-system);
    font-size: 14px;
    line-height: 20px;
    font-weight: var(--font-weight-medium);
    color: var(--color-text-primary);
  }

  .answer-form__options {
    display: flex;
    flex-direction: column;
    gap: 7px;
  }

  /* `all: unset` strips browser button styles so we can style from scratch.
   * Tabindex/focus comes from the underlying <button>. */
  .answer-form__opt {
    all: unset;
    box-sizing: border-box;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-height: 34px;
    padding: 8px 11px;
    border-radius: 10px;
    background: var(--color-surface-app);
    border: 1px solid var(--color-card-border);
    font-family: var(--font-system);
    color: var(--color-text-primary);
  }

  .answer-form__opt:hover:not([disabled]) {
    background: var(--color-surface-rail);
  }

  .answer-form__opt[data-selected="true"] {
    background: #fff7e8;
    border-color: #f2dca7;
  }

  .answer-form__opt[disabled] {
    cursor: not-allowed;
    opacity: 0.7;
  }

  .answer-form__opt-label {
    font-size: var(--font-size-body);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-regular);
  }

  .answer-form__opt-desc {
    font-size: 12px;
    line-height: 16px;
    color: var(--color-text-secondary);
  }

  .answer-form__submit {
    align-self: flex-end;
    padding: 8px 16px;
    border-radius: 10px;
    background: var(--color-action-cream);
    border: 1px solid var(--color-action-cream-border);
    color: var(--color-action-cream-text);
    font-family: var(--font-system);
    font-size: var(--font-size-button);
    line-height: var(--line-height-button);
    font-weight: var(--font-weight-medium);
    cursor: pointer;
  }

  .answer-form__submit:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
