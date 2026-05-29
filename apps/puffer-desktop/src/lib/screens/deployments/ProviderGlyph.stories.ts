import type { Meta, StoryObj } from "@storybook/svelte-vite";
import ProviderGlyph from "./ProviderGlyph.svelte";

const meta = {
  title: "Deployments/ProviderGlyph",
  component: ProviderGlyph,
  parameters: {
    layout: "centered"
  },
  args: {
    kind: "vercel",
    size: 28
  },
  argTypes: {
    kind: {
      control: "select",
      options: ["vercel", "aws", "fly", "railway", "cloudflare", "supabase", "custom"]
    }
  }
} satisfies Meta<typeof ProviderGlyph>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Glyph: Story = {};
