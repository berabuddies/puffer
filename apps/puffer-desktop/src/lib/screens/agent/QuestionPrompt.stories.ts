import type { Meta, StoryObj } from "@storybook/svelte-vite";
import QuestionPrompt from "./QuestionPrompt.svelte";
import type { UserQuestionTimelineItem } from "../../types";
import StoryFrame from "../../storybook/StoryFrame.svelte";

function questionItem(overrides: Partial<UserQuestionTimelineItem>): UserQuestionTimelineItem {
  return {
    id: overrides.id ?? "question-color",
    kind: "question",
    title: overrides.title ?? "Question",
    summary: overrides.summary ?? "Agent asked for user input.",
    body: overrides.body ?? "",
    meta: overrides.meta ?? ["AskUserQuestion"],
    status: overrides.status ?? "pending",
    questions: overrides.questions ?? [
      {
        header: "颜色选择",
        question: "你喜欢绿色还是红色？",
        options: [
          { label: "绿色", description: "清新、自然、平静" },
          { label: "红色", description: "热情、醒目、有活力" }
        ]
      }
    ],
    answers: overrides.answers,
    actor: overrides.actor ?? null,
    createdAtMs: overrides.createdAtMs ?? Date.now()
  };
}

const meta = {
  title: "Agent/QuestionPrompt",
  component: QuestionPrompt,
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
    })
  ],
  args: {
    item: questionItem({}),
    disabled: false,
    onResolve: () => {}
  }
} satisfies Meta<typeof QuestionPrompt>;

export default meta;
type Story = StoryObj<typeof meta>;

export const SingleChoice: Story = {
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "width: min(1120px, 100%);"
      }
    })
  ]
};

export const SearchableChoice: Story = {
  args: {
    item: questionItem({
      id: "question-model",
      questions: [
        {
          header: "模型选择",
          question: "这次 agent 会话应该使用哪个模型？",
          searchable: true,
          options: [
            { label: "qwen3-coder-plus", description: "代码任务默认模型，速度和质量平衡" },
            { label: "claude-sonnet-4.5", description: "复杂推理和产品设计评审" },
            { label: "gpt-5-codex", description: "长上下文代码修改和测试" },
            { label: "worldrouter/auto", description: "由 WorldRouter 自动路由" }
          ]
        }
      ]
    })
  }
};

export const MultiSelect: Story = {
  args: {
    item: questionItem({
      id: "question-scope",
      questions: [
        {
          header: "覆盖范围",
          question: "Storybook 里需要继续补哪些 tool call 样式？",
          multiSelect: true,
          options: [
            { label: "文件操作", description: "Read / Write / Edit / Grep / Glob" },
            { label: "浏览器和网络", description: "WebSearch / WebFetch / BrowserAction" },
            { label: "任务和工作流", description: "TaskCreate / CronCreate / WorkflowCreate" },
            { label: "连接器", description: "Slack / Discord / Telegram / MCP" }
          ]
        }
      ]
    })
  }
};

export const DirectInput: Story = {
  args: {
    item: questionItem({
      id: "question-text",
      questions: [
        {
          header: "自定义说明",
          question: "这张卡片应该显示什么提示文案？",
          type: "input",
          options: []
        }
      ]
    })
  }
};

export const WithPreview: Story = {
  args: {
    item: questionItem({
      id: "question-layout",
      questions: [
        {
          header: "布局选择",
          question: "选择一个 QuestionPrompt 的展示密度。",
          options: [
            {
              label: "Compact",
              description: "更适合窄聊天流",
              preview: ".pf-question { padding: 10px 12px; gap: 10px; }"
            },
            {
              label: "Comfortable",
              description: "更适合完整详情面板",
              preview: ".pf-question { padding: 12px 14px; gap: 12px; }"
            }
          ]
        }
      ]
    })
  }
};

export const MultipleQuestions: Story = {
  args: {
    item: questionItem({
      id: "question-multiple",
      questions: [
        {
          header: "颜色选择",
          question: "你喜欢绿色还是红色？",
          options: [
            { label: "绿色", description: "清新、自然、平静" },
            { label: "红色", description: "热情、醒目、有活力" }
          ]
        },
        {
          header: "理由",
          question: "为什么这样选？",
          type: "input",
          options: []
        }
      ]
    })
  }
};

export const Answered: Story = {
  args: {
    item: questionItem({
      id: "question-answered",
      status: "answered",
      answers: {
        "你喜欢绿色还是红色？": "绿色"
      }
    })
  }
};

export const Resolving: Story = {
  args: {
    disabled: true,
    item: questionItem({
      id: "question-resolving",
      questions: [
        {
          header: "颜色选择",
          question: "你喜欢绿色还是红色？",
          options: [
            { label: "绿色", description: "清新、自然、平静" },
            { label: "红色", description: "热情、醒目、有活力" }
          ]
        }
      ]
    })
  }
};
