import type { Meta, StoryObj } from "@storybook/svelte-vite";
import StatePill from "./StatePill.svelte";

const meta = {
  title: "Deployments/StatePill",
  component: StatePill,
  parameters: {
    layout: "centered"
  },
  args: {
    state: "healthy"
  }
} satisfies Meta<typeof StatePill>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Healthy: Story = {};

export const Deploying: Story = {
  args: {
    state: "deploying"
  }
};

export const Failed: Story = {
  args: {
    state: "failed"
  }
};
