import type { Meta, StoryObj } from "@storybook/svelte-vite";
import MessageBody from "./MessageBody.svelte";
import { storyMarkdown } from "../storybook/mockTimeline";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Components/MessageBody",
  component: MessageBody,
  parameters: {
    layout: "centered"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "width: min(720px, 90vw); padding: 24px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    body: storyMarkdown
  }
} satisfies Meta<typeof MessageBody>;

export default meta;
type Story = StoryObj<typeof meta>;

export const RichMarkdown: Story = {};

export const PlainText: Story = {
  args: {
    body: "A compact assistant message with no markdown, useful for dense transcript rows."
  }
};
