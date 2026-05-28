import type { Meta, StoryObj } from "@storybook/svelte-vite";
import LoginView from "./LoginView.svelte";
import { mockSettingsSnapshot } from "../mockData";
import type { SettingsSnapshot } from "../types";
import StoryFrame from "../storybook/StoryFrame.svelte";

const worldrouterSnapshot: SettingsSnapshot = {
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

const emptySnapshot: SettingsSnapshot = {
  ...mockSettingsSnapshot,
  auth: [],
  providers: mockSettingsSnapshot.providers.map((provider) => ({
    ...provider,
    modelCount: Math.max(provider.modelCount, 1)
  }))
};

const meta = {
  title: "Components/LoginView",
  component: LoginView,
  parameters: {
    layout: "fullscreen"
  },
  decorators: [
    () => ({
      Component: StoryFrame,
      props: {
        style: "height: 760px; overflow: auto; background: var(--background); padding: 24px;"
      }
    })
  ],
  args: {
    snapshot: worldrouterSnapshot,
    loading: false,
    remoteEnabled: false,
    busyProviderId: null,
    errorMessage: null,
    externals: []
  }
} satisfies Meta<typeof LoginView>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Connected: Story = {};

export const NeedsProvider: Story = {
  args: {
    snapshot: emptySnapshot
  }
};

export const ErrorState: Story = {
  args: {
    errorMessage: "WorldRouter rejected the API key. Check the provider token and try again."
  }
};

export const RemoteMode: Story = {
  args: {
    remoteEnabled: true
  }
};
