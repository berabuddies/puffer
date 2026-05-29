import type { Meta, StoryObj } from "@storybook/svelte-vite";
import ConversationPane from "./ConversationPane.svelte";
import { mockSessionDetail } from "../mockData";
import { storyPermissionItem } from "../storybook/mockTimeline";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Components/ConversationPane",
  component: ConversationPane,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "height: 820px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    session: mockSessionDetail.session,
    timeline: mockSessionDetail.timeline,
    loading: false,
    noDiffMessage: null,
    pendingPermissions: []
  }
} satisfies Meta<typeof ConversationPane>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Transcript: Story = {};

export const PendingPermission: Story = {
  args: {
    pendingPermissions: [storyPermissionItem]
  }
};

export const Loading: Story = {
  args: {
    session: null,
    timeline: [],
    loading: true
  }
};
