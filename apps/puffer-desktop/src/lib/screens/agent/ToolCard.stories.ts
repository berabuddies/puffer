import type { Meta, StoryObj } from "@storybook/svelte-vite";
import ToolCard from "./ToolCard.svelte";
import type { ToolTimelineItem } from "../../types";
import StoryFrame from "../../storybook/StoryFrame.svelte";

function toolItem(overrides: Partial<ToolTimelineItem>): ToolTimelineItem {
  const toolName = overrides.toolName ?? "Bash";
  const status = overrides.status ?? "ok";
  return {
    id: overrides.id ?? `tool-${toolName.toLowerCase()}`,
    kind: "tool",
    title: overrides.title ?? `Tool call: ${toolName}`,
    summary: overrides.summary ?? `${toolName} completed.`,
    body: overrides.body ?? "",
    meta: overrides.meta ?? [toolName, status],
    toolName,
    status,
    input: overrides.input ?? "{}",
    output: overrides.output ?? "",
    inputJson: overrides.inputJson ?? null,
    metadata: overrides.metadata,
    subject: overrides.subject ?? null,
    createdAtMs: overrides.createdAtMs ?? Date.now()
  };
}

const bashOutput = {
  stdout: [
    "src/lib/screens/agent/ConversationView.svelte",
    "src/lib/screens/agent/ToolCard.svelte",
    "src/lib/design/chat.css"
  ].join("\n"),
  stderr: "",
  durationMs: 482
};

const readOutput = {
  file: {
    filePath: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
    startLine: 1018,
    content: [
      "function handleHeadClick() {",
      "  if (toggleable) collapsed = !collapsed;",
      "}",
      "",
      "<div class=\"pf-tool\" data-collapsed={collapsed}>"
    ].join("\n")
  }
};

const editOutput = {
  changes: [
    {
      path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
      patch: [
        "@@ -1018,6 +1018,7 @@",
        " function handleHeadClick() {",
        "+  if (!toggleable) return;",
        "   if (toggleable) collapsed = !collapsed;",
        " }"
      ].join("\n")
    }
  ]
};

const grepOutput = {
  mode: "content",
  numFiles: 3,
  numLines: 4,
  numMatches: 4,
  content: [
    "ConversationView.svelte:1839:<ToolCard",
    "ConversationView.svelte:1872:<ToolCard",
    "ToolCard.svelte:1022:<div class=\"pf-tool\"",
    "chat.css:147:.pf-tool"
  ].join("\n")
};

const globOutput = {
  filenames: [
    "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
    "apps/puffer-desktop/src/lib/screens/agent/ConversationView.svelte",
    "apps/puffer-desktop/src/lib/design/chat.css"
  ],
  numFiles: 3,
  durationMs: 36,
  hint: "Use Grep when you need content matches inside these files."
};

const writeOutput = {
  originalFile: [
    "export const toolGroups = [];",
    "export const enabled = false;"
  ].join("\n")
};

const lspOutput = {
  status: "ok",
  result: [
    {
      path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
      line: 1022,
      symbol: "ToolCard",
      kind: "component"
    },
    {
      path: "apps/puffer-desktop/src/lib/screens/agent/ConversationView.svelte",
      line: 1839,
      symbol: "ToolCard",
      kind: "reference"
    }
  ]
};

const mcpOutput = {
  status: "ok",
  durationMs: 104,
  result: {
    structuredContent: {
      tools: [
        { name: "get_issue", description: "Retrieve a Linear issue" },
        { name: "list_comments", description: "List issue comments" },
        { name: "save_comment", description: "Create or update a comment" }
      ]
    }
  }
};

const failedMcpOutput = {
  status: "error",
  durationMs: 91,
  error: {
    message: "Linear token is missing or expired."
  }
};

const webSearchOutput = {
  result: [
    "Storybook 10 supports Svelte CSF through @storybook/svelte-vite.",
    "Component stories can be authored as CSF objects with args.",
    "Decorators should return Svelte components rather than string templates."
  ].join("\n"),
  durationMs: 740,
  bytes: 18342
};

const httpOutput = {
  status: "ok",
  code: 200,
  codeText: "OK",
  durationMs: 128,
  bytes: 512,
  result: JSON.stringify({
    id: "evt_123",
    status: "queued",
    provider: "worldrouter"
  }, null, 2)
};

