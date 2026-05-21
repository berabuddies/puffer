import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { createInterface } from "node:readline";
import { tmpdir } from "node:os";
import path from "node:path";
import { existsSync } from "node:fs";
import { expect, test } from "@playwright/test";

type DaemonHandshake = {
  url: string;
  token: string;
  workspaceRoot: string;
  protocolVersion: string;
};

type DaemonFixtureOptions = {
  openaiBaseUrl?: string;
  anthropicBaseUrl?: string;
  defaultProvider: string;
  defaultModel: string;
};

const repoRoot = path.resolve(process.cwd(), "../..");
const defaultPufferBinary = path.join(repoRoot, "target", "debug", "puffer");
const pufferBinary = process.env.PUFFER_DESKTOP_TEST_DAEMON ?? defaultPufferBinary;

test("real daemon UI can create an OpenAI-backed agent and render a reply", async ({ page }) => {
  test.skip(
    !existsSync(pufferBinary),
    `Build puffer first or set PUFFER_DESKTOP_TEST_DAEMON; missing ${pufferBinary}`
  );
  test.setTimeout(60_000);

  const mock = await OpenAiMock.start("Puffer smoke reply");
  const fixture = await DaemonFixture.start({
    openaiBaseUrl: mock.baseUrl,
    defaultProvider: "openai",
    defaultModel: "openai/gpt-5"
  });
  try {
    const params = new URLSearchParams({
      skipOnboarding: "1",
      corbinaBackend: fixture.handshake.url,
      corbinaToken: fixture.handshake.token
    });
    await page.goto(`/?${params.toString()}`);

    await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
    await page.getByRole("button", { name: "New agent in default workspace" }).click();
    const dialog = page.getByRole("dialog", { name: "New agent" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("radio", { name: /OpenAI|Codex/ })).toBeVisible();
    await dialog.getByRole("button", { name: "Start agent" }).click();

    const composer = page.locator(".pf-composer textarea");
    await expect(composer).toBeEnabled();
    await composer.fill("Say exactly: Puffer smoke reply");
    await page.getByRole("button", { name: "Send" }).click();

    await expect.poll(() => mock.responsesCalls, { timeout: 20_000 }).toBe(1);
    await expect(
      page.locator('.pf-msg[data-role="agent"]').filter({ hasText: "Puffer smoke reply" })
    ).toBeVisible();
    expect(mock.lastResponsesBody).toContain("Say exactly: Puffer smoke reply");
  } finally {
    await fixture.stop();
    await mock.stop();
  }
});

test("real daemon UI can create an Anthropic-backed agent and render a reply", async ({ page }) => {
  test.skip(
    !existsSync(pufferBinary),
    `Build puffer first or set PUFFER_DESKTOP_TEST_DAEMON; missing ${pufferBinary}`
  );
  test.setTimeout(60_000);

  const mock = await AnthropicMock.start("Claude smoke reply");
  const fixture = await DaemonFixture.start({
    anthropicBaseUrl: mock.baseUrl,
    defaultProvider: "anthropic",
    defaultModel: "anthropic/claude-sonnet-4-5"
  });
  try {
    const params = new URLSearchParams({
      skipOnboarding: "1",
      corbinaBackend: fixture.handshake.url,
      corbinaToken: fixture.handshake.token
    });
    await page.goto(`/?${params.toString()}`);

    await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
    await page.getByRole("button", { name: "New agent in default workspace" }).click();
    const dialog = page.getByRole("dialog", { name: "New agent" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("radio", { name: /Anthropic|Claude/ })).toBeVisible();
    await dialog.getByRole("button", { name: "Start agent" }).click();

    const composer = page.locator(".pf-composer textarea");
    await expect(composer).toBeEnabled();
    await composer.fill("Say exactly: Claude smoke reply");
    await page.getByRole("button", { name: "Send" }).click();

    await expect.poll(() => mock.messagesCalls, { timeout: 20_000 }).toBe(1);
    await expect(
      page.locator('.pf-msg[data-role="agent"]').filter({ hasText: "Claude smoke reply" })
    ).toBeVisible();
    expect(mock.lastMessagesBody).toContain("Say exactly: Claude smoke reply");
  } finally {
    await fixture.stop();
    await mock.stop();
  }
});

for (const scenario of [
  {
    label: "Codex alias",
    reply: "Codex alias smoke reply",
    expectedProvider: /OpenAI|Codex/,
    startMock: () => OpenAiMock.start("Codex alias smoke reply"),
    fixtureOptions: (baseUrl: string): DaemonFixtureOptions => ({
      openaiBaseUrl: baseUrl,
      defaultProvider: "codex",
      defaultModel: "codex/gpt-5"
    }),
    calls: (mock: OpenAiMock | AnthropicMock) => (mock as OpenAiMock).responsesCalls,
    lastBody: (mock: OpenAiMock | AnthropicMock) => (mock as OpenAiMock).lastResponsesBody
  },
  {
    label: "Claude alias",
    reply: "Claude alias smoke reply",
    expectedProvider: /Anthropic|Claude/,
    startMock: () => AnthropicMock.start("Claude alias smoke reply"),
    fixtureOptions: (baseUrl: string): DaemonFixtureOptions => ({
      anthropicBaseUrl: baseUrl,
      defaultProvider: "claude",
      defaultModel: "claude/claude-sonnet-4-5"
    }),
    calls: (mock: OpenAiMock | AnthropicMock) => (mock as AnthropicMock).messagesCalls,
    lastBody: (mock: OpenAiMock | AnthropicMock) => (mock as AnthropicMock).lastMessagesBody
  }
]) {
  test(`real daemon UI can create a ${scenario.label} agent and render a reply`, async ({
    page
  }) => {
    test.skip(
      !existsSync(pufferBinary),
      `Build puffer first or set PUFFER_DESKTOP_TEST_DAEMON; missing ${pufferBinary}`
    );
    test.setTimeout(60_000);

    const mock = await scenario.startMock();
    const fixture = await DaemonFixture.start(scenario.fixtureOptions(mock.baseUrl));
    try {
      const params = new URLSearchParams({
        skipOnboarding: "1",
        corbinaBackend: fixture.handshake.url,
        corbinaToken: fixture.handshake.token
      });
      await page.goto(`/?${params.toString()}`);

      await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
      await page.getByRole("button", { name: "New agent in default workspace" }).click();
      const dialog = page.getByRole("dialog", { name: "New agent" });
      await expect(dialog).toBeVisible();
      await expect(dialog.getByRole("radio", { name: scenario.expectedProvider })).toBeVisible();
      await dialog.getByRole("button", { name: "Start agent" }).click();

      const composer = page.locator(".pf-composer textarea");
      await expect(page.getByText(/Reconnect .* to continue this session\./)).toHaveCount(0);
      await expect(composer).toBeEnabled();
      await composer.fill(`Say exactly: ${scenario.reply}`);
      await page.getByRole("button", { name: "Send" }).click();

      await expect.poll(() => scenario.calls(mock), { timeout: 20_000 }).toBe(1);
      await expect(
        page.locator('.pf-msg[data-role="agent"]').filter({ hasText: scenario.reply })
      ).toBeVisible();
      expect(scenario.lastBody(mock)).toContain(`Say exactly: ${scenario.reply}`);
    } finally {
      await fixture.stop();
      await mock.stop();
    }
  });
}

class DaemonFixture {
  readonly handshake: DaemonHandshake;
  private readonly child: ChildProcessWithoutNullStreams;
  private readonly root: string;
  private stderr = "";

  private constructor(handshake: DaemonHandshake, child: ChildProcessWithoutNullStreams, root: string) {
    this.handshake = handshake;
    this.child = child;
    this.root = root;
  }

  static async start(options: DaemonFixtureOptions): Promise<DaemonFixture> {
    const root = await mkdtemp(path.join(tmpdir(), "puffer-desktop-ui-"));
    const workspace = path.join(root, "workspace");
    const pufferHome = path.join(root, "home");
    const pufferConfig = path.join(pufferHome, ".puffer");
    const discoveryCache = path.join(root, "discovery.json");
    await mkdir(workspace, { recursive: true });
    await mkdir(pufferConfig, { recursive: true });
    if (options.anthropicBaseUrl) {
      const workspaceProviders = path.join(workspace, ".puffer", "resources", "providers");
      await mkdir(workspaceProviders, { recursive: true });
      await writeFile(
        path.join(workspaceProviders, "anthropic.yaml"),
        anthropicProviderYaml(options.anthropicBaseUrl)
      );
    }
    await writeFile(
      path.join(pufferConfig, "auth.json"),
      JSON.stringify({
        format_version: 1,
        providers: {
          ...(options.openaiBaseUrl ? { openai: { kind: "api_key", key: "sk-test" } } : {}),
          ...(options.anthropicBaseUrl ? { anthropic: { kind: "api_key", key: "sk-ant-test" } } : {})
        }
      })
    );
    await writeFile(discoveryCache, discoveryCacheJson());
    const env: Record<string, string | undefined> = {
      ...process.env,
      PUFFER_HOME: pufferHome,
      PUFFER_BUILTIN_RESOURCES_DIR: path.join(repoRoot, "resources"),
      PUFFER_DISCOVERY_CACHE_PATH: discoveryCache
    };
    if (options.openaiBaseUrl) {
      env.OPENAI_BASE_URL = options.openaiBaseUrl;
    }

    const child = spawn(
      pufferBinary,
      [
        "daemon",
        "--bind",
        "127.0.0.1:0",
        "--token",
        "desktop-ui-token",
        "--print-handshake",
        "--no-browser",
        "--disable-auto-title"
      ],
      {
        cwd: workspace,
        env
      }
    );
    let stderr = "";
    child.stderr.on("data", (chunk) => {
      stderr += String(chunk);
    });

    const handshake = await readHandshake(child, () => stderr);
    await daemonRpc(handshake, "update_config", {
      ...(options.openaiBaseUrl ? { openaiBaseUrl: options.openaiBaseUrl } : {}),
      defaultProvider: options.defaultProvider,
      defaultModel: options.defaultModel
    });
    const fixture = new DaemonFixture(handshake, child, root);
    fixture.stderr = stderr;
    child.stderr.on("data", (chunk) => {
      fixture.stderr += String(chunk);
    });
    return fixture;
  }

  async stop(): Promise<void> {
    if (!this.child.killed) this.child.kill();
    await new Promise<void>((resolve) => {
      this.child.once("exit", () => resolve());
      setTimeout(resolve, 1_000);
    });
    const unexpectedStderr = this.stderr
      .split(/\r?\n/)
      .filter((line) => line.trim() && !line.startsWith("puffer daemon listening on "))
      .join("\n");
    if (unexpectedStderr.trim()) {
      console.error(`puffer daemon stderr:\n${unexpectedStderr}`);
    }
    await rm(this.root, { recursive: true, force: true });
  }
}

class OpenAiMock {
  readonly baseUrl: string;
  responsesCalls = 0;
  lastResponsesBody = "";
  private readonly server: Server;
  private readonly reply: string;

  private constructor(server: Server, baseUrl: string, reply: string) {
    this.server = server;
    this.baseUrl = baseUrl;
    this.reply = reply;
  }

  static async start(reply: string): Promise<OpenAiMock> {
    let mock: OpenAiMock | null = null;
    const server = createServer((request, response) => {
      if (mock) {
        void mock.handle(request, response);
      } else {
        response.writeHead(503, { "content-type": "text/plain" });
        response.end("mock not ready");
      }
    });
    await new Promise<void>((resolve) => {
      server.listen(0, "127.0.0.1", resolve);
    });
    const address = server.address();
    if (address === null || typeof address === "string") {
      throw new Error("mock server did not bind a TCP address");
    }
    const ready = new OpenAiMock(server, `http://127.0.0.1:${address.port}`, reply);
    mock = ready;
    return ready;
  }

  async stop(): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.server.close((error) => (error ? reject(error) : resolve()));
    });
  }

  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    if (request.url === "/v1/models") {
      writeJson(response, {
        data: [{ id: "gpt-5", name: "GPT 5 smoke" }]
      });
      return;
    }
    if (request.url === "/v1/responses") {
      this.responsesCalls += 1;
      this.lastResponsesBody = await readRequestBody(request);
      writeJson(response, {
        id: "resp_desktop_ui_smoke",
        status: "completed",
        output_text: this.reply,
        output: [
          {
            type: "message",
            role: "assistant",
            content: [{ type: "output_text", text: this.reply }]
          }
        ],
        usage: {
          input_tokens: 10,
          output_tokens: 4,
          input_tokens_details: { cached_tokens: 0 }
        }
      });
      return;
    }
    response.writeHead(404, { "content-type": "text/plain" });
    response.end("not found");
  }
}

