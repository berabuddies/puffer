<script lang="ts">
  import Icon from "../../design/Icon.svelte";
  import type {
    MonitorRuleAddRequest,
    MonitorRuleMode,
    MonitorRuleOperator,
    MonitorRuleSchema,
    WorkflowBinding,
    WorkflowFilterRule
  } from "../../types";
  import {
    MESSAGE_TEXT_PATH,
    coerceRuleValue,
    defaultValueKeyForDetail,
    monitorRuleChipsForMode,
    monitorRuleDetails,
    operatorLabel,
    valueOptionsForDetail,
    valueRequiredForOperator,
    type MonitorRuleChip,
    type MonitorRuleDetail
  } from "./monitorRules";

  type Props = {
    binding: WorkflowBinding | null;
    schema?: MonitorRuleSchema | null;
    savingMode: MonitorRuleMode | null;
    deletingKey: string | null;
    onAddRule: (request: MonitorRuleAddRequest) => Promise<void> | void;
    onDeleteRule: (
      mode: MonitorRuleMode,
      rule: WorkflowFilterRule,
      key: string
    ) => Promise<void> | void;
  };

  let {
    binding,
    schema = null,
    savingMode,
    deletingKey,
    onAddRule,
    onDeleteRule
  }: Props = $props();

  let openMode = $state<MonitorRuleMode | null>(null);
  let selectedPath = $state(MESSAGE_TEXT_PATH);
  let selectedOperator = $state<MonitorRuleOperator>("contains");
  let selectedValue = $state("");
  let activeBindingSlug = "";

  let details = $derived(monitorRuleDetails(schema));
  let eventTextDetail = $derived(details.find((detail) => detail.target === "event_text"));
  let payloadDetails = $derived(details.filter((detail) => detail.target === "payload"));
  let selectedDetail = $derived(details.find((detail) => detail.path === selectedPath) ?? details[0] ?? null);
  let selectedOperators = $derived(selectedDetail?.operators ?? []);
  let selectedValueOptions = $derived(selectedDetail ? valueOptionsForDetail(selectedDetail) : []);
  let selectedNeedsValue = $derived(valueRequiredForOperator(selectedOperator));
  let includeChips = $derived(monitorRuleChipsForMode(binding, "include", schema));
  let excludeChips = $derived(monitorRuleChipsForMode(binding, "exclude", schema));
  let adding = $derived(openMode !== null && savingMode === openMode);

  $effect(() => {
    const nextSlug = binding?.slug ?? "";
    if (activeBindingSlug === nextSlug) return;
    activeBindingSlug = nextSlug;
    closeBuilder();
  });

  $effect(() => {
    if (details.some((detail) => detail.path === selectedPath)) return;
    resetBuilderControls();
  });

  function startBuilder(mode: MonitorRuleMode) {
    openMode = mode;
    resetBuilderControls();
  }

  function closeBuilder() {
    openMode = null;
    resetBuilderControls();
  }

  function resetBuilderControls() {
    const first = details[0];
    selectedPath = first?.path ?? MESSAGE_TEXT_PATH;
    selectedOperator = first?.operators[0] ?? "contains";
    selectedValue = first ? defaultValueKeyForDetail(first) : "";
  }

  function detailForPath(path: string): MonitorRuleDetail {
    return details.find((detail) => detail.path === path) ?? details[0];
  }

  function onDetailChange(event: Event) {
    const detail = detailForPath((event.currentTarget as HTMLSelectElement).value);
    selectedPath = detail.path;
    selectedOperator = detail.operators[0] ?? "contains";
    selectedValue = defaultValueKeyForDetail(detail);
  }

  function onConditionChange(event: Event) {
    selectedOperator = (event.currentTarget as HTMLSelectElement).value as MonitorRuleOperator;
    if (selectedDetail) selectedValue = defaultValueKeyForDetail(selectedDetail);
  }

  function onValueInput(event: Event) {
    selectedValue = (event.currentTarget as HTMLInputElement | HTMLSelectElement).value;
  }

  async function submitCondition(event: SubmitEvent) {
    event.preventDefault();
    if (!binding || !openMode || !selectedDetail) return;
    const needsValue = valueRequiredForOperator(selectedOperator);
    const rawValue = selectedValue.trim();
    if (needsValue && rawValue.length === 0) return;

    const request: MonitorRuleAddRequest = selectedDetail.target === "event_text"
      ? {
          connection_slug: binding.connection_slug,
          mode: openMode,
          kind: "keyword",
          keywords: [rawValue],
          operator: selectedOperator,
          case_insensitive: true
        }
      : {
          connection_slug: binding.connection_slug,
          mode: openMode,
          kind: "field",
          field: selectedDetail.path,
          operator: selectedOperator,
          value: needsValue ? coerceRuleValue(selectedDetail, rawValue) : null
        };
    await onAddRule(request);
    closeBuilder();
  }

  async function deleteChip(chip: MonitorRuleChip) {
    await onDeleteRule(chip.mode, chip.rule, chip.key);
  }
