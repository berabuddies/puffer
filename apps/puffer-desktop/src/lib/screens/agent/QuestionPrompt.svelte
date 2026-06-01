<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import type { AskUserQuestionItem, UserQuestionTimelineItem } from "../../types";

  type Answers = Record<string, string | string[]>;
  type Annotations = Record<string, Record<string, string>>;
  type MarkdownPart =
    | { kind: "text"; text: string }
    | { kind: "image"; alt: string; src: string };

  type Props = {
    item: UserQuestionTimelineItem;
    disabled?: boolean;
    onResolve: (questionId: string, answers: Answers, annotations?: Annotations) => void;
  };

  let { item, disabled = false, onResolve }: Props = $props();
  let selectedAnswers = $state<Answers>({});
  let customText = $state<Record<string, string>>({});
  let customActive = $state<Record<string, boolean>>({});
  let searchText = $state<Record<string, string>>({});
  let collapsed = $state(false);
  let lastItemId: string | null = null;

  let answered = $derived(item.status !== "pending");

  $effect(() => {
    if (item.id === lastItemId) return;
    lastItemId = item.id;
    collapsed = answered;
  });

  function answerKeyFor(question: AskUserQuestionItem, index?: number): string {
    const duplicateCount = item.questions.filter(
      (candidate) => candidate.question === question.question
    ).length;
    if (duplicateCount <= 1) return question.question;
    const prefix = question.header?.trim();
    if (prefix) return `${prefix}: ${question.question}`;
    return typeof index === "number" ? `${question.question} #${index + 1}` : question.question;
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
      ? item.answers?.[answerKeyFor(question, index)]
      : selectedAnswers[draftKeyFor(index)];
    return Array.isArray(current) ? current.includes(label) : current === label;
  }

  function customValue(index: number): string {
    return customText[draftKeyFor(index)] ?? "";
  }

  function searchValue(index: number): string {
    return searchText[draftKeyFor(index)] ?? "";
  }

  function searchTerms(query: string): string[] {
    return query
      .trim()
      .toLowerCase()
      .split(/\s+/)
      .filter(Boolean);
  }

  function optionSearchText(option: AskUserQuestionItem["options"][number]): string {
    return [option.label, option.description].join(" ").toLowerCase();
  }

  function optionMatches(option: AskUserQuestionItem["options"][number], query: string): boolean {
    const terms = searchTerms(query);
    if (terms.length === 0) return true;
    const searchText = optionSearchText(option);
    return terms.every((term) => searchText.includes(term));
  }

  function filteredOptions(question: AskUserQuestionItem, index: number): AskUserQuestionItem["options"] {
    if (!question.searchable) return question.options;
    const query = searchValue(index);
    return question.options.filter((option) => optionMatches(option, query));
  }

  function searchSummary(
    question: AskUserQuestionItem,
    index: number,
    visibleCount: number
  ): string {
    const total = question.options.length;
    const query = searchValue(index).trim();
    if (!query) return total === 1 ? "1 option" : `${total} options`;
    return visibleCount === 1 ? `1/${total} match` : `${visibleCount}/${total} matches`;
  }

  function emptySearchMessage(index: number): string {
    const query = searchValue(index).trim();
    if (!query) return "No options available.";
    return `No options match "${query}".`;
  }

  function setSearch(question: AskUserQuestionItem, index: number, value: string) {
    const key = draftKeyFor(index);
    searchText = { ...searchText, [key]: value };
    const selected = selectedAnswers[key];
    if (typeof selected === "string") {
      const selectedOption = question.options.find((option) => option.label === selected);
      if (selectedOption && !optionMatches(selectedOption, value)) {
        const { [key]: _removed, ...rest } = selectedAnswers;
        selectedAnswers = rest;
      }
    }
  }

  function customChecked(question: AskUserQuestionItem, index: number): boolean {
    if (answered) return customAnswers(question, index).length > 0;
    const key = draftKeyFor(index);
    const text = customValue(index).trim();
    if (!text) return false;
    return question.multiSelect || customActive[key] === true;
  }

  function customAnswers(question: AskUserQuestionItem, index?: number): string[] {
    const answer = item.answers?.[answerKeyFor(question, index)];
    const values = Array.isArray(answer) ? answer : typeof answer === "string" ? [answer] : [];
    const optionLabels = new Set(question.options.map((option) => option.label));
    return values.filter((value) => !optionLabels.has(value));
  }

  function answerSummary(): string {
    const answers = item.answers ?? {};
    return item.questions
      .map((question) => {
        const index = item.questions.indexOf(question);
        const answer = answers[answerKeyFor(question, index)];
        if (!answer) return null;
        return Array.isArray(answer) ? answer.join(", ") : answer;
      })
      .filter((value): value is string => Boolean(value))
      .join(" · ");
  }

  function answerFor(question: AskUserQuestionItem, index: number): string | string[] | null {
    const key = draftKeyFor(index);
    const custom = customValue(index).trim();
    if (question.type === "input") return custom ? custom : null;
    if (question.searchable) {
      const selected = selectedAnswers[key];
      return typeof selected === "string" && selected.trim() ? selected : null;
    }
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
      if (answer !== null) next[answerKeyFor(question, index)] = answer;
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

  function questionHint(question: AskUserQuestionItem): string {
    if (question.type === "input") return "Type the requested value.";
    if (question.searchable) return "Search options, then choose one.";
    return question.multiSelect
      ? "Choose one or more options, or enter a custom answer."
      : "Choose one option, or enter a custom answer.";
  }

  function displayAnswer(question: AskUserQuestionItem, index: number): string {
    const answer = item.answers?.[answerKeyFor(question, index)];
    if (Array.isArray(answer)) return answer.join(", ");
    if (typeof answer === "string") return answer;
    return customValue(index);
  }

  function submit() {
    if (answered || disabled || !canSubmit()) return;
    onResolve(item.id, buildAnswers(), {});
  }

  function markdownImageParts(value: string | null | undefined): MarkdownPart[] {
    if (!value) return [];
    const parts: MarkdownPart[] = [];
    const imagePattern = /!\[([^\]\n]*)\]\(([^)\s]+)\)/g;
    let cursor = 0;
    for (const match of value.matchAll(imagePattern)) {
      const start = match.index ?? 0;
      if (start > cursor) {
        parts.push({ kind: "text", text: value.slice(cursor, start) });
      }
      const src = safeImageSrc(match[2] ?? "");
      if (src) {
        parts.push({ kind: "image", alt: match[1] ?? "", src });
      } else {
        parts.push({ kind: "text", text: match[0] });
      }
      cursor = start + match[0].length;
    }
    if (cursor < value.length) {
      parts.push({ kind: "text", text: value.slice(cursor) });
    }
    return parts.length > 0 ? parts : [{ kind: "text", text: value }];
  }

  function safeImageSrc(src: string): string | null {
    const trimmed = src.trim();
    if (/^data:image\/(?:png|jpe?g|gif|webp|svg\+xml);base64,[a-z0-9+/=]+$/i.test(trimmed)) {
      return trimmed;
    }
    if (/^https?:\/\//i.test(trimmed)) return trimmed;
    return null;
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
        <div class="pf-question-title">
          {#each markdownImageParts(question.question) as part, partIndex (`question-${index}-${partIndex}`)}
            {#if part.kind === "image"}
              <img
                class="pf-question-markdown-image"
                src={part.src}
                alt={part.alt || "Question image"}
              />
            {:else}
              <span class="pf-question-markdown-text">{part.text}</span>
            {/if}
          {/each}
        </div>
        {#if !answered}
          <div class="pf-question-hint">
            {questionHint(question)}
          </div>
        {/if}
        {#if question.type === "input"}
          <label
            class="pf-question-input"
            data-readonly={answered || disabled}
          >
            {#if answered}
              <div class="pf-question-input-readonly">{displayAnswer(question, index)}</div>
            {:else}
              <input
                class="pf-question-direct-input"
                value={customValue(index)}
                disabled={disabled}
                placeholder="Type answer"
                oninput={(event) =>
                  setCustom(question, index, (event.currentTarget as HTMLInputElement).value)}
              />
            {/if}
          </label>
        {:else}
          {@const visibleOptions = filteredOptions(question, index)}
          {#if question.searchable && !answered}
            <label class="pf-question-search">
              <Icon name="search" size={13} />
              <input
                value={searchValue(index)}
                disabled={disabled}
                placeholder="Search options"
                oninput={(event) =>
                  setSearch(question, index, (event.currentTarget as HTMLInputElement).value)}
              />
              <span class="pf-question-search-status">
                {searchSummary(question, index, visibleOptions.length)}
              </span>
            </label>
          {/if}
          <div class="pf-question-options" data-multi={question.multiSelect === true}>
            {#each visibleOptions as option (option.label)}
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
                    <div class="pf-question-preview">
                      {#each markdownImageParts(option.preview) as part, partIndex (`preview-${index}-${option.label}-${partIndex}`)}
                        {#if part.kind === "image"}
                          <img
                            class="pf-question-markdown-image"
                            src={part.src}
                            alt={part.alt || `${option.label} preview image`}
                          />
                        {:else}
                          <span class="pf-question-markdown-text">{part.text}</span>
                        {/if}
                      {/each}
                    </div>
                  {/if}
                </span>
                {#if answered && checked(question, index, option.label)}
                  <span class="pf-question-selected">Selected</span>
                {/if}
              </label>
            {/each}
          </div>
          {#if question.searchable && visibleOptions.length === 0 && !answered}
            <div class="pf-question-empty">{emptySearchMessage(index)}</div>
          {/if}
        {/if}
        {#if question.type !== "input" && !question.searchable && (!answered || customAnswers(question, index).length > 0)}
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
                {customAnswers(question, index).join(", ")}
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

  .pf-question-markdown-text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .pf-question-markdown-image {
    display: block;
    max-width: min(260px, 100%);
    max-height: 260px;
    object-fit: contain;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: #fff;
    padding: 8px;
    margin: 6px 0;
  }

  .pf-question-hint {
    color: var(--muted-foreground);
    font-size: 12px;
    line-height: 1.35;
  }

  .pf-question-search,
  .pf-question-input {
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--background);
    color: var(--foreground);
  }

  .pf-question-search {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 0 10px;
  }

  .pf-question-search-status {
    flex-shrink: 0;
    color: var(--muted-foreground);
    font-size: 11.5px;
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  .pf-question-search input,
  .pf-question-direct-input {
    width: 100%;
    min-width: 0;
    border: 0;
    outline: 0;
    background: transparent;
    color: var(--foreground);
    padding: 8px 0;
    font: inherit;
  }

  .pf-question-input {
    display: block;
    padding: 0 10px;
  }

  .pf-question-input[data-readonly="true"] {
    padding: 8px 10px;
  }

  .pf-question-input-readonly {
    min-width: 0;
    color: var(--foreground);
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .pf-question-empty {
    color: var(--muted-foreground);
    border: 1px dashed var(--border);
    border-radius: 8px;
    padding: 8px 10px;
    font-size: 12px;
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
  .pf-question-preview {
    color: var(--muted-foreground);
    line-height: 1.35;
  }

  .pf-question-preview {
    margin: 4px 0 0;
    white-space: pre-wrap;
    font-family: var(--font-mono);
    font-size: 11px;
    background: color-mix(in oklab, var(--muted) 70%, var(--background));
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 7px;
  }

  .pf-question-preview .pf-question-markdown-image {
    max-width: min(220px, 100%);
    max-height: 220px;
    margin: 2px 0 6px;
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