const browserOutput = {
  status: "ok",
  durationMs: 311,
  result: {
    title: "Puffer Storybook",
    url: "http://localhost:6006/?path=/story/agent-toolcard--bash-output"
  }
};

const questionOutput = {
  status: "answered",
  result: {
    question: "Pick which tool call style to inspect next",
    answer: "BrowserAction"
  }
};

const taskOutput = {
  status: "ok",
  result: {
    id: "task_42",
    title: "Audit Storybook coverage",
    state: "in_progress",
    updatedAt: "2026-05-28T09:38:00.000Z"
  }
};

const memoryOutput = {
  status: "saved",
  result: {
    key: "storybook.toolcall.coverage",
    value: "Agent tool calls should include file, terminal, browser, web, task, connector, multimedia, workflow, and MCP states."
  }
};

const connectorOutput = {
  status: "ok",
  result: {
    channel: "#design-system",
    messageTs: "1716889200.000100",
    delivered: true
  }
};

const imageOutput = {
  status: "ok",
  savedPath: "/Users/yuna/Documents/puffer/tmp/tool-card-preview.png",
  revisedPrompt: "A compact UI catalog card for agent tool call states."
};

const visionOutput = {
  status: "ok",
  result: {
    description: "The screenshot shows a table of agent tool categories.",
    labels: ["tool call", "storybook", "coverage"]
  }
};

const workflowOutput = {
  status: "ok",
  result: {
    workflowId: "wf_toolcall_catalog",
    nodes: 5,
    enabled: true
  }
};

const subagentOutput = {
  status: "ok",
  receiverThreadIds: ["thread_design_audit", "thread_component_audit"],
  agentsStates: {
    thread_design_audit: {
      status: "running",
      message: "Checking visual coverage."
    },
    thread_component_audit: {
      status: "complete",
      message: "Mapped tool types to stories."
    }
  }
};

const meta = {
  title: "Agent/ToolCard",
  component: ToolCard,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "min-height: 720px; padding: 32px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    defaultCollapsed: false,
    sessionId: null,
    item: toolItem({
      toolName: "Bash",
      summary: "Searched for the tool call renderer.",
      input: JSON.stringify({ command: "rg \"ToolCard|pf-tool\" src/lib -n" }),
      inputJson: { command: "rg \"ToolCard|pf-tool\" src/lib -n" },
      output: JSON.stringify(bashOutput, null, 2)
    })
  }
} satisfies Meta<typeof ToolCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const BashOutput: Story = {};

