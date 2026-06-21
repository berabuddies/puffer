import type {
  MonitorRuleFieldType,
  MonitorRuleMode,
  MonitorRuleOperator,
  MonitorRuleSchema,
  MonitorRuleSchemaField,
  MonitorRuleSchemaValue,
  WorkflowBinding,
  WorkflowFilterRule
} from "../../types";

export type MonitorRuleDetailTarget = "event_text" | "payload";

export type MonitorRuleDetail = {
  path: string;
  label: string;
  type: MonitorRuleFieldType;
  operators: MonitorRuleOperator[];
  values: MonitorRuleSchemaValue[];
  target: MonitorRuleDetailTarget;
  tone: string;
};

export type MonitorRuleChip = {
  key: string;
  mode: MonitorRuleMode;
  rule: WorkflowFilterRule;
  index: number;
  tone: string;
  title: string;
  detailLabel: string;
  operatorLabel: string;
  valueLabel: string;
};

export const MESSAGE_TEXT_PATH = "$text";

const FIELD_TONES = [
  "blue",
  "green",
  "amber",
  "rose",
  "teal",
  "violet",
  "cyan",
  "lime",
  "orange",
  "pink",
  "indigo",
  "emerald",
  "yellow",
  "red",
  "sky",
  "fuchsia",
  "slate",
  "mint",
  "coral",
  "plum"
];

const MESSAGE_TEXT_DETAIL: MonitorRuleDetail = {
  path: MESSAGE_TEXT_PATH,
  label: "Message text",
  type: "string",
  operators: ["contains", "equals", "matches"],
  values: [],
  target: "event_text",
  tone: "text"
};

const BOOLEAN_VALUES: MonitorRuleSchemaValue[] = [
  { value: true, label: "Yes" },
  { value: false, label: "No" }
];

export function monitorRuleDetails(schema: MonitorRuleSchema | null | undefined): MonitorRuleDetail[] {
  const fields = (schema?.fields ?? []).map(detailFromSchemaField);
  return [{ ...MESSAGE_TEXT_DETAIL, label: eventTextLabel(schema) }, ...fields];
}

export function eventTextLabel(schema: MonitorRuleSchema | null | undefined): string {
  if (schema?.event_source === "gmail-browser" || schema?.event_source === "email") {
    return "Email content";
  }
  if (schema?.event_source === "gcal-browser") {
    return "Event content";
  }
  return "Message text";
}

export function detailFromSchemaField(field: MonitorRuleSchemaField, index = 0): MonitorRuleDetail {
  const values = field.values ?? (field.type === "boolean" ? BOOLEAN_VALUES : []);
  return {
    path: field.path,
    label: field.label || field.path,
    type: field.type,
    operators: field.operators.length > 0 ? field.operators : defaultOperatorsForType(field.type),
    values,
    target: "payload",
    tone: FIELD_TONES[index % FIELD_TONES.length]
  };
}

export function defaultOperatorsForType(type: MonitorRuleFieldType): MonitorRuleOperator[] {
  if (type === "exists") return ["exists"];
  if (type === "boolean" || type === "enum") return ["equals"];
  return ["contains", "equals", "matches"];
}

export function operatorLabel(operator: MonitorRuleOperator): string {
  if (operator === "equals") return "is";
  if (operator === "matches") return "matches regex";
  if (operator === "exists") return "exists";
  return "contains";
}

export function valueOptionsForDetail(detail: MonitorRuleDetail): MonitorRuleSchemaValue[] {
  if (detail.values.length > 0) return detail.values;
  if (detail.type === "boolean") return BOOLEAN_VALUES;
  return [];
}

export function valueKey(value: string | number | boolean): string {
  return String(value);
}

export function defaultValueKeyForDetail(detail: MonitorRuleDetail): string {
  const options = valueOptionsForDetail(detail);
  return options.length > 0 ? valueKey(options[0].value) : "";
}

export function valueRequiredForOperator(operator: MonitorRuleOperator): boolean {
  return operator !== "exists";
}

