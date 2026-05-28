import type { Meta, StoryObj } from "@storybook/svelte-vite";
import SidebarProjects from "./SidebarProjects.svelte";
import { storyAgents } from "../storybook/mockShell";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Shell/SidebarProjects",
  component: SidebarProjects,
  parameters: {
    layout: "centered"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "width: 248px; height: 560px; padding: 12px; background: #fafafa; color: var(--foreground);"
      }
    })
  ],
  args: {
    agents: storyAgents,
    activeAgentId: storyAgents[0]?.id ?? null,
    onOpenAgent: () => {},
    onToggleAgentPin: () => {}
  }
} satisfies Meta<typeof SidebarProjects>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithSections: Story = {};

export const LongNames: Story = {
  args: {
    agents: storyAgents.map((agent, index) => ({
      ...agent,
      name: index === 0 ? "Yuna session kickoff with a very long branch label" : agent.name,
      project: index < 3 ? "Very Long Workspace Project Name" : agent.project
    }))
  }
};

export const BusyPinned: Story = {
  args: {
    agents: storyAgents.map((agent, index) => ({
      ...agent,
      pinned: index % 2 === 0,
      pinBusy: index === 1
    }))
  }
};

export const Empty: Story = {
  args: {
    agents: [],
    activeAgentId: null
  }
};