export const ReadFile: Story = {
  args: {
    item: toolItem({
      toolName: "Read",
      summary: "Loaded the tool renderer source.",
      input: JSON.stringify({ file_path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte" }),
      inputJson: { file_path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte" },
      output: JSON.stringify(readOutput, null, 2)
    })
  }
};

export const EditDiff: Story = {
  args: {
    item: toolItem({
      toolName: "Edit",
      summary: "Added a guard to the row toggle handler.",
      input: JSON.stringify({
        file_path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
        old_string: "if (toggleable) collapsed = !collapsed;",
        new_string: "if (!toggleable) return;\n  if (toggleable) collapsed = !collapsed;"
      }),
      inputJson: {
        file_path: "apps/puffer-desktop/src/lib/screens/agent/ToolCard.svelte",
        old_string: "if (toggleable) collapsed = !collapsed;",
        new_string: "if (!toggleable) return;\n  if (toggleable) collapsed = !collapsed;"
      },
      output: JSON.stringify(editOutput, null, 2)
    })
  }
};

export const WriteFile: Story = {
  args: {
    item: toolItem({
      toolName: "Write",
      summary: "Created a local Storybook coverage fixture.",
      input: JSON.stringify({
        file_path: "apps/puffer-desktop/src/lib/storybook/toolCoverage.ts",
        content: [
          "export const toolGroups = [",
          "  \"file\",",
          "  \"terminal\",",
          "  \"browser\"",
          "];"
        ].join("\n")
      }),
      inputJson: {
        file_path: "apps/puffer-desktop/src/lib/storybook/toolCoverage.ts",
        content: [
          "export const toolGroups = [",
          "  \"file\",",
          "  \"terminal\",",
          "  \"browser\"",
          "];"
        ].join("\n")
      },
      output: JSON.stringify(writeOutput, null, 2)
    })
  }
};

export const GlobResults: Story = {
  args: {
    item: toolItem({
      toolName: "Glob",
      summary: "Matched Storybook and agent view files by path.",
      input: JSON.stringify({ pattern: "apps/puffer-desktop/src/lib/**/*Tool*.svelte" }),
      inputJson: { pattern: "apps/puffer-desktop/src/lib/**/*Tool*.svelte" },
      output: JSON.stringify(globOutput, null, 2)
    })
  }
};

export const GrepResults: Story = {
  args: {
    item: toolItem({
      toolName: "Grep",
      summary: "Found all tool card usage sites.",
      input: JSON.stringify({ pattern: "ToolCard|pf-tool", path: "apps/puffer-desktop/src/lib" }),
      inputJson: { pattern: "ToolCard|pf-tool", path: "apps/puffer-desktop/src/lib" },
      output: JSON.stringify(grepOutput, null, 2)
    })
  }
};

export const NotebookEdit: Story = {
  args: {
    item: toolItem({
      toolName: "NotebookEdit",
      summary: "Updated a notebook cell used for evaluation notes.",
      input: JSON.stringify({
        notebook_path: "analysis/tool-call-coverage.ipynb",
        cell_id: "coverage-summary",
        new_source: "covered = len(tool_stories)"
      }),
      inputJson: {
        notebook_path: "analysis/tool-call-coverage.ipynb",
        cell_id: "coverage-summary",
        new_source: "covered = len(tool_stories)"
      },
      output: JSON.stringify({
        status: "ok",
        result: "Updated cell coverage-summary in analysis/tool-call-coverage.ipynb"
      }, null, 2)
    })
  }
};

export const ProcessControl: Story = {
  args: {
    item: toolItem({
      toolName: "ProcessControl",
      summary: "Checked the Storybook dev process.",
      input: JSON.stringify({ action: "status", process: "storybook", port: 6006 }),
      inputJson: { action: "status", process: "storybook", port: 6006 },
      output: JSON.stringify({
        status: "running",
        pid: 19980,
        port: 6006,
        uptime: "4m 12s"
      }, null, 2)
    })
  }
};

export const LspLookup: Story = {
  args: {
    item: toolItem({
      toolName: "LSP",
      summary: "Found component definition and references.",
      input: JSON.stringify({ action: "references", symbol: "ToolCard" }),
      inputJson: { action: "references", symbol: "ToolCard" },
      output: JSON.stringify(lspOutput, null, 2)
    })
  }
};

export const WebSearch: Story = {
  args: {
    item: toolItem({
      toolName: "WebSearch",
      summary: "Searched current Storybook Svelte guidance.",
      input: JSON.stringify({ query: "Storybook 10 Svelte CSF decorators" }),
      inputJson: { query: "Storybook 10 Svelte CSF decorators" },
      output: JSON.stringify(webSearchOutput, null, 2)
    })
  }
};

export const WebFetch: Story = {
  args: {
    item: toolItem({
      toolName: "WebFetch",
      summary: "Fetched a Storybook documentation page.",
      input: JSON.stringify({ url: "https://storybook.js.org/docs/svelte" }),
      inputJson: { url: "https://storybook.js.org/docs/svelte" },
      output: JSON.stringify({
        code: 200,
        codeText: "OK",
        durationMs: 223,
        bytes: 40580,
        result: "Storybook for Svelte uses the @storybook/svelte-vite framework package."
      }, null, 2)
    })
  }
};

export const HttpRequest: Story = {
  args: {
    item: toolItem({
      toolName: "HttpRequest",
      summary: "Sent a JSON request to a local service.",
      input: JSON.stringify({
        method: "POST",
        url: "http://localhost:1420/api/events",
        body: { kind: "storybook.preview" }
      }),
      inputJson: {
        method: "POST",
        url: "http://localhost:1420/api/events",
        body: { kind: "storybook.preview" }
      },
      output: JSON.stringify(httpOutput, null, 2)
    })
  }
};

export const BrowserAction: Story = {
  args: {
    item: toolItem({
      toolName: "BrowserAction",
      summary: "Clicked a Storybook story in the browser.",
      input: JSON.stringify({
        action: "click",
        url: "http://localhost:6006/",
        ref: "Agent / ToolCard"
      }),
      inputJson: {
        action: "click",
        url: "http://localhost:6006/",
        ref: "Agent / ToolCard"
      },
      output: JSON.stringify(browserOutput, null, 2)
    })
  }
};

export const AskUserQuestion: Story = {
  args: {
    item: toolItem({
      toolName: "AskUserQuestion",
      summary: "Asked the user to choose the next UI state.",
      input: JSON.stringify({
        question: "Which tool call style should be audited next?",
        options: ["BrowserAction", "TaskCreate", "MCP"]
      }),
      inputJson: {
        question: "Which tool call style should be audited next?",
        options: ["BrowserAction", "TaskCreate", "MCP"]
      },
      output: JSON.stringify(questionOutput, null, 2)
    })
  }
};

export const SendUserMessage: Story = {
  args: {
    item: toolItem({
      toolName: "SendUserMessage",
      summary: "Sent a follow-up message to the user.",
      input: JSON.stringify({
        message: "Tool call Storybook coverage is ready for review."
      }),
      inputJson: {
        message: "Tool call Storybook coverage is ready for review."
      },
      output: JSON.stringify({
        status: "delivered",
        result: "Message queued in current thread."
      }, null, 2)
    })
  }
};

export const TaskCreate: Story = {
  args: {
    item: toolItem({
      toolName: "TaskCreate",
      summary: "Created a tracked task.",
      input: JSON.stringify({
        title: "Finish tool call Storybook coverage",
        priority: "high"
      }),
      inputJson: {
        title: "Finish tool call Storybook coverage",
        priority: "high"
      },
      output: JSON.stringify(taskOutput, null, 2)
    })
  }
};

export const CronCreate: Story = {
  args: {
    item: toolItem({
      toolName: "CronCreate",
      summary: "Scheduled a recurring check.",
      input: JSON.stringify({
        name: "storybook-coverage-check",
        schedule: "weekly"
      }),
      inputJson: {
        name: "storybook-coverage-check",
        schedule: "weekly"
      },
      output: JSON.stringify({
        status: "active",
        nextRun: "2026-06-04T09:00:00.000Z"
      }, null, 2)
    })
  }
};

export const Memory: Story = {
  args: {
    item: toolItem({
      toolName: "Memory",
      summary: "Saved a project memory.",
      input: JSON.stringify({
        key: "storybook.toolcall.coverage",
        value: "Keep tool call styles complete in Storybook."
      }),
      inputJson: {
        key: "storybook.toolcall.coverage",
        value: "Keep tool call styles complete in Storybook."
      },
      output: JSON.stringify(memoryOutput, null, 2)
    })
  }
};

export const SlackAction: Story = {
  args: {
    item: toolItem({
      toolName: "SlackAction",
      summary: "Posted a design-system update.",
      input: JSON.stringify({
        channel: "#design-system",
        text: "Tool call styles are now cataloged."
      }),
      inputJson: {
        channel: "#design-system",
        text: "Tool call styles are now cataloged."
      },
      output: JSON.stringify(connectorOutput, null, 2)
    })
  }
};

export const ConnectorAction: Story = {
  args: {
    item: toolItem({
      toolName: "ConnectorAction",
      summary: "Sent a connector request to Telegram.",
      input: JSON.stringify({
        connector: "telegram",
        action: "send_message",
        chat: "design-review"
      }),
      inputJson: {
        connector: "telegram",
        action: "send_message",
        chat: "design-review"
      },
      output: JSON.stringify({
        status: "ok",
        result: {
          provider: "telegram",
          messageId: "tg_8821"
        }
      }, null, 2)
    })
  }
};

export const ImageGeneration: Story = {
  args: {
    item: toolItem({
      toolName: "ImageGeneration",
      summary: "Generated a visual preview asset.",
      input: JSON.stringify({
        prompt: "A compact UI catalog card for agent tool call states."
      }),
      inputJson: {
        prompt: "A compact UI catalog card for agent tool call states."
      },
      output: JSON.stringify(imageOutput, null, 2)
    })
  }
};

export const VisionAnalyze: Story = {
  args: {
    item: toolItem({
      toolName: "VisionAnalyze",
      summary: "Analyzed the uploaded tool capability screenshot.",
      input: JSON.stringify({
        path: "/Users/yuna/Library/Application Support/CleanShot/media/tool-call-table.png"
      }),
      inputJson: {
        path: "/Users/yuna/Library/Application Support/CleanShot/media/tool-call-table.png"
      },
      output: JSON.stringify(visionOutput, null, 2)
    })
  }
};

export const WorkflowCreate: Story = {
  args: {
    item: toolItem({
      toolName: "WorkflowCreate",
      summary: "Created an automation workflow.",
      input: JSON.stringify({
        name: "Tool call catalog QA",
        trigger: "storybook_change"
      }),
      inputJson: {
        name: "Tool call catalog QA",
        trigger: "storybook_change"
      },
      output: JSON.stringify(workflowOutput, null, 2)
    })
  }
};

export const Agent: Story = {
  args: {
    item: toolItem({
      toolName: "Agent",
      summary: "Spawned sub-agents to inspect coverage.",
      input: JSON.stringify({
        agent_type: "designer",
        model: "worldrouter/qwen3-coder-plus",
        reasoningEffort: "medium",
        message: "Audit the tool call Storybook coverage."
      }),
      inputJson: {
        agent_type: "designer",
        model: "worldrouter/qwen3-coder-plus",
        reasoningEffort: "medium",
        message: "Audit the tool call Storybook coverage."
      },
      output: JSON.stringify(subagentOutput, null, 2)
    })
  }
};

export const McpResults: Story = {
  args: {
    item: toolItem({
      toolName: "mcp__linear__list_tools",
      summary: "Listed Linear MCP tools.",
      input: JSON.stringify({
        server: "linear",
        tool: "list_tools",
        arguments: { query: "issue comments" }
      }),
      inputJson: {
        server: "linear",
        tool: "list_tools",
        arguments: { query: "issue comments" }
      },
      output: JSON.stringify(mcpOutput, null, 2)
    })
  }
};

export const SpotifyAction: Story = {
  args: {
    item: toolItem({
      toolName: "SpotifyAction",
      summary: "Controlled Spotify playback.",
      input: JSON.stringify({ action: "pause" }),
      inputJson: { action: "pause" },
      output: JSON.stringify({
        status: "ok",
        result: {
          device: "MacBook Pro",
          playing: false
        }
      }, null, 2)
    })
  }
};

export const ShopifyAction: Story = {
  args: {
    item: toolItem({
      toolName: "ShopifyAction",
      summary: "Updated a Shopify product draft.",
      input: JSON.stringify({
        action: "update_product",
        productId: "gid://shopify/Product/42"
      }),
      inputJson: {
        action: "update_product",
        productId: "gid://shopify/Product/42"
      },
      output: JSON.stringify({
        status: "ok",
        result: {
          title: "Puffer Tool Call Catalog",
          state: "draft"
        }
      }, null, 2)
    })
  }
};

export const ComputerUseAction: Story = {
  args: {
    item: toolItem({
      toolName: "ComputerUseAction",
      summary: "Clicked through a local desktop UI.",
      input: JSON.stringify({
        app: "Puffer",
        action: "click",
        target: "Agent / ToolCard"
      }),
      inputJson: {
        app: "Puffer",
        action: "click",
        target: "Agent / ToolCard"
      },
      output: JSON.stringify({
        status: "ok",
        result: "Clicked sidebar item and verified focus."
      }, null, 2)
    })
  }
};

export const FailedMcpCall: Story = {
  args: {
    item: toolItem({
      toolName: "mcp__linear__get_issue",
      status: "error",
      summary: "Linear rejected the request.",
      input: JSON.stringify({
        server: "linear",
        tool: "get_issue",
        arguments: { id: "PUF-123" }
      }),
      inputJson: {
        server: "linear",
        tool: "get_issue",
        arguments: { id: "PUF-123" }
      },
      output: JSON.stringify(failedMcpOutput, null, 2)
    })
  }
};

export const Running: Story = {
  args: {
    item: toolItem({
      toolName: "Bash",
      status: "running",
      summary: "Running the Storybook build.",
      input: JSON.stringify({ command: "npm run build-storybook" }),
      inputJson: { command: "npm run build-storybook" },
      output: ""
    })
  }
};

export const Collapsed: Story = {
  args: {
    defaultCollapsed: true
  }
};
