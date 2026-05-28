import type { Meta, StoryObj } from "@storybook/svelte-vite";
import DiffView from "./DiffView.svelte";
import { mockSessionDetail } from "../mockData";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Components/DiffView",
  component: DiffView,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "height: 760px; padding: 16px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    diff: mockSessionDetail.latestDiff!,
    compact: false
  }
} satisfies Meta<typeof DiffView>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Full: Story = {};

export const Compact: Story = {
  args: {
    compact: true
  }
};