class AnthropicMock {
  readonly baseUrl: string;
  messagesCalls = 0;
  lastMessagesBody = "";
  private readonly server: Server;
  private readonly reply: string;

  private constructor(server: Server, baseUrl: string, reply: string) {
    this.server = server;
    this.baseUrl = baseUrl;
    this.reply = reply;
  }

  static async start(reply: string): Promise<AnthropicMock> {
    let mock: AnthropicMock | null = null;
    const server = createServer((request, response) => {
      if (mock) {
        void mock.handle(request, response);
      } else {
        response.writeHead(503, { "content-type": "text/plain" });
        response.end("mock not ready");
      }
    });
    await new Promise<void>((resolve) => {
      server.listen(0, "127.0.0.1", resolve);
    });
    const address = server.address();
    if (address === null || typeof address === "string") {
      throw new Error("mock server did not bind a TCP address");
    }
    const ready = new AnthropicMock(server, `http://127.0.0.1:${address.port}`, reply);
    mock = ready;
    return ready;
  }

  async stop(): Promise<void> {
    await new Promise<void>((resolve, reject) => {
      this.server.close((error) => (error ? reject(error) : resolve()));
    });
  }

  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    if (request.url === "/v1/models") {
      writeJson(response, {
        data: [{ id: "claude-sonnet-4-5", display_name: "Claude Sonnet 4.5" }]
      });
      return;
    }
    if (request.url?.startsWith("/v1/messages")) {
      this.messagesCalls += 1;
      this.lastMessagesBody = await readRequestBody(request);
      writeSse(response, anthropicTextStream(this.reply));
      return;
    }
    response.writeHead(404, { "content-type": "text/plain" });
    response.end("not found");
  }
}

