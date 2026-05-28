import type { Meta, StoryObj } from "@storybook/svelte-vite";
import Approval from "./Approval.svelte";
import type { PermissionTimelineItem } from "../../types";
import StoryFrame from "../../storybook/StoryFrame.svelte";

function permissionItem(overrides: Partial<PermissionTimelineItem>): PermissionTimelineItem {
  const toolName = overrides.toolName ?? "Bash";
  return {
    id: overrides.id ?? `approval-${toolName.toLowerCase()}`,
    kind: "permission",
    title: overrides.title ?? "Permission request",
    summary: overrides.summary ?? "This tool call requires approval before the agent can continue.",
    body: overrides.body ?? "",
    meta: overrides.meta ?? ["required"],
    toolName,
    status: overrides.status ?? "required",
    permissionDialog: overrides.permissionDialog ?? {
      state: "required",
      reason: "Running a shell command requires confirmation.",
      summary: "npm run build-storybook",
      inputText: JSON.stringify({ command: "npm run build-storybook" }),
      toolName,
      choices: ["Approve once", "Always allow", "Deny"]
    },
    scopeLabel: overrides.scopeLabel ?? "workspace",
    choices: overrides.choices ?? ["Approve once", "Always allow", "Deny"],
    actor: overrides.actor ?? null,
    createdAtMs: overrides.createdAtMs ?? Date.now()
  };
}

const meta = {
  title: "Agent/Approval",
  component: Approval,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: [
          "min-height: 720px",
          "padding: 32px",
          "background: var(--background)",
          "color: var(--foreground)",
          "display: flex",
          "justify-content: center"
        ].join(";")
      }
    }),
    () => ({
      Component: StoryFrame,
      props: {
        style: "width: min(1120px, 100%);"
      }
    })
  ],
  args: {
    item: permissionItem({}),
    disabled: false,
    onResolve: () => {}
  }
} satisfies Meta<typeof Approval>;

export default meta;
type Story = StoryObj<typeof meta>;

export const BashCommand: Story = {};

export const EditFile: Story = {
  args: {
    item: permissionItem({
      id: "approval-edit",
      toolName: "Edit",
      summary: "Editing a source file requires approval.",
      permissionDialog: {
        state: "required",
        reason: "Workspace policy asks before modifying tracked files.",
        summary: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
        inputText: JSON.stringify({
          file_path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
          old_string: "AskUserQuestion",
          new_string: "QuestionPrompt"
        }),
        toolName: "Edit",
        choices: ["Approve once", "Always allow", "Deny"]
      }
    })
  }
};

export const ExternalConnector: Story = {
  args: {
    item: permissionItem({
      id: "approval-connector",
      toolName: "SlackAction",
      summary: "Posting to Slack needs confirmation.",
      choices: ["Send once", "Always allow SlackAction", "Deny"],
      permissionDialog: {
        state: "required",
        reason: "External connector action may send data outside the workspace.",
        summary: "Post message to #design-system",
        inputText: JSON.stringify({
          channel: "#design-system",
          text: "Tool call Storybook coverage is ready."
        }),
        toolName: "SlackAction",
        choices: ["Send once", "Always allow SlackAction", "Deny"]
      }
    })
  }
};

export const Resolving: Story = {
  args: {
    disabled: true,
    item: permissionItem({
      id: "approval-resolving",
      toolName: "Bash",
      summary: "Approval choice is being sent."
    })
  }
};
