import type { Meta, StoryObj } from "@storybook/svelte-vite";
import AgentCard from "./AgentCard.svelte";
import { AGENTS } from "../../data/mockProjects";
import StoryFrame from "../../storybook/StoryFrame.svelte";

const meta = {
  title: "Workspace/AgentCard",
  component: AgentCard,
  parameters: {
    layout: "centered"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "width: 220px; padding: 16px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    a: AGENTS[0]
  }
} satisfies Meta<typeof AgentCard>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Running: Story = {};

export const Awaiting: Story = {
  args: {
    a: AGENTS.find((agent) => agent.status === "awaiting") ?? AGENTS[0]
  }
};

export const Review: Story = {
  args: {
    a: AGENTS.find((agent) => agent.status === "review") ?? AGENTS[0]
  }
};