</script>

<div class="pf-monitor-rule-editor">
  {#if binding}
    <section class="pf-monitor-rule-group" role="group" aria-label="Only create tasks when">
      <div class="pf-monitor-rule-group-head">
        <div>
          <strong>Only create tasks when</strong>
          <span>{includeChips.length === 0 ? "No required conditions" : `${includeChips.length} condition${includeChips.length === 1 ? "" : "s"}`}</span>
        </div>
        <button
          type="button"
          class="pf-monitor-rule-add-button"
          disabled={savingMode !== null || deletingKey !== null}
          onclick={() => startBuilder("include")}
        >
          <Icon name="plus" size={13} />Add task condition
        </button>
      </div>

      <div class="pf-monitor-rule-chip-list">
        {#each includeChips as chip (chip.key)}
          <span class="pf-monitor-rule-chip" data-mode={chip.mode} data-tone={chip.tone}>
            <span class="pf-monitor-rule-chip-text">
              <strong>{chip.detailLabel}</strong>
              <span>{chip.operatorLabel}</span>
              {#if chip.valueLabel}
                <strong>{chip.valueLabel}</strong>
              {/if}
            </span>
            <button
              type="button"
              aria-label={`Remove task condition ${chip.title}`}
              disabled={deletingKey !== null || savingMode !== null}
              onclick={() => void deleteChip(chip)}
            >
              <Icon name="x" size={10} />
            </button>
          </span>
        {/each}
      </div>

      {#if openMode === "include"}
        <form class="pf-monitor-rule-builder" onsubmit={(event) => void submitCondition(event)}>
          <label>
            <span>Message detail</span>
            <select aria-label="Message detail" value={selectedPath} onchange={onDetailChange}>
              <option value={MESSAGE_TEXT_PATH}>{eventTextDetail?.label ?? "Message text"}</option>
              {#each payloadDetails as detail (detail.path)}
                <option value={detail.path}>{detail.label}</option>
              {/each}
            </select>
          </label>
          <label>
            <span>Condition</span>
            <select aria-label="Condition" value={selectedOperator} onchange={onConditionChange}>
              {#each selectedOperators as operator (operator)}
                <option value={operator}>{operatorLabel(operator)}</option>
              {/each}
            </select>
          </label>
          {#if selectedNeedsValue}
            <label>
              <span>Value</span>
              {#if selectedValueOptions.length > 0}
                <select aria-label="Value" value={selectedValue} onchange={onValueInput}>
                  {#each selectedValueOptions as option (String(option.value))}
                    <option value={String(option.value)}>{option.label}</option>
                  {/each}
                </select>
              {:else}
                <input
                  aria-label="Value"
                  value={selectedValue}
                  placeholder="Value"
                  oninput={onValueInput}
                />
              {/if}
            </label>
          {/if}
          <div class="pf-monitor-rule-builder-actions">
            <button type="button" class="pf-secondary-button" onclick={closeBuilder}>Cancel</button>
            <button type="submit" class="pf-primary-button" disabled={adding}>
              {adding ? "Adding..." : "Add condition"}
            </button>
          </div>
        </form>
      {/if}
    </section>

    <section class="pf-monitor-rule-group" role="group" aria-label="Skip tasks when">
      <div class="pf-monitor-rule-group-head">
        <div>
          <strong>Skip tasks when</strong>
          <span>{excludeChips.length === 0 ? "No skip conditions" : `${excludeChips.length} condition${excludeChips.length === 1 ? "" : "s"}`}</span>
        </div>
        <button
          type="button"
          class="pf-monitor-rule-add-button"
          disabled={savingMode !== null || deletingKey !== null}
          onclick={() => startBuilder("exclude")}
        >
          <Icon name="plus" size={13} />Add skip condition
        </button>
      </div>

      <div class="pf-monitor-rule-chip-list">
        {#each excludeChips as chip (chip.key)}
          <span class="pf-monitor-rule-chip" data-mode={chip.mode} data-tone={chip.tone}>
            <span class="pf-monitor-rule-chip-text">
              <strong>{chip.detailLabel}</strong>
              <span>{chip.operatorLabel}</span>
              {#if chip.valueLabel}
                <strong>{chip.valueLabel}</strong>
              {/if}
            </span>
            <button
              type="button"
              aria-label={`Remove skip condition ${chip.title}`}
              disabled={deletingKey !== null || savingMode !== null}
              onclick={() => void deleteChip(chip)}
            >
              <Icon name="x" size={10} />
            </button>
          </span>
        {/each}
      </div>

      {#if openMode === "exclude"}
        <form class="pf-monitor-rule-builder" onsubmit={(event) => void submitCondition(event)}>
          <label>
            <span>Message detail</span>
            <select aria-label="Message detail" value={selectedPath} onchange={onDetailChange}>
              <option value={MESSAGE_TEXT_PATH}>{eventTextDetail?.label ?? "Message text"}</option>
              {#each payloadDetails as detail (detail.path)}
                <option value={detail.path}>{detail.label}</option>
              {/each}
            </select>
          </label>
          <label>
            <span>Condition</span>
            <select aria-label="Condition" value={selectedOperator} onchange={onConditionChange}>
              {#each selectedOperators as operator (operator)}
                <option value={operator}>{operatorLabel(operator)}</option>
              {/each}
            </select>
          </label>
          {#if selectedNeedsValue}
            <label>
              <span>Value</span>
              {#if selectedValueOptions.length > 0}
                <select aria-label="Value" value={selectedValue} onchange={onValueInput}>
                  {#each selectedValueOptions as option (String(option.value))}
                    <option value={String(option.value)}>{option.label}</option>
                  {/each}
                </select>
              {:else}
                <input
                  aria-label="Value"
                  value={selectedValue}
                  placeholder="Value"
                  oninput={onValueInput}
                />
              {/if}
            </label>
          {/if}
          <div class="pf-monitor-rule-builder-actions">
            <button type="button" class="pf-secondary-button" onclick={closeBuilder}>Cancel</button>
            <button type="submit" class="pf-primary-button" disabled={adding}>
              {adding ? "Adding..." : "Add condition"}
            </button>
          </div>
        </form>
      {/if}
    </section>
  {:else}
    <section class="pf-monitor-rule-group is-empty" role="group" aria-label="Only create tasks when">
      <div class="pf-monitor-rule-group-head">
        <div>
          <strong>Only create tasks when</strong>
          <span>No active monitor</span>
        </div>
      </div>
    </section>
    <section class="pf-monitor-rule-group is-empty" role="group" aria-label="Skip tasks when">
      <div class="pf-monitor-rule-group-head">
        <div>
          <strong>Skip tasks when</strong>
          <span>No active monitor</span>
        </div>
      </div>
    </section>
  {/if}
</div>