async function readHandshake(
  child: ChildProcessWithoutNullStreams,
  stderr: () => string
): Promise<DaemonHandshake> {
  const lines = createInterface({ input: child.stdout });
  const linePromise = new Promise<string>((resolve, reject) => {
    lines.once("line", resolve);
    child.once("exit", (code, signal) => {
      reject(new Error(`daemon exited before handshake code=${code} signal=${signal}\n${stderr()}`));
    });
  });
  const timeout = new Promise<never>((_, reject) => {
    setTimeout(() => reject(new Error(`daemon handshake timed out\n${stderr()}`)), 10_000);
  });
  const line = await Promise.race([linePromise, timeout]);
  lines.close();
  return JSON.parse(line) as DaemonHandshake;
}

async function daemonRpc<T>(
  handshake: DaemonHandshake,
  method: string,
  params: Record<string, unknown>
): Promise<T> {
  const url = new URL(handshake.url);
  url.searchParams.set("token", handshake.token);
  const socket = new WebSocket(url);
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener("open", () => resolve(), { once: true });
    socket.addEventListener("error", () => reject(new Error(`daemon websocket failed for ${method}`)), {
      once: true
    });
  });
  try {
    const id = "setup-1";
    const result = new Promise<T>((resolve, reject) => {
      const timeout = setTimeout(() => {
        reject(new Error(`${method} timed out`));
      }, 10_000);
      socket.addEventListener("message", (event) => {
        const message = JSON.parse(String(event.data));
        if (message.id !== id) return;
        clearTimeout(timeout);
        if (message.error) {
          reject(new Error(`${method} failed: ${JSON.stringify(message.error)}`));
        } else {
          resolve(message.result as T);
        }
      });
    });
    socket.send(JSON.stringify({ id, method, params }));
    return await result;
  } finally {
    socket.close();
  }
}

