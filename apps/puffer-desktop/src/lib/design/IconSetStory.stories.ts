import type { Meta, StoryObj } from "@storybook/svelte-vite";
import IconSetStory from "./IconSetStory.svelte";
import type { IconName } from "./Icon.svelte";

const icons: IconName[] = [
  "file",
  "folder",
  "terminal",
  "play",
  "settings",
  "sparkles",
  "key",
  "shield",
  "server",
  "bot"
];

const meta = {
  title: "Design/Icon Set",
  component: IconSetStory,
  parameters: {
    layout: "centered"
  },
  args: {
    icons
  }
} satisfies Meta<typeof IconSetStory>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Set: Story = {};
