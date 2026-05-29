import type { Meta, StoryObj } from "@storybook/svelte-vite";
import Sidebar from "./Sidebar.svelte";
import { storyAgents, storyUser } from "../storybook/mockShell";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Shell/Sidebar",
  component: Sidebar,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "height: 820px; display: flex; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    screen: "workspace",
    collapsed: false,
    width: 280,
    agents: storyAgents,
    activeAgentId: storyAgents[0]?.id ?? null,
    user: storyUser,
    onSelectScreen: () => {}
  }
} satisfies Meta<typeof Sidebar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Expanded: Story = {};

export const Collapsed: Story = {
  args: {
    collapsed: true
  }
};
