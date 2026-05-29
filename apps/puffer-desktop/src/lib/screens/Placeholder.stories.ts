import type { Meta, StoryObj } from "@storybook/svelte-vite";
import Placeholder from "./Placeholder.svelte";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Screens/Placeholder",
  component: Placeholder,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "height: 720px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    eyebrow: "Preview",
    title: "Puffer screen",
    subtitle: "Component catalog",
    sub: "A neutral empty state for screens that are still warming up.",
    chips: [
      { label: "Local", icon: "repo" },
      { label: "Ready", icon: "check" }
    ]
  }
} satisfies Meta<typeof Placeholder>;

export default meta;
type Story = StoryObj<typeof meta>;

export const EmptyScreen: Story = {};