export function coerceRuleValue(detail: MonitorRuleDetail, rawValue: string): string | number | boolean {
  const option = valueOptionsForDetail(detail).find((candidate) => valueKey(candidate.value) === rawValue);
  if (option) return option.value;
  if (detail.type === "number") {
    const parsed = Number(rawValue);
    return Number.isFinite(parsed) ? parsed : rawValue;
  }
  return rawValue;
}

export function includeRules(binding: WorkflowBinding | null | undefined): WorkflowFilterRule[] {
  if (!binding) return [];
  if (binding.include_filters && binding.include_filters.length > 0) return binding.include_filters;
  return binding.include_filter ? [binding.include_filter] : [];
}

export function monitorRuleChipsForMode(
  binding: WorkflowBinding | null | undefined,
  mode: MonitorRuleMode,
  schema: MonitorRuleSchema | null | undefined
): MonitorRuleChip[] {
  const rules = mode === "include" ? includeRules(binding) : binding?.ignore_filters ?? [];
  const details = monitorRuleDetails(schema);
  return rules
    .map((rule, index) => chipFromRule(mode, rule, index, details))
    .sort(compareRuleChips);
}

function compareRuleChips(left: MonitorRuleChip, right: MonitorRuleChip): number {
  return left.title.localeCompare(right.title, undefined, {
    numeric: true,
    sensitivity: "base"
  }) || left.index - right.index;
}

function chipFromRule(
  mode: MonitorRuleMode,
  rule: WorkflowFilterRule,
  index: number,
  details: MonitorRuleDetail[]
): MonitorRuleChip {
  const parsed = parsedRule(rule, details);
  return {
    key: `${mode}:${index}:${JSON.stringify(rule)}`,
    mode,
    rule,
    index,
    tone: parsed.detail.tone,
    title: `${parsed.detail.label} ${operatorLabel(parsed.operator)}${parsed.valueLabel ? ` ${parsed.valueLabel}` : ""}`,
    detailLabel: parsed.detail.label,
    operatorLabel: operatorLabel(parsed.operator),
    valueLabel: parsed.valueLabel
  };
}

function parsedRule(
  rule: WorkflowFilterRule,
  details: MonitorRuleDetail[]
): { detail: MonitorRuleDetail; operator: MonitorRuleOperator; valueLabel: string } {
  if (rule.type === "regex" && typeof rule.pattern === "string") {
    const parsed = parseRegexRule(rule.pattern);
    return {
      detail: detailByPath(details, MESSAGE_TEXT_PATH),
      operator: parsed.operator,
      valueLabel: parsed.valueLabel
    };
  }
  if (rule.type === "jq" && typeof rule.expression === "string") {
    const parsed = parseJqExpression(rule.expression, details);
    if (parsed) return parsed;
  }
  return {
    detail: { ...MESSAGE_TEXT_DETAIL, label: "Rule" },
    operator: "matches",
    valueLabel: fallbackRuleSummary(rule)
  };
}

function parseJqExpression(
  expression: string,
  details: MonitorRuleDetail[]
): { detail: MonitorRuleDetail; operator: MonitorRuleOperator; valueLabel: string } | null {
  const equalsMatch = expression.match(/^\.(?<path>[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)*) == (?<value>.+)$/);
  if (equalsMatch?.groups) {
    const detail = detailByPath(details, equalsMatch.groups.path);
    const parsedValue = parseJsonScalar(equalsMatch.groups.value);
    return {
      detail,
      operator: "equals",
      valueLabel: displayValue(detail, parsedValue)
    };
  }

  const existsMatch = expression.match(/^\.(?<path>[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)*) \| exists$/);
  if (existsMatch?.groups) {
    return {
      detail: detailByPath(details, existsMatch.groups.path),
      operator: "exists",
      valueLabel: ""
    };
  }

  const testMatch = expression.match(/^\.(?<path>[A-Za-z0-9_]+(?:\.[A-Za-z0-9_]+)*) \| test\("(?<pattern>(?:\\.|[^"\\])*)"\)$/);
  if (testMatch?.groups) {
    const detail = detailByPath(details, testMatch.groups.path);
    const pattern = parseJsonString(`"${testMatch.groups.pattern}"`) ?? testMatch.groups.pattern;
    if (detail.type === "exists" && pattern === ".+") {
      return { detail, operator: "exists", valueLabel: "" };
    }
    const containsLiteral = caseInsensitiveRegexLiteral(pattern);
    if (containsLiteral !== null) {
      return {
        detail,
        operator: "contains",
        valueLabel: decodeRegexLiteral(containsLiteral)
      };
    }
    return {
      detail,
      operator: "matches",
      valueLabel: pattern
    };
  }
  return null;
}

