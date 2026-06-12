import { describe, expect, test } from "vitest";
import emailSchemaJson from "../../../../../../resources/subscribers/email/event_schema.json";
import gcalSchemaJson from "../../../../../../resources/subscribers/gcal-browser/event_schema.json";
import gmailSchemaJson from "../../../../../../resources/subscribers/gmail-browser/event_schema.json";
import telegramUserSchemaJson from "../../../../../../resources/subscribers/telegram-user/event_schema.json";
import larkBotSchemaJson from "../../../../../../resources/connectors/lark-bot/event_schema.json";
import larkLoginSchemaJson from "../../../../../../resources/connectors/lark-login/event_schema.json";
import telegramBotSchemaJson from "../../../../../../resources/connectors/telegram-bot/event_schema.json";
import type {
  MonitorRuleMode,
  MonitorRuleOperator,
  MonitorRuleSchema,
  MonitorRuleSchemaField,
  WorkflowBinding,
  WorkflowFilterRule
} from "../../types";
import { monitorRuleChipsForMode } from "./monitorRules";

type SchemaCase = {
  slug: string;
  schema: MonitorRuleSchema;
};

const SCHEMAS: SchemaCase[] = [
  { slug: "telegram-user", schema: telegramUserSchemaJson as MonitorRuleSchema },
  { slug: "gmail-browser", schema: gmailSchemaJson as MonitorRuleSchema },
  { slug: "gcal-browser", schema: gcalSchemaJson as MonitorRuleSchema },
  { slug: "email", schema: emailSchemaJson as MonitorRuleSchema },
  { slug: "telegram-bot", schema: telegramBotSchemaJson as MonitorRuleSchema },
  { slug: "lark-login", schema: larkLoginSchemaJson as MonitorRuleSchema },
  { slug: "lark-bot", schema: larkBotSchemaJson as MonitorRuleSchema }
];

describe("monitor rule chip labels", () => {
  test("render message text rules without generated regex wrappers", () => {
    const cases: Array<{
      name: string;
      rule: WorkflowFilterRule;
      title: string;
      operatorLabel: string;
      valueLabel: string;
    }> = [
      {
        name: "contains",
        rule: { type: "regex", pattern: "invoice", case_insensitive: true },
        title: "Message text contains invoice",
        operatorLabel: "contains",
        valueLabel: "invoice"
      },
      {
        name: "equals",
        rule: { type: "regex", pattern: "^(?:invoice)$", case_insensitive: true },
        title: "Message text is invoice",
        operatorLabel: "is",
        valueLabel: "invoice"
      },
      {
        name: "matches",
        rule: { type: "regex", pattern: "(?:invoice|receipt)", case_insensitive: true },
        title: "Message text matches regex invoice|receipt",
        operatorLabel: "matches regex",
        valueLabel: "invoice|receipt"
      }
    ];

    for (const testCase of cases) {
      for (const { slug } of SCHEMAS) {
        for (const mode of ["include", "exclude"] satisfies MonitorRuleMode[]) {
          const [chip] = monitorRuleChipsForMode(bindingWithRule(slug, mode, testCase.rule), mode, null);

          expect(chip.title, `${slug} ${testCase.name} ${mode}`).toBe(testCase.title);
          expect(chip.operatorLabel, `${slug} ${testCase.name} ${mode}`).toBe(testCase.operatorLabel);
          expect(chip.valueLabel, `${slug} ${testCase.name} ${mode}`).toBe(testCase.valueLabel);
          expect(chip.title, `${slug} ${testCase.name} ${mode}`).not.toContain("^(?:");
          expect(chip.title, `${slug} ${testCase.name} ${mode}`).not.toContain(")$");
        }
      }
    }
  });

  test("render every bundled connector field rule without backend jq or regex syntax", () => {
    for (const { slug, schema } of SCHEMAS) {
      for (const field of schema.fields ?? []) {
        for (const operator of field.operators) {
          for (const value of valuesForFieldOperator(field, operator)) {
            for (const mode of ["include", "exclude"] satisfies MonitorRuleMode[]) {
              const rule = compiledFieldRule(field, operator, value);
              const binding = bindingWithRule(slug, mode, rule);
              const [chip] = monitorRuleChipsForMode(binding, mode, schema);
              const expectedTitle = expectedChipTitle(field, operator, value);

              expect(chip, `${slug} ${mode} ${field.path} ${operator}`).toBeDefined();
              expect(chip.title, `${slug} ${field.path} ${operator} ${String(value)}`).toBe(expectedTitle);
              expect(chip.detailLabel, `${slug} ${field.path} ${operator}`).toBe(field.label);
              expect(chip.operatorLabel, `${slug} ${field.path} ${operator}`).toBe(operatorDisplayLabel(operator));
              expect(chip.valueLabel, `${slug} ${field.path} ${operator}`).toBe(expectedValueLabel(field, operator, value));
              expect(chip.title, `${slug} ${field.path} ${operator}`).not.toMatch(/^Rule matches regex /);
              expect(chip.title, `${slug} ${field.path} ${operator}`).not.toContain("(?i:");
              expect(chip.title, `${slug} ${field.path} ${operator}`).not.toContain("test(");
              expect(chip.title, `${slug} ${field.path} ${operator}`).not.toContain(" == ");
              expect(chip.title, `${slug} ${field.path} ${operator}`).not.toContain(" | exists");
              expect(chip.title, `${slug} ${field.path} ${operator}`).not.toContain(`.${field.path}`);
            }
          }
        }
      }
    }
  });
});

