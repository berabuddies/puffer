<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import type { AskUserQuestionItem, UserQuestionTimelineItem } from "../../types";

  type Answers = Record<string, string | string[]>;
  type Annotations = Record<string, Record<string, string>>;

  type Props = {
    item: UserQuestionTimelineItem;
    disabled?: boolean;
    onResolve: (questionId: string, answers: Answers, annotations?: Annotations) => void;
  };

  let { item, disabled = false, onResolve }: Props = $props();
  let selectedAnswers = $state<Answers>({});
  let customText = $state<Record<string, string>>({});
  let customActive = $state<Record<string, boolean>>({});
  let collapsed = $state(false);
  let lastItemId: string | null = null;

  let answered = $derived(item.status !== "pending");

  $effect(() => {
    if (item.id === lastItemId) return;
    lastItemId = item.id;
    collapsed = answered;
  });

  function answerKeyFor(question: AskUserQuestionItem): string {
    return question.question;
  }

  function draftKeyFor(index: number): string {
    return `${item.id}:${index}`;
  }

  function selectedList(index: number): string[] {
    const current = selectedAnswers[draftKeyFor(index)];
    return Array.isArray(current) ? current : [];
  }

  function selectSingle(index: number, label: string) {
    const key = draftKeyFor(index);
    selectedAnswers = { ...selectedAnswers, [key]: label };
    customActive = { ...customActive, [key]: false };
  }

  function toggleMulti(index: number, label: string) {
    const key = draftKeyFor(index);
    const list = selectedList(index);
    const next = list.includes(label) ? list.filter((v) => v !== label) : [...list, label];
    selectedAnswers = { ...selectedAnswers, [key]: next };
  }

  function setCustom(question: AskUserQuestionItem, index: number, value: string) {
    const key = draftKeyFor(index);
    customText = { ...customText, [key]: value };
    if (!question.multiSelect) {
      customActive = { ...customActive, [key]: true };
    }
  }

  function checked(question: AskUserQuestionItem, index: number, label: string): boolean {
    const current = answered
      ? item.answers?.[answerKeyFor(question)]
      : selectedAnswers[draftKeyFor(index)];
    return Array.isArray(current) ? current.includes(label) : current === label;
  }

  function customValue(index: number): string {
    return customText[draftKeyFor(index)] ?? "";
  }

  function customChecked(question: AskUserQuestionItem, index: number): boolean {
    if (answered) return customAnswers(question).length > 0;
    const key = draftKeyFor(index);
    const text = customValue(index).trim();
    if (!text) return false;
    return question.multiSelect || customActive[key] === true;
  }

  function customAnswers(question: AskUserQuestionItem): string[] {
    const answer = item.answers?.[answerKeyFor(question)];
    const values = Array.isArray(answer) ? answer : typeof answer === "string" ? [answer] : [];
    const optionLabels = new Set(question.options.map((option) => option.label));
    return values.filter((value) => !optionLabels.has(value));
  }

  function answerSummary(): string {
    const answers = item.answers ?? {};
    return item.questions
      .map((question) => {
        const answer = answers[answerKeyFor(question)];
        if (!answer) return null;
        return Array.isArray(answer) ? answer.join(", ") : answer;
      })
      .filter((value): value is string => Boolean(value))
      .join(" · ");
  }

  function answerFor(question: AskUserQuestionItem, index: number): string | string[] | null {
    const key = draftKeyFor(index);
    const custom = customValue(index).trim();
    if (question.multiSelect) {
      const values = selectedList(index);
      const withCustom = custom ? [...values, custom] : values;
      return withCustom.length > 0 ? withCustom : null;
    }
    if (customActive[key] === true && custom) return custom;
    const selected = selectedAnswers[key];
    return typeof selected === "string" && selected.trim() ? selected : null;
  }

  function buildAnswers(): Answers {
    const next: Answers = {};
    for (const [index, question] of item.questions.entries()) {
      const answer = answerFor(question, index);
      if (answer !== null) next[answerKeyFor(question)] = answer;
    }
    return next;
  }

  function hasAnswer(question: AskUserQuestionItem, index: number): boolean {
    const answer = answerFor(question, index);
    if (Array.isArray(answer)) return answer.length > 0;
    return typeof answer === "string" && answer.trim().length > 0;
  }

  function canSubmit(): boolean {
    if (answered || disabled) return false;
    return item.questions.every((question, index) => hasAnswer(question, index));
  }

  function submit() {
    if (answered || disabled || !canSubmit()) return;
    onResolve(item.id, buildAnswers(), {});
  }
</script>

<form
  class="pf-question"
  onsubmit={(event) => {
    event.preventDefault();
    submit();
  }}
