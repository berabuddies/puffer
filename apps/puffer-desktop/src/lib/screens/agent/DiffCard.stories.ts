import type { Meta, StoryObj } from "@storybook/svelte-vite";
import DiffCard from "./DiffCard.svelte";
import type { DiffSnapshot, DiffTimelineItem } from "../../types";
import StoryFrame from "../../storybook/StoryFrame.svelte";

const componentPatch = [
  "diff --git a/apps/puffer-desktop/src/lib/screens/agent/QuestionPrompt.svelte b/apps/puffer-desktop/src/lib/screens/agent/QuestionPrompt.svelte",
  "index 4f7e9f1..5ba28aa 100644",
  "--- a/apps/puffer-desktop/src/lib/screens/agent/QuestionPrompt.svelte",
  "+++ b/apps/puffer-desktop/src/lib/screens/agent/QuestionPrompt.svelte",
  "@@ -242,7 +242,7 @@",
  "     <span class=\"pf-question-head-left\">",
  "       <Icon name={answered ? \"check\" : \"sparkles\"} size={14} color=\"var(--puffer-accent)\" />",
  "-      <span>{answered ? \"Answered\" : \"Prompt\"}</span>",
  "+      <span>{answered ? \"Answered\" : \"Question\"}</span>",
  "     </span>",
  "     {#if answered}",
  "       <span class=\"pf-question-summary\">{answerSummary()}</span>"
].join("\n");

const multiFilePatch = [
  "diff --git a/apps/puffer-desktop/src/lib/screens/agent/Approval.stories.ts b/apps/puffer-desktop/src/lib/screens/agent/Approval.stories.ts",
  "new file mode 100644",
  "--- /dev/null",
  "+++ b/apps/puffer-desktop/src/lib/screens/agent/Approval.stories.ts",
  "@@ -0,0 +1,4 @@",
  "+import type { Meta, StoryObj } from \"@storybook/svelte-vite\";",
  "+import Approval from \"./Approval.svelte\";",
  "+",
  "+export const BashCommand = {};",
  "diff --git a/apps/puffer-desktop/src/lib/screens/agent/DiffCard.stories.ts b/apps/puffer-desktop/src/lib/screens/agent/DiffCard.stories.ts",
  "new file mode 100644",
  "--- /dev/null",
  "+++ b/apps/puffer-desktop/src/lib/screens/agent/DiffCard.stories.ts",
  "@@ -0,0 +1,4 @@",
  "+import type { Meta, StoryObj } from \"@storybook/svelte-vite\";",
  "+import DiffCard from \"./DiffCard.svelte\";",
  "+",
  "+export const Expanded = {};"
].join("\n");

function diffSnapshot(overrides: Partial<DiffSnapshot>): DiffSnapshot {
  return {
    id: overrides.id ?? "diff-question-prompt",
    source: overrides.source ?? "agent",
    title: overrides.title ?? "QuestionPrompt label fix",
    command: overrides.command ?? "git diff -- apps/puffer-desktop/src/lib/screens/agent/QuestionPrompt.svelte",
    status: overrides.status ?? "1 file changed, 1 insertion(+), 1 deletion(-)",
    unstagedDiffstat: overrides.unstagedDiffstat ?? "QuestionPrompt.svelte | 2 +-",
    stagedDiffstat: overrides.stagedDiffstat ?? "",
    patch: overrides.patch ?? componentPatch
  };
}

function diffItem(overrides: Partial<DiffTimelineItem>): DiffTimelineItem {
  const diff = overrides.diff ?? diffSnapshot({});
  return {
    id: overrides.id ?? diff.id,
    kind: "diff",
    title: overrides.title ?? diff.title,
    summary: overrides.summary ?? diff.status,
    body: overrides.body ?? diff.patch,
    meta: overrides.meta ?? [diff.command],
    diff,
    actor: overrides.actor ?? null,
    createdAtMs: overrides.createdAtMs ?? Date.now()
  };
}

const meta = {
  title: "Agent/DiffCard",
  component: DiffCard,
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
    item: diffItem({}),
    defaultCollapsed: false
  }
} satisfies Meta<typeof DiffCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Expanded: Story = {};

export const Collapsed: Story = {
  args: {
    defaultCollapsed: true
  }
};

export const MultiFile: Story = {
  args: {
    item: diffItem({
      id: "diff-multi-file",
      diff: diffSnapshot({
        id: "diff-multi-file",
        title: "Add final interaction card stories",
        command: "git diff -- apps/puffer-desktop/src/lib/screens/agent",
        status: "2 files changed, 8 insertions(+)",
        unstagedDiffstat: "Approval.stories.ts | 4 ++++\nDiffCard.stories.ts | 4 ++++",
        patch: multiFilePatch
      })
    })
  }
};