function valuesForFieldOperator(field: MonitorRuleSchemaField, operator: MonitorRuleOperator): Array<string | number | boolean | null> {
  if (operator === "exists") return [null];
  if (operator === "equals" && field.values?.length) {
    return field.values.map((value) => value.value);
  }
  if (operator === "equals" && field.type === "boolean") return [true, false];
  return [equalValue(field)];
}

function bindingWithRule(slug: string, mode: MonitorRuleMode, rule: WorkflowFilterRule): WorkflowBinding {
  return {
    slug: `monitor-${slug}`,
    description: `Monitor ${slug}`,
    connection_slug: slug,
    connector_slug: slug,
    status: "enabled",
    enabled: true,
    action_type: "triage_agent",
    include_filters: mode === "include" ? [rule] : [],
    ignore_filters: mode === "exclude" ? [rule] : []
  };
}

function compiledFieldRule(
  field: MonitorRuleSchemaField,
  operator: MonitorRuleOperator,
  value: string | number | boolean | null
): WorkflowFilterRule {
  if (operator === "exists") {
    return { type: "jq", expression: `.${field.path} | exists` };
  }
  if (operator === "equals") {
    return { type: "jq", expression: `.${field.path} == ${JSON.stringify(value)}` };
  }
  if (operator === "matches") {
    return { type: "jq", expression: `.${field.path} | test(${JSON.stringify(String(value))})` };
  }
  return { type: "jq", expression: `.${field.path} | test(${JSON.stringify(caseInsensitiveLiteral(String(value)))})` };
}

function equalValue(field: MonitorRuleSchemaField): string | number | boolean {
  if (field.values?.length) return field.values[0].value;
  if (field.type === "boolean") return true;
  if (field.type === "number") return 123;
  return "invoice";
}

function expectedChipTitle(
  field: MonitorRuleSchemaField,
  operator: MonitorRuleOperator,
  value: string | number | boolean | null
): string {
  const valueLabel = expectedValueLabel(field, operator, value);
  return `${field.label} ${operatorDisplayLabel(operator)}${valueLabel ? ` ${valueLabel}` : ""}`;
}

function expectedValueLabel(
  field: MonitorRuleSchemaField,
  operator: MonitorRuleOperator,
  value: string | number | boolean | null
): string {
  if (operator === "exists") return "";
  if (operator === "equals") {
    return field.values?.find((candidate) => candidate.value === value)?.label ?? String(value);
  }
  return String(value);
}

function operatorDisplayLabel(operator: MonitorRuleOperator): string {
  if (operator === "equals") return "is";
  if (operator === "matches") return "matches regex";
  if (operator === "exists") return "exists";
  return "contains";
}

function caseInsensitiveLiteral(value: string): string {
  return `(?i:${escapeRegexLiteral(value)})`;
}

function escapeRegexLiteral(value: string): string {
  return value.replace(/[\\^$.*+?()[\]{}|/-]/g, "\\$&");
}