>
  <button
    type="button"
    class="pf-question-head"
    onclick={() => {
      if (answered) collapsed = !collapsed;
    }}
    aria-expanded={answered ? !collapsed : undefined}
  >
    <span class="pf-question-head-left">
      <Icon name={answered ? "check" : "sparkles"} size={14} color="var(--puffer-accent)" />
      <span>{answered ? "Answered" : "Question"}</span>
    </span>
    {#if answered}
      <span class="pf-question-summary">{answerSummary()}</span>
      <Icon name={collapsed ? "chevR" : "chevD"} size={11} />
    {/if}
  </button>
  {#if !answered || !collapsed}
    {#each item.questions as question, index (draftKeyFor(index))}
      <div class="pf-question-block">
        <div class="pf-question-kicker">{question.header}</div>
        <div class="pf-question-title">{question.question}</div>
        {#if !answered}
          <div class="pf-question-hint">
            {question.multiSelect ? "Choose one or more options, or enter a custom answer." : "Choose one option, or enter a custom answer."}
          </div>
        {/if}
        <div class="pf-question-options" data-multi={question.multiSelect === true}>
          {#each question.options as option (option.label)}
            <label
              class="pf-question-option"
              data-selected={checked(question, index, option.label)}
              data-readonly={answered || disabled}
            >
              <input
                type={question.multiSelect ? "checkbox" : "radio"}
                name={`question-${item.id}-${index}`}
                checked={checked(question, index, option.label)}
                disabled={answered || disabled}
                onchange={() =>
                  question.multiSelect
                    ? toggleMulti(index, option.label)
                    : selectSingle(index, option.label)}
              />
              <span class="pf-question-option-body">
                <span>{option.label}</span>
                <small>{option.description}</small>
                {#if option.preview}
                  <pre>{option.preview}</pre>
                {/if}
              </span>
              {#if answered && checked(question, index, option.label)}
                <span class="pf-question-selected">Selected</span>
              {/if}
            </label>
          {/each}
        </div>
        {#if !answered || customAnswers(question).length > 0}
          <label
            class="pf-question-other"
            data-selected={customChecked(question, index)}
            data-readonly={answered || disabled}
          >
            <input
              class="pf-question-other-choice"
              type={question.multiSelect ? "checkbox" : "radio"}
              name={`question-${item.id}-${index}`}
              checked={customChecked(question, index)}
              disabled={answered || disabled}
              onchange={(event) => {
                const checked = (event.currentTarget as HTMLInputElement).checked;
                if (question.multiSelect) {
                  if (!checked) setCustom(question, index, "");
                  return;
                }
                customActive = { ...customActive, [draftKeyFor(index)]: true };
              }}
              aria-label="Use custom answer"
            />
            {#if answered}
              <div class="pf-question-other-readonly">
                {customAnswers(question).join(", ")}
              </div>
            {:else}
              <input
                class="pf-question-other-input"
                value={customValue(index)}
                disabled={disabled}
                placeholder="Type another answer"
                onfocus={() => {
                  if (!question.multiSelect) customActive = { ...customActive, [draftKeyFor(index)]: true };
                }}
                oninput={(event) =>
                  setCustom(question, index, (event.currentTarget as HTMLInputElement).value)}
              />
            {/if}
          </label>
        {/if}
      </div>
    {/each}
    {#if !answered}
      <div class="pf-question-actions">
        <button
          type="button"
          class="sc-btn"
          data-variant="default"
          data-size="sm"
          disabled={!canSubmit()}
          onclick={() => submit()}
        >
          Send answer
        </button>
      </div>
    {/if}
  {/if}
</form>

<style>
  .pf-question {
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 42%, var(--border));
    background: color-mix(in oklab, var(--puffer-accent) 5%, var(--background));
    border-radius: 10px;
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 12px;
    font-size: 13px;
  }

  .pf-question-head {
    all: unset;
    box-sizing: border-box;
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    font-size: 12.5px;
    font-weight: 600;
    cursor: default;
  }

  .pf-question-head[aria-expanded] {
    cursor: pointer;
  }

  .pf-question-head-left {
    display: inline-flex;
    align-items: center;
    gap: 8px;
    flex-shrink: 0;
  }

  .pf-question-summary {
    min-width: 0;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted-foreground);
    font-weight: 500;
    text-align: right;
  }

  .pf-question-block {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .pf-question-kicker {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 11px;
    text-transform: uppercase;
  }

  .pf-question-title {
    color: var(--foreground);
    font-weight: 600;
  }

  .pf-question-hint {
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 1.35;
  }

  .pf-question-options {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: 8px;
  }

  .pf-question-option {
    border: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    border-radius: 8px;
    padding: 9px 10px;
    text-align: left;
    display: flex;
    align-items: flex-start;
    gap: 8px;
    cursor: pointer;
  }

  .pf-question-option[data-selected="true"] {
    border-color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 10%, var(--background));
  }

  .pf-question-option[data-readonly="true"] {
    cursor: default;
  }

  .pf-question-option input,
  .pf-question-other-choice {
    accent-color: var(--puffer-accent);
    margin: 2px 0 0;
    flex-shrink: 0;
  }

  .pf-question-option-body {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 3px;
    flex: 1;
  }

  .pf-question-option-body > span {
    font-weight: 600;
  }

  .pf-question-option small,
  .pf-question-option pre {
    color: var(--muted-foreground);
    line-height: 1.35;
  }

  .pf-question-option pre {
    margin: 4px 0 0;
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: 11px;
    background: color-mix(in oklab, var(--muted) 70%, var(--background));
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 7px;
  }

  .pf-question-other {
    display: flex;
    align-items: center;
    gap: 8px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
    padding: 0 10px;
  }

  .pf-question-other[data-readonly="true"] {
    cursor: default;
    padding: 8px 10px;
  }

  .pf-question-other[data-selected="true"] {
    border-color: var(--puffer-accent);
    background: color-mix(in oklab, var(--puffer-accent) 8%, var(--background));
  }

  .pf-question-other-input {
    width: 100%;
    min-width: 0;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--foreground);
    padding: 8px 10px;
    font: inherit;
  }

  .pf-question-other-readonly {
    min-width: 0;
    color: var(--foreground);
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .pf-question-selected {
    align-self: flex-start;
    border: 1px solid color-mix(in oklab, var(--puffer-accent) 45%, var(--border));
    border-radius: 999px;
    padding: 2px 7px;
    color: var(--puffer-accent);
    font-size: 11px;
    font-weight: 700;
    flex-shrink: 0;
  }

  .pf-question-actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
