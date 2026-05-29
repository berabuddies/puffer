import type { Meta, StoryObj } from "@storybook/svelte-vite";
import ButtonGallery from "./ButtonGallery.svelte";

const meta = {
  title: "Design/Buttons",
  component: ButtonGallery,
  parameters: {
    layout: "fullscreen"
  }
} satisfies Meta<typeof ButtonGallery>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Gallery: Story = {};
