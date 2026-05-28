import type { Meta, StoryObj } from "@storybook/svelte-vite";
import BrandLogo from "./BrandLogo.svelte";

const meta = {
  title: "Design/BrandLogo",
  component: BrandLogo,
  parameters: {
    layout: "centered"
  },
  args: {
    size: 48,
    decorative: false
  }
} satisfies Meta<typeof BrandLogo>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Logo: Story = {};
