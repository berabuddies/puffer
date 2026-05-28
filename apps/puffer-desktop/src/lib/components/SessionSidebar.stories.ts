import type { Meta, StoryObj } from "@storybook/svelte-vite";
import SessionSidebar from "./SessionSidebar.svelte";
import { mockFolders } from "../mockData";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Components/SessionSidebar",
  component: SessionSidebar,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "width: 320px; height: 720px;"
      }
    })
  ],
  args: {
    groups: mockFolders,
    activeSessionId: "session-b",
    loading: false
  }
} satisfies Meta<typeof SessionSidebar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const ActiveSession: Story = {};

export const Loading: Story = {
  args: {
    groups: [],
    activeSessionId: null,
    loading: true
  }
};

export const Empty: Story = {
  args: {
    groups: [],
    activeSessionId: null,
    loading: false
  }
};
