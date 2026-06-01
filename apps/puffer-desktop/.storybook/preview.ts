import "../src/app.css";
import "../src/lib/design/chat.css";
import "../src/lib/design/deployments.css";
import "../src/lib/design/settings.css";
import "../src/lib/design/tasks.css";
import "../src/lib/design/workflow.css";
import "../src/lib/design/workspace.css";
import "./preview-docs.css";

import type { Preview } from "@storybook/svelte-vite";

const preview: Preview = {
  parameters: {
    controls: {
      matchers: {
        color: /(background|color)$/i,
        date: /Date$/i
      }
    },
    backgrounds: {
      default: "puffer",
      values: [
        { name: "puffer", value: "#f6f4ef" },
        { name: "dark", value: "#111827" },
        { name: "white", value: "#ffffff" }
      ]
    },
    layout: "fullscreen"
  }
};

export default preview;
