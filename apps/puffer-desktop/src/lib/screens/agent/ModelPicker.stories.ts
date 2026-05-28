import type { Meta, StoryObj } from "@storybook/svelte-vite";
import ModelPicker from "./ModelPicker.svelte";
import { mockSettingsSnapshot } from "../../mockData";
import type { SettingsSnapshot } from "../../types";
import { storyModels } from "../../storybook/mockModels";

const snapshot: SettingsSnapshot = {
  ...mockSettingsSnapshot,
  config: {
    ...mockSettingsSnapshot.config,
    defaultProvider: "worldrouter",
    defaultModel: "qwen3-coder-plus"
  },
  auth: [
    ...mockSettingsSnapshot.auth,
    {
      providerId: "worldrouter",
      kind: "api_key",
      email: null,
      expiresAtMs: null,
      scopes: [],
      planType: null,
      organizationName: null
    }
  ],
  providers: [
    ...mockSettingsSnapshot.providers,
    {
      id: "worldrouter",
      displayName: "WorldRouter",
      baseUrl: "https://inference-api.worldrouter.ai/v1",
      defaultApi: "openai-completions",
      modelCount: 67,
      authModes: ["api_key"],
      sourceKind: "resourcepack",
      sourcePath: "resources/providers/worldrouter.yaml"
    }
  ]
};

const meta = {
  title: "Agent/ModelPicker",
  component: ModelPicker,
  parameters: {
    layout: "centered"
  },
  args: {
    snapshot,
    currentProvider: "worldrouter",
    currentModel: "qwen3-coder-plus",
    allowProviderSwitch: true,
    disabled: false,
    modelLoader: async () => storyModels,
    onChange: () => {}
  }
} satisfies Meta<typeof ModelPicker>;

export default meta;
type Story = StoryObj<typeof meta>;

export const CurrentModel: Story = {};

export const Disabled: Story = {
  args: {
    disabled: true
  }
};
