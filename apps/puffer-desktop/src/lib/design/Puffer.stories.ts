import type { Meta, StoryObj } from "@storybook/svelte-vite";
import Puffer from "./Puffer.svelte";

const meta = {
  title: "Design/Puffer",
  component: Puffer,
  parameters: {
    layout: "centered"
  },
  args: {
    size: 72,
    state: "running"
  }
} satisfies Meta<typeof Puffer>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Running: Story = {};

export const Awaiting: Story = {
  args: {
    state: "awaiting"
  }
};
