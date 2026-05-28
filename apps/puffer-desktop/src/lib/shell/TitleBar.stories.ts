import type { Meta, StoryObj } from "@storybook/svelte-vite";
import TitleBar from "./TitleBar.svelte";
import StoryFrame from "../storybook/StoryFrame.svelte";

const meta = {
  title: "Shell/TitleBar",
  component: TitleBar,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        className: "is-tauri is-macos",
        style: "height: 48px; background: var(--background); color: var(--foreground);"
      }
    })
  ],
  args: {
    onSearch: () => {}
  }
} satisfies Meta<typeof TitleBar>;

export default meta;
type Story = StoryObj<typeof meta>;

export const MacChrome: Story = {};