function detailByPath(details: MonitorRuleDetail[], path: string): MonitorRuleDetail {
  return details.find((detail) => detail.path === path)
    ?? {
      path,
      label: sentenceLabel(path),
      type: "string",
      operators: ["contains", "equals", "matches"],
      values: [],
      target: "payload",
      tone: FIELD_TONES[0]
    };
}

function displayValue(detail: MonitorRuleDetail, value: unknown): string {
  const option = detail.values.find((candidate) => candidate.value === value);
  if (option) return option.label;
  if (value === null || value === undefined) return "";
  return String(value);
}

function regexRuleLabel(pattern: string): string {
  return splitRegexAlternation(pattern)
    .map(displayRegexLiteral)
    .map((keyword) => keyword.trim())
    .filter(Boolean)
    .join(", ");
}

function parseRegexRule(pattern: string): { operator: MonitorRuleOperator; valueLabel: string } {
  const equalsMatch = pattern.match(/^\^\(\?:(?<inner>.*)\)\$$/s);
  if (equalsMatch?.groups) {
    return {
      operator: "equals",
      valueLabel: regexRuleLabel(equalsMatch.groups.inner)
    };
  }
  const matchesMatch = pattern.match(/^\(\?:(?<inner>.*)\)$/s);
  if (matchesMatch?.groups) {
    return {
      operator: "matches",
      valueLabel: matchesMatch.groups.inner
    };
  }
  return {
    operator: "contains",
    valueLabel: regexRuleLabel(pattern)
  };
}

function fallbackRuleSummary(rule: WorkflowFilterRule): string {
  if (rule.type === "jq" && typeof rule.expression === "string") return rule.expression;
  const scalarEntries = Object.entries(rule)
    .filter(([, value]) => value === null || ["string", "number", "boolean"].includes(typeof value))
    .map(([key, value]) => `${key}=${JSON.stringify(value)}`);
  return scalarEntries.length > 0 ? scalarEntries.join(" ") : JSON.stringify(rule);
}

function parseJsonScalar(raw: string): unknown {
  try {
    return JSON.parse(raw);
  } catch {
    return raw;
  }
}

function parseJsonString(raw: string): string | null {
  const parsed = parseJsonScalar(raw);
  return typeof parsed === "string" ? parsed : null;
}

function sentenceLabel(path: string): string {
  return path
    .split(".")
    .at(-1)
    ?.replace(/_/g, " ")
    .replace(/\b\w/g, (char) => char.toUpperCase())
    ?? path;
}

function splitRegexAlternation(pattern: string): string[] {
  const parts: string[] = [];
  let current = "";
  let escaped = false;
  for (const ch of pattern) {
    if (escaped) {
      current += `\\${ch}`;
      escaped = false;
    } else if (ch === "\\") {
      escaped = true;
    } else if (ch === "|") {
      parts.push(current);
      current = "";
    } else {
      current += ch;
    }
  }
  if (escaped) current += "\\";
  parts.push(current);
  return parts;
}

function decodeRegexLiteral(pattern: string): string {
  return pattern.replace(/\\([\\.^$*+?()[\]{}|/-])/g, "$1");
}

function displayRegexLiteral(pattern: string): string {
  return decodeRegexLiteral(caseInsensitiveRegexLiteral(pattern) ?? pattern);
}

function caseInsensitiveRegexLiteral(pattern: string): string | null {
  const match = pattern.match(/^\(\?i:(?<inner>.*)\)$/s);
  return match?.groups?.inner ?? null;
}
