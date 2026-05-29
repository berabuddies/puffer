import type { Meta, StoryObj } from "@storybook/svelte-vite";
import Icon, { type IconName } from "./Icon.svelte";

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
  title: "Design/Icon",
  component: Icon,
  parameters: {
    layout: "centered"
  },
  args: {
    name: "sparkles",
    size: 18,
    strokeWidth: 1.8
  },
  argTypes: {
    name: {
      control: "select",
      options: icons
    }
  }
} satisfies Meta<typeof Icon>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Single: Story = {};
