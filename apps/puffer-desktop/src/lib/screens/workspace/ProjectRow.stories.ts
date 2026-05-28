import type { Meta, StoryObj } from "@storybook/svelte-vite";
import ProjectRow from "./ProjectRow.svelte";
import { AGENTS, PROJECTS } from "../../data/mockProjects";
import StoryFrame from "../../storybook/StoryFrame.svelte";

const project = PROJECTS[0];

const meta = {
  title: "Workspace/ProjectRow",
  component: ProjectRow,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "padding: 24px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    project,
    agents: AGENTS.filter((agent) => agent.project === project.id),
    pinned: true
  }
} satisfies Meta<typeof ProjectRow>;

export default meta;
type Story = StoryObj<typeof meta>;

export const WithAgents: Story = {};

export const Empty: Story = {
  args: {
    agents: []
  }
};