async function readRequestBody(request: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}

function writeJson(response: ServerResponse, value: unknown): void {
  const body = JSON.stringify(value);
  response.writeHead(200, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(body)
  });
  response.end(body);
}

function writeSse(response: ServerResponse, events: string): void {
  response.writeHead(200, {
    "content-type": "text/event-stream",
    "cache-control": "no-cache",
    connection: "keep-alive"
  });
  response.end(events);
}

function anthropicTextStream(reply: string): string {
  return [
    sseEvent("message_start", {
      type: "message_start",
      message: {
        id: "msg_desktop_ui_smoke",
        type: "message",
        role: "assistant",
        model: "claude-sonnet-4-5",
        content: [],
        usage: {
          input_tokens: 10,
          cache_read_input_tokens: 0,
          cache_creation_input_tokens: 0,
          output_tokens: 1
        }
      }
    }),
    sseEvent("content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "" }
    }),
    sseEvent("content_block_delta", {
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text: reply }
    }),
    sseEvent("content_block_stop", { type: "content_block_stop", index: 0 }),
    sseEvent("message_delta", {
      type: "message_delta",
      delta: { stop_reason: "end_turn" },
      usage: { output_tokens: 4 }
    }),
    sseEvent("message_stop", { type: "message_stop" })
  ].join("");
}

function sseEvent(event: string, data: unknown): string {
  return `event:${event}\ndata:${JSON.stringify(data)}\n\n`;
}

function discoveryCacheJson(): string {
  const now = 1_700_000_000_000;
  return JSON.stringify({
    entries: {
      "llama-cpp": { models: [], cached_at_ms: now },
      lmstudio: { models: [], cached_at_ms: now },
      ollama: { models: [], cached_at_ms: now },
      vllm: { models: [], cached_at_ms: now }
    }
  });
}

function anthropicProviderYaml(baseUrl: string): string {
  return `id: anthropic
display_name: Anthropic
base_url: "${baseUrl}"
default_api: anthropic-messages
auth_modes:
  - api_key
  - oauth
discovery:
  path: /v1/models
  response: anthropic_models
  api: anthropic-messages
  context_window: 200000
  max_output_tokens: 8192
  supports_reasoning: true
models:
  - id: claude-sonnet-4-5
    display_name: Claude Sonnet 4.5
    provider: anthropic
    api: anthropic-messages
    context_window: 200000
    max_output_tokens: 8192
    supports_reasoning: true
`;
}
