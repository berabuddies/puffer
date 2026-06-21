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
import { monitorRuleChipsForMode, monitorRuleDetails } from "./monitorRules";

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
  test("customizes event text labels per connector", () => {
    const cases: Array<{ schema: MonitorRuleSchema; label: string }> = [
      { schema: telegramUserSchemaJson as MonitorRuleSchema, label: "Message text" },
      { schema: gmailSchemaJson as MonitorRuleSchema, label: "Email content" },
      { schema: gcalSchemaJson as MonitorRuleSchema, label: "Event content" },
      { schema: emailSchemaJson as MonitorRuleSchema, label: "Email content" },
      { schema: telegramBotSchemaJson as MonitorRuleSchema, label: "Message text" },
      { schema: larkLoginSchemaJson as MonitorRuleSchema, label: "Message text" },
      { schema: larkBotSchemaJson as MonitorRuleSchema, label: "Message text" }
    ];

    for (const { schema, label } of cases) {
      expect(monitorRuleDetails(schema)[0].label).toBe(label);
      const [chip] = monitorRuleChipsForMode(
        bindingWithRule(schema.event_source ?? "connector", "include", {
          type: "regex",
          pattern: "invoice",
          case_insensitive: true
        }),
        "include",
        schema
      );
      expect(chip.title).toBe(`${label} contains invoice`);
    }
  });

  test("keeps Telegram chat fields user-facing", () => {
    const fields = telegramUserSchemaJson.fields ?? [];

    expect(fields.find((field) => field.path === "group_channel_name")?.label).toBe("Group/Channel Name");
    expect(fields.some((field) => field.path === "chat_title")).toBe(false);
    expect(fields.some((field) => field.path === "chat_username")).toBe(false);
    expect(fields.some((field) => field.path === "is_outgoing")).toBe(false);
  });

  test("keeps Telegram bot fields limited to user-facing message routing details", () => {
    const fields = telegramBotSchemaJson.fields ?? [];

    expect(fields.map((field) => [field.path, field.label])).toEqual([
      ["is_group", "Group chat"],
      ["bot_mentioned", "Bot mentioned"]
    ]);
    expect(fields.some((field) => field.path === "conversation_id")).toBe(false);
    expect(fields.some((field) => field.path === "user_id")).toBe(false);
    expect(fields.some((field) => field.path === "thread_id")).toBe(false);
    expect(fields.some((field) => field.path === "from_bot")).toBe(false);
  });

  test("keeps Gmail fields focused on user-facing message details", () => {
    const fields = gmailSchemaJson.fields ?? [];

    expect(fields.map((field) => [field.path, field.label])).toEqual([
      ["message.sender", "Sender name"],
      ["message.fromEmail", "From email"],
      ["message.subject", "Subject"],
      ["message.snippet", "Snippet"],
      ["message.unread", "Unread"],
      ["message.hasAttachment", "Has attachment"]
    ]);
    expect(fields.some((field) => field.path === "account")).toBe(false);
    expect(fields.some((field) => field.path === "message.threadId")).toBe(false);
    expect(fields.some((field) => field.path === "message.url")).toBe(false);
  });

  test("keeps Email fields aligned with Gmail user-facing message details", () => {
    const fields = emailSchemaJson.fields ?? [];

    expect(emailSchemaJson.text_fields?.map((field) => field.label)).toEqual(["Subject", "Snippet"]);
    expect(fields.map((field) => [field.path, field.label])).toEqual([
      ["sender_name", "Sender name"],
      ["from", "From email"],
      ["subject", "Subject"],
      ["body_preview", "Snippet"],
      ["unread", "Unread"],
      ["has_attachment", "Has attachment"]
    ]);
    expect(fields.some((field) => field.path === "thread_id")).toBe(false);
    expect(fields.some((field) => field.path === "message_id")).toBe(false);
    expect(fields.some((field) => field.path === "uid")).toBe(false);
    expect(fields.some((field) => field.path === "date_ms")).toBe(false);
  });

  test("keeps GCal fields focused on user-facing event details", () => {
    const fields = gcalSchemaJson.fields ?? [];

    expect(fields.map((field) => [field.path, field.label])).toEqual([["event.title", "Event title"]]);
    expect(fields.some((field) => field.path === "account")).toBe(false);
    expect(fields.some((field) => field.path === "event.id")).toBe(false);
    expect(fields.some((field) => field.path === "event.index")).toBe(false);
    expect(fields.some((field) => field.path === "event.when")).toBe(false);
    expect(fields.some((field) => field.path === "event.location")).toBe(false);
    expect(fields.some((field) => field.path === "event.url")).toBe(false);
  });

  test("keeps Lark fields limited to stable user-facing event fields", () => {
    for (const schema of [larkLoginSchemaJson, larkBotSchemaJson]) {
      const fields = schema.fields ?? [];

      expect(fields.map((field) => [field.path, field.label])).toEqual([
        ["message_type", "Message type"],
        ["chat_type", "Chat type"]
      ]);
      expect(fields.some((field) => field.path === "chat_id")).toBe(false);
      expect(fields.some((field) => field.path === "sender_open_id")).toBe(false);
      expect(fields.some((field) => field.path === "message_id")).toBe(false);
      expect(fields.some((field) => field.path === "create_time")).toBe(false);
    }
  });

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
      for (const { slug, schema } of SCHEMAS) {
        const detailLabel = monitorRuleDetails(schema)[0].label;
        const title = testCase.title.replace("Message text", detailLabel);
        for (const mode of ["include", "exclude"] satisfies MonitorRuleMode[]) {
          const [chip] = monitorRuleChipsForMode(bindingWithRule(slug, mode, testCase.rule), mode, schema);

          expect(chip.title, `${slug} ${testCase.name} ${mode}`).toBe(title);
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
