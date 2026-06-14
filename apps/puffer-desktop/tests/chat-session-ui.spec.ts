import { expect, type Locator, type Page, test } from "@playwright/test";
import {
  defaultFakeMediaCapabilities,
  FakeDaemon,
  ONE_PIXEL_JPEG_BASE64,
  type FakeMediaCapability
} from "./support/fakeDaemon";
import type { MediaSettings, MessageAttachment } from "../src/lib/types";

const baseTime = Date.now();
const onePixelPngBytes = Array.from(
  Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==",
    "base64"
  )
);
const onePixelJpegBytes = Array.from(Buffer.from(ONE_PIXEL_JPEG_BASE64, "base64"));

const configuredImageMedia: MediaSettings = {
  image: {
    providerId: "openai",
    logicalModelId: "gpt-image-1",
    selections: {
      size: "1024x1024",
      quality: "auto",
      output_format: "png"
    }
  },
  video: null
};

type FakeMediaCapabilityAxis = FakeMediaCapability["axes"][number];

function mediaEnumAxis(input: {
  id: string;
  label: string;
  values: string[];
  defaultValue: string;
  requestField?: string | null;
  wireType?: "string" | "number";
}): FakeMediaCapabilityAxis {
  return {
    id: input.id,
    label: input.label,
    role: "param",
    control: { enum: { values: input.values, default: input.defaultValue } },
    requestField: input.requestField === undefined ? input.id : input.requestField,
    wireType: input.wireType ?? "string"
  };
}

function mediaRangeAxis(input: {
  id: string;
  label: string;
  min: number;
  max: number;
  step: number;
  defaultValue: number;
  requestField?: string | null;
}): FakeMediaCapabilityAxis {
  return {
    id: input.id,
    label: input.label,
    role: "param",
    control: {
      range: {
        min: input.min,
        max: input.max,
        step: input.step,
        default: input.defaultValue
      }
    },
    requestField: input.requestField === undefined ? input.id : input.requestField,
    wireType: "number"
  };
}

function imageCapability(input: {
  providerId: string;
  providerDisplayName: string;
  modelId: string;
  modelDisplayName: string;
  axes: FakeMediaCapabilityAxis[];
}): FakeMediaCapability {
  return {
    providerId: input.providerId,
    providerDisplayName: input.providerDisplayName,
    modelId: input.modelId,
    modelDisplayName: input.modelDisplayName,
    kind: "image",
    operation: "generate",
    adapter: "images_json",
    axes: input.axes,
    status: "available",
    source: "fake-daemon",
    reason: null,
    checkedAtMs: baseTime
  };
}

function videoCapability(input: {
  providerId: string;
  providerDisplayName: string;
  modelId: string;
  modelDisplayName: string;
  adapter: string;
  axes: FakeMediaCapabilityAxis[];
}): FakeMediaCapability {
  return {
    providerId: input.providerId,
    providerDisplayName: input.providerDisplayName,
    modelId: input.modelId,
    modelDisplayName: input.modelDisplayName,
    kind: "video",
    operation: "generate",
    adapter: input.adapter,
    axes: input.axes,
    status: "available",
    source: "fake-daemon",
    reason: null,
    checkedAtMs: baseTime
  };
}

function singleOptionImageCapability(): FakeMediaCapability {
  return imageCapability({
    providerId: "openai",
    providerDisplayName: "OpenAI",
    modelId: "gpt-image-1",
    modelDisplayName: "GPT Image 1",
    axes: [
      mediaEnumAxis({
        id: "size",
        label: "Size",
        values: ["1024x1024"],
        defaultValue: "1024x1024"
      }),
      mediaEnumAxis({
        id: "quality",
        label: "Quality",
        values: ["auto"],
        defaultValue: "auto"
      })
    ]
  });
}

function singleOptionVideoCapability(): FakeMediaCapability {
  const axes = [
    mediaEnumAxis({
      id: "aspect_ratio",
      label: "Aspect ratio",
      values: ["16:9"],
      defaultValue: "16:9",
      requestField: "metadata.ratio"
    }),
    mediaEnumAxis({
      id: "duration_seconds",
      label: "Duration",
      values: ["8"],
      defaultValue: "8",
      requestField: "seconds"
    })
  ];
  return videoCapability({
    providerId: "runway",
    providerDisplayName: "Runway",
    modelId: "gen-4",
    modelDisplayName: "Gen-4",
    adapter: "replicate_video",
    axes
  });
}

function configurableVideoCapability(): FakeMediaCapability {
  return videoCapability({
    providerId: "runway",
    providerDisplayName: "Runway",
    modelId: "gen-4",
    modelDisplayName: "Gen-4",
    adapter: "replicate_video",
    axes: [
      mediaEnumAxis({
        id: "aspect_ratio",
        label: "Aspect ratio",
        values: ["16:9", "9:16"],
        defaultValue: "16:9",
        requestField: "metadata.ratio"
      }),
      mediaRangeAxis({
        id: "duration_seconds",
        label: "Duration",
        min: 5,
        max: 12,
        step: 1,
        defaultValue: 8,
        requestField: "seconds"
      })
    ]
  });
}

function configurableVideoCapabilityWithProviderOptions(): FakeMediaCapability {
  const axes = [
    mediaEnumAxis({
      id: "aspect_ratio",
      label: "Aspect ratio",
      values: ["16:9", "9:16", "1:1"],
      defaultValue: "16:9",
      requestField: "ratio"
    }),
    mediaRangeAxis({
      id: "duration_seconds",
      label: "Duration",
      min: 5,
      max: 12,
      step: 1,
      defaultValue: 5,
      requestField: "duration"
    }),
    mediaEnumAxis({
      id: "resolution",
      label: "Resolution",
      values: ["480p", "720p", "1080p"],
      defaultValue: "720p",
      requestField: "resolution"
    })
  ];
  return videoCapability({
    providerId: "byteplus",
    providerDisplayName: "BytePlus",
    modelId: "dreamina-seedance-2-0-260128",
    modelDisplayName: "Dreamina Seedance 2.0",
    adapter: "byteplus_video",
    axes
  });
}

function relaydanceVideoCapability(input: {
  modelId: string;
  modelDisplayName: string;
  resolution: string;
}): FakeMediaCapability {
  const axes = [
    mediaEnumAxis({
      id: "duration_seconds",
      label: "Duration",
      values: ["5", "8"],
      defaultValue: "5",
      requestField: "seconds"
    }),
    mediaEnumAxis({
      id: "resolution",
      label: "Resolution",
      values: [input.resolution],
      defaultValue: input.resolution,
      requestField: "metadata.resolution"
    }),
    mediaEnumAxis({
      id: "aspect_ratio",
      label: "Aspect ratio",
      values: ["16:9", "9:16"],
      defaultValue: "16:9",
      requestField: "metadata.ratio"
    })
  ];
  return videoCapability({
    providerId: "relaydance",
    providerDisplayName: "Relaydance",
    modelId: input.modelId,
    modelDisplayName: input.modelDisplayName,
    adapter: "relaydance_video",
    axes
  });
}

function generatedAttachment(jobId: string, artifactId: string, index: number): MessageAttachment {
  return {
    id: `generated-image:${artifactId}`,
    name: "Generated image",
    mimeType: "image/png",
    size: 8,
    extension: "PNG",
    kind: "image",
    state: "available",
    source: { kind: "generated_media", jobId, artifactId, index }
  };
}

async function expectReadOnlyField(dialog: Locator, label: string, value: string): Promise<void> {
  const field = dialog
    .locator(".pf-media-field")
    .filter({ hasText: label })
    .filter({ hasText: value });
  await expect(field).toHaveCount(1);
  await expect(field).toBeVisible();
  await expect(field.locator("select")).toHaveCount(0);
  await expect(field.getByText(label, { exact: true })).toBeVisible();
  await expect(field.getByText(value, { exact: true })).toBeVisible();
}

async function openSession(page: Page, name: RegExp): Promise<void> {
  await page.getByRole("button", { name }).first().click();
}

async function expectOverlayActionBeforeClose(
  actionButton: Locator,
  closeButton: Locator
): Promise<void> {
  const closeHandle = await closeButton.elementHandle();
  if (!closeHandle) throw new Error("close button handle was unavailable");
  try {
    expect(
      await actionButton.evaluate(
        (action, close) =>
          Boolean(action.compareDocumentPosition(close) & Node.DOCUMENT_POSITION_FOLLOWING),
        closeHandle
      )
    ).toBe(true);
  } finally {
    await closeHandle.dispose();
  }
}

async function reconnectBackend(page: Page, daemon: FakeDaemon): Promise<void> {
  await daemon.dropConnections();
  const banner = page.locator(".connection-banner");
  await expect(banner).toContainText("Puffer backend disconnected.");
  daemon.allowConnections();
  await banner.getByRole("button", { name: "Reconnect backend" }).click();
  await expect(page.locator(".connection-banner")).toHaveCount(0);
}

async function dispatchFileDrag(
  page: Page,
  type: "dragenter" | "drop",
  files: Array<{ name: string; mimeType: string; buffer: Buffer }>
): Promise<void> {
  const dataTransfer = await page.evaluateHandle((uploads) => {
    const transfer = new DataTransfer();
    for (const upload of uploads) {
      transfer.items.add(
        new File([Uint8Array.from(upload.bytes)], upload.name, { type: upload.mimeType })
      );
    }
    return transfer;
  }, files.map((file) => ({
    name: file.name,
    mimeType: file.mimeType,
    bytes: Array.from(file.buffer)
  })));
  try {
    await page
      .locator('[data-testid="agent-chat-drop-surface"]')
      .dispatchEvent(type, { dataTransfer });
  } finally {
    await dataTransfer.dispose();
  }
}

async function installAttachmentStageHook(page: Page): Promise<void> {
  await page.addInitScript(() => {
    (window as unknown as {
      __PUFFER_TEST_STAGE_CHAT_ATTACHMENT__?: (
        sessionId: string,
        attachment: Record<string, unknown>
      ) => Record<string, unknown>;
    }).__PUFFER_TEST_STAGE_CHAT_ATTACHMENT__ = (_sessionId, attachment) => {
      const { file: _file, previewUrl: _previewUrl, ...rest } = attachment;
      return {
        ...rest,
        id: `staged-${String(attachment.id)}`,
        state: "available",
        source: {
          kind: "local_file",
          path: `/tmp/puffer/attachments/staged-${String(attachment.id)}`
        }
      };
    };
  });
}

async function composerTextareaMetrics(page: Page): Promise<{
  height: number;
  scrollHeight: number;
  clientHeight: number;
  overflowY: string;
}> {
  return page.locator(".pf-composer textarea").evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    const rect = textarea.getBoundingClientRect();
    return {
      height: rect.height,
      scrollHeight: textarea.scrollHeight,
      clientHeight: textarea.clientHeight,
      overflowY: getComputedStyle(textarea).overflowY
    };
  });
}

async function chatThreadMetrics(page: Page): Promise<{
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  bottomGap: number;
}> {
  return page.locator(".pf-chat-thread").evaluate((node) => {
    const thread = node as HTMLDivElement;
    return {
      scrollTop: thread.scrollTop,
      scrollHeight: thread.scrollHeight,
      clientHeight: thread.clientHeight,
      bottomGap: thread.scrollHeight - thread.scrollTop - thread.clientHeight
    };
  });
}

test("composer add content menu attaches image and file drafts", async ({ page }) => {
  const imageBuffer = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==",
    "base64"
  );
  const pdfBuffer = Buffer.from("%PDF-1.7\n", "utf8");
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-attachments",
        displayName: "Attachment composer",
        title: "Attachment composer",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "attachment-seed",
            text: "Attach files here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await installAttachmentStageHook(page);
  await daemon.open(page);

  await openSession(page, /Attachment composer/);
  await expect(page.getByText("Attach files here.")).toBeVisible();

  await page.getByRole("button", { name: "Add content" }).click();
  await expect(page.getByRole("menuitem", { name: "Add images and files" })).toBeVisible();

  await page.locator('[data-testid="composer-file-input"]').setInputFiles([
    {
      name: "sample.png",
      mimeType: "image/png",
      buffer: imageBuffer
    },
    {
      name: "notes.md",
      mimeType: "text/markdown",
      buffer: Buffer.from("# Notes\n\nReview this.", "utf8")
    }
  ]);

  await expect(page.locator('[data-testid="composer-attachment-preview-strip"]')).toBeVisible();
  await expect(page.getByAltText("sample.png")).toBeVisible();
  await expect(page.getByText("notes.md")).toBeVisible();

  await page.getByRole("button", { name: "Remove attachment notes.md" }).click();
  await expect(page.getByText("notes.md")).toHaveCount(0);

  await page.locator('[data-testid="composer-file-input"]').setInputFiles([
    {
      name: "report.pdf",
      mimeType: "application/pdf",
      buffer: pdfBuffer
    }
  ]);
  await expect(page.getByText("report.pdf")).toBeVisible();
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();

  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) =>
      candidate.params.sessionId === "session-attachments" &&
      candidate.params.message === "[Image: sample.png]\n[File: report.pdf]" &&
      Array.isArray(candidate.params.attachmentIds) &&
      candidate.params.attachmentIds.length === 2 &&
      candidate.params.attachments === undefined
  );
  const attachmentIds = request.params.attachmentIds as string[];
  await expect(page.getByAltText("sample.png")).toBeVisible();
  await expect(page.getByText("report.pdf")).toBeVisible();
  await expect(page.getByText("[Image: sample.png]")).toHaveCount(0);
  await expect(page.getByText("[File: report.pdf]")).toHaveCount(0);
  await expect(page.locator('[data-testid="composer-attachment-preview-strip"]')).toHaveCount(0);

  daemon.setSessionTimeline("session-attachments", [
    {
      kind: "assistant_message",
      id: "attachment-seed",
      text: "Attach files here.",
      createdAtMs: baseTime - 30_000
    },
    {
      kind: "user_message",
      id: "attachment-persisted-user",
      text: request.params.message,
      createdAtMs: Date.now(),
      attachments: [
        {
          id: attachmentIds[0],
          name: "sample.png",
          mimeType: "image/png",
          kind: "image",
          extension: "PNG",
          size: imageBuffer.length,
          state: "available",
          source: { kind: "local_file", path: "/tmp/puffer/attachments/sample.png" }
        },
        {
          id: attachmentIds[1],
          name: "report.pdf",
          mimeType: "application/pdf",
          kind: "file",
          extension: "PDF",
          size: pdfBuffer.length,
          state: "available",
          source: { kind: "local_file", path: "/tmp/puffer/attachments/report.pdf" }
        }
      ]
    },
    {
      kind: "assistant_message",
      id: "attachment-persisted-assistant",
      text: "Done.",
      createdAtMs: Date.now() + 1
    }
  ]);
  daemon.emit("session:session-attachments:event", {
    type: "turn-complete",
    turnId: "turn-session-attachments",
    assistantText: "Done."
  });

  await expect(page.getByText("Done.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Open image attachment sample.png" })).toBeVisible();
  await expect(page.getByText("report.pdf")).toBeVisible();
  await expect(page.getByText("[Image: sample.png]")).toHaveCount(0);
  await expect(page.getByText("[File: report.pdf]")).toHaveCount(0);

  daemon.seedAttachmentPreview("session-attachments", attachmentIds[0], {
    state: "available",
    mimeType: "image/png",
    bytes: Array.from(imageBuffer)
  });
  await page.reload();
  await openSession(page, /Attachment composer/);
  const persistedImageButton = page.getByRole("button", { name: "Open image attachment sample.png" });
  await expect(persistedImageButton).toBeVisible();
  await expect(persistedImageButton.getByAltText("sample.png")).toBeVisible();
  await expect(page.getByText("report.pdf")).toBeVisible();
  await expect(page.getByText("[Image: sample.png]")).toHaveCount(0);
  await persistedImageButton.click();
  const refreshedPreview = page.getByRole("dialog", { name: "sample.png" });
  await expect(refreshedPreview).toBeVisible();
  await expect(refreshedPreview.getByAltText("sample.png")).toBeVisible();
});

test("composer image generation settings modal saves media config from daemon capabilities", async ({ page }) => {
  const imageCapabilities: FakeMediaCapability[] = [
    ...defaultFakeMediaCapabilities(),
    imageCapability({
      providerId: "openai",
      providerDisplayName: "OpenAI",
      modelId: "gpt-image-2",
      modelDisplayName: "GPT Image 2",
      axes: [
        mediaEnumAxis({
          id: "size",
          label: "Size",
          values: ["1024x1024", "1024x1536", "1536x1024"],
          defaultValue: "1024x1024"
        }),
        mediaEnumAxis({
          id: "quality",
          label: "Quality",
          values: ["auto", "high"],
          defaultValue: "auto"
        }),
        mediaEnumAxis({
          id: "output_format",
          label: "Output format",
          values: ["png", "webp"],
          defaultValue: "png"
        })
      ]
    }),
    imageCapability({
      providerId: "byteplus",
      providerDisplayName: "BytePlus",
      modelId: "seedream-3",
      modelDisplayName: "Seedream 3",
      axes: [
        mediaEnumAxis({
          id: "size",
          label: "Size",
          values: ["1024x1024", "1280x720", "720x1280"],
          defaultValue: "1024x1024"
        }),
        mediaEnumAxis({
          id: "quality",
          label: "Quality",
          values: ["standard", "high"],
          defaultValue: "standard"
        }),
        mediaEnumAxis({
          id: "output_format",
          label: "Output format",
          values: ["png", "webp"],
          defaultValue: "png"
        })
      ]
    }),
    imageCapability({
      providerId: "byteplus",
      providerDisplayName: "BytePlus",
      modelId: "seedream-4",
      modelDisplayName: "Seedream 4",
      axes: [
        mediaEnumAxis({
          id: "size",
          label: "Size",
          values: ["1024x1024", "1280x720", "720x1280"],
          defaultValue: "1024x1024"
        }),
        mediaEnumAxis({
          id: "quality",
          label: "Quality",
          values: ["standard", "high"],
          defaultValue: "standard"
        }),
        mediaEnumAxis({
          id: "output_format",
          label: "Output format",
          values: ["png", "webp"],
          defaultValue: "png"
        })
      ]
    })
  ];
  const daemon = new FakeDaemon({
    mediaCapabilities: imageCapabilities,
    sessions: [
      {
        sessionId: "session-image-settings",
        displayName: "Image generation settings session",
        title: "Image generation settings session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "image-settings-seed",
            text: "Tune image settings here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({
    media: {
      image: null,
      video: null
    }
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Image generation settings session/);

  await page.getByRole("button", { name: "Add content" }).click();
  await expect(page.getByRole("menuitem", { name: "Add images and files" })).toBeVisible();
  await page.getByRole("menuitem", { name: "Image generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Image generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "image"
  );
  await expect(dialog.getByLabel("Provider")).toHaveValue("openai");
  await expect(dialog.getByLabel("Model")).toHaveValue(
    ["openai", "gpt-image-1"].join("\u0000")
  );
  await expect(dialog.getByRole("combobox", { name: "Provider" })).toBeVisible();
  await expect(dialog.getByRole("combobox", { name: "Model" })).toBeVisible();
  const imageFolder = dialog.getByLabel("Image folder");
  await expect(imageFolder).toHaveValue("/tmp/puffer/.puffer/media/images");
  await expect(imageFolder).toHaveJSProperty("readOnly", true);
  const openFolderButton = dialog.getByRole("button", { name: "Open folder" });
  await expect(openFolderButton).toBeVisible();
  await expect(openFolderButton).toHaveAttribute("data-variant", "outline");
  const imageFolderBox = await imageFolder.boundingBox();
  const openFolderButtonBox = await openFolderButton.boundingBox();
  expect(imageFolderBox).not.toBeNull();
  expect(openFolderButtonBox).not.toBeNull();
  expect(openFolderButtonBox!.height).toBe(imageFolderBox!.height);
  const sizeOptions = await dialog.getByLabel("Size").locator("option").evaluateAll((options) =>
    options.map((option) => (option as HTMLOptionElement).value)
  );
  expect(sizeOptions).toEqual(["1024x1024", "1024x1536", "1536x1024"]);

  await dialog.getByLabel("Provider").selectOption("byteplus");
  await dialog
    .getByLabel("Model")
    .selectOption(["byteplus", "seedream-3"].join("\u0000"));
  await dialog.getByLabel("Size").selectOption("1280x720");
  await dialog.getByRole("button", { name: "Save" }).click();

  const update = await daemon.waitForRequest("update_config", (request) => "media" in request.params);
  expect(update.params).toEqual({
    media: {
      image: {
        providerId: "byteplus",
        logicalModelId: "seedream-3",
        selections: {
          size: "1280x720",
          quality: "standard",
          output_format: "png"
        }
      },
      video: null
    }
  });
  await expect(dialog).toHaveCount(0);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Image generation settings" }).click();

  const reopenedDialog = page.getByRole("dialog", { name: "Image generation settings" });
  await expect(reopenedDialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "image"
  );
  await expect(reopenedDialog.getByLabel("Provider")).toHaveValue("byteplus");
  await expect(reopenedDialog.getByLabel("Model")).toHaveValue(
    ["byteplus", "seedream-3"].join("\u0000")
  );
  await expect(reopenedDialog.getByLabel("Size")).toHaveValue("1280x720");
  await expect(reopenedDialog.getByLabel("Quality")).toHaveValue("standard");
  await expect(reopenedDialog.getByLabel("Output format")).toHaveValue("png");
  expect(daemon.requests.filter((request) => request.method === "update_config")).toHaveLength(1);
});

test("composer image generation settings shows capability loading status", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-settings-loading",
        displayName: "Image settings loading",
        title: "Image settings loading",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "image-settings-loading-seed",
            text: "Open image settings while capabilities load.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "list_media_capabilities",
    (request) => request.params.kind === "image",
    500
  );
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Image settings loading/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Image generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Image generation settings" });
  await expect(dialog).toBeVisible();
  await page.waitForTimeout(100);
  await expect(dialog.getByRole("status")).toContainText("Loading image capabilities...");
  await expect(dialog.getByText("Checking available image generation models.")).toBeVisible();
  await expect(dialog.locator(".pf-media-loading-spinner")).toBeVisible();
  await expect(dialog.locator(".pf-media-loading")).toHaveCSS("border-top-width", "0px");

  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "image"
  );
  await expect(dialog.getByRole("status")).toHaveCount(0);
  await expect(dialog.getByText("Image folder", { exact: true })).toBeVisible();
});

test("composer image generation settings renders single choices as read-only values", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [singleOptionImageCapability()],
    sessions: [
      {
        sessionId: "session-image-settings-single-option",
        displayName: "Single-option image settings",
        title: "Single-option image settings",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "image-settings-single-option-seed",
            text: "Open single-option image settings.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Single-option image settings/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Image generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Image generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "image"
  );
  await expect(dialog.getByRole("combobox")).toHaveCount(0);
  await expectReadOnlyField(dialog, "Provider", "OpenAI");
  await expectReadOnlyField(dialog, "Model", "GPT Image 1");
  await expectReadOnlyField(dialog, "Size", "1024x1024");
  await expectReadOnlyField(dialog, "Quality", "auto");
});

test("composer image generation settings clamps unsupported saved selections", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-settings-unsupported-params",
        displayName: "Unsupported image selections",
        title: "Unsupported image selections",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "image-settings-unsupported-params-seed",
            text: "Tune unsupported image settings here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({
    media: {
      image: {
        providerId: "openai",
        logicalModelId: "gpt-image-1",
        selections: {
          size: "2048x2048",
          quality: "ultra",
          output_format: "gif"
        }
      },
      video: null
    }
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Unsupported image selections/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Image generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Image generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "image"
  );
  await dialog.getByRole("button", { name: "Save" }).click();

  const update = await daemon.waitForRequest(
    "update_config",
    (request) => "media" in request.params
  );
  expect(update.params).toMatchObject({
    media: {
      image: {
        providerId: "openai",
        logicalModelId: "gpt-image-1",
        selections: {
          size: "1024x1024",
          quality: "auto",
          output_format: "png"
        }
      }
    }
  });
});

test("composer video generation settings modal remains reachable without capabilities", async ({ page }) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [],
    sessions: [
      {
        sessionId: "session-video-settings",
        displayName: "Video generation settings session",
        title: "Video generation settings session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-seed",
            text: "Tune video settings here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Video generation settings session/);

  await page.getByRole("button", { name: "Add content" }).click();
  await expect(page.getByRole("menuitem", { name: "Video generation settings" })).toBeVisible();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  const emptyState = dialog.getByText("No video capabilities available.");
  await expect(emptyState).toBeVisible();
  await expect(emptyState).not.toHaveClass(/pf-media-state/);
  await expect(emptyState).toHaveCSS("border-top-width", "0px");
  await expect(dialog.getByRole("button", { name: "Save" })).toBeDisabled();

  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
});

test("composer video generation settings shows capability loading status", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [configurableVideoCapability()],
    sessions: [
      {
        sessionId: "session-video-settings-loading",
        displayName: "Video settings loading",
        title: "Video settings loading",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-loading-seed",
            text: "Open video settings while capabilities load.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "list_media_capabilities",
    (request) => request.params.kind === "video",
    500
  );
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Video settings loading/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await page.waitForTimeout(100);
  await expect(dialog.getByRole("status")).toContainText("Loading video capabilities...");
  await expect(dialog.getByText("Checking available video generation models.")).toBeVisible();
  await expect(dialog.locator(".pf-media-loading-spinner")).toBeVisible();
  await expect(dialog.locator(".pf-media-loading")).toHaveCSS("border-top-width", "0px");

  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  await expect(dialog.getByRole("status")).toHaveCount(0);
  await expect(dialog.getByLabel("Aspect ratio")).toHaveValue("16:9");
});

test("composer video generation settings renders saved single choices as read-only values", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [singleOptionVideoCapability()],
    sessions: [
      {
        sessionId: "session-video-settings-single-option",
        displayName: "Single-option video settings",
        title: "Single-option video settings",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-single-option-seed",
            text: "Open single-option video settings.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({
    media: {
      image: null,
      video: {
        providerId: "runway",
        logicalModelId: "gen-4",
        selections: {
          aspect_ratio: "16:9",
          duration_seconds: "8"
        }
      }
    }
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Single-option video settings/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  await expect(dialog.getByText("Saved model is no longer available.")).toHaveCount(0);
  await expect(dialog.getByRole("combobox")).toHaveCount(0);
  await expectReadOnlyField(dialog, "Provider", "Runway");
  await expectReadOnlyField(dialog, "Model", "Gen-4");
  await expectReadOnlyField(dialog, "Aspect ratio", "16:9");
  await expectReadOnlyField(dialog, "Duration", "8");

  await dialog.getByRole("button", { name: "Save" }).click();
  const update = await daemon.waitForRequest(
    "update_config",
    (request) => "media" in request.params
  );
  expect(update.params).toEqual({
    media: {
      image: null,
      video: {
        providerId: "runway",
        logicalModelId: "gen-4",
        selections: {
          aspect_ratio: "16:9",
          duration_seconds: "8"
        }
      }
    }
  });
});

test("composer video generation settings shows multiple models for one provider", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [
      relaydanceVideoCapability({
        modelId: "doubao-seedance-2-0-720p",
        modelDisplayName: "Seedance 2.0 720p",
        resolution: "720p"
      }),
      relaydanceVideoCapability({
        modelId: "doubao-seedance-2-0-1080p",
        modelDisplayName: "Seedance 2.0 1080p",
        resolution: "1080p"
      })
    ],
    sessions: [
      {
        sessionId: "session-video-settings-multi-model",
        displayName: "Multi-model video settings",
        title: "Multi-model video settings",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-multi-model-seed",
            text: "Open video settings with multiple models.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Multi-model video settings/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  await expectReadOnlyField(dialog, "Provider", "Relaydance");
  await expect(dialog.getByLabel("Model")).toBeVisible();
  await expect(dialog.getByRole("combobox")).toHaveCount(3);
  await expectReadOnlyField(dialog, "Resolution", "720p");

  await dialog.getByLabel("Model").selectOption({ label: "Seedance 2.0 1080p" });
  await expectReadOnlyField(dialog, "Resolution", "1080p");

  await dialog.getByRole("button", { name: "Save" }).click();
  const update = await daemon.waitForRequest(
    "update_config",
    (request) => "media" in request.params
  );
  expect(update.params).toEqual({
    media: {
      image: null,
      video: {
        providerId: "relaydance",
        logicalModelId: "doubao-seedance-2-0-1080p",
        selections: {
          duration_seconds: "5",
          resolution: "1080p",
          aspect_ratio: "16:9"
        }
      }
    }
  });
});

test("composer video generation settings saves configurable video selections", async ({ page }) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [configurableVideoCapability()],
    sessions: [
      {
        sessionId: "session-video-settings-configurable",
        displayName: "Configurable video settings",
        title: "Configurable video settings",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-configurable-seed",
            text: "Tune configurable video settings.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Configurable video settings/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  await expect(dialog.getByRole("combobox")).toHaveCount(1);
  const videoFolder = dialog.getByLabel("Video folder");
  await expect(videoFolder).toHaveValue("/tmp/puffer/.puffer/media/videos");
  await expect(videoFolder).toHaveJSProperty("readOnly", true);
  const openFolderButton = dialog.getByRole("button", { name: "Open folder" });
  await expect(openFolderButton).toBeVisible();
  await expect(openFolderButton).toHaveAttribute("data-variant", "outline");
  const videoFolderBox = await videoFolder.boundingBox();
  const openFolderButtonBox = await openFolderButton.boundingBox();
  expect(videoFolderBox).not.toBeNull();
  expect(openFolderButtonBox).not.toBeNull();
  expect(openFolderButtonBox!.height).toBe(videoFolderBox!.height);
  await dialog.getByLabel("Aspect ratio").selectOption("9:16");
  await dialog.getByLabel("Duration").fill("12");

  await dialog.getByRole("button", { name: "Save" }).click();
  const update = await daemon.waitForRequest(
    "update_config",
    (request) => "media" in request.params
  );
  expect(update.params).toEqual({
    media: {
      image: null,
      video: {
        providerId: "runway",
        logicalModelId: "gen-4",
        selections: {
          aspect_ratio: "9:16",
          duration_seconds: "12"
        }
      }
    }
  });
});

test("composer video generation settings saves additional provider options", async ({ page }) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [configurableVideoCapabilityWithProviderOptions()],
    sessions: [
      {
        sessionId: "session-video-settings-resolution",
        displayName: "Video settings resolution",
        title: "Video settings resolution",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-resolution-seed",
            text: "Open video settings with resolution.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Video settings resolution/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  await expect(dialog.getByLabel("Resolution")).toHaveValue("720p");

  await dialog.getByLabel("Aspect ratio").selectOption("1:1");
  await dialog.getByLabel("Duration").fill("12");
  await dialog.getByLabel("Resolution").selectOption("1080p");

  await dialog.getByRole("button", { name: "Save" }).click();
  const update = await daemon.waitForRequest(
    "update_config",
    (request) => "media" in request.params
  );
  expect(update.params).toEqual({
    media: {
      image: null,
      video: {
        providerId: "byteplus",
        logicalModelId: "dreamina-seedance-2-0-260128",
        selections: {
          aspect_ratio: "1:1",
          duration_seconds: "12",
          resolution: "1080p"
        }
      }
    }
  });
});

test("composer video generation settings normalizes stale provider options", async ({ page }) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [configurableVideoCapabilityWithProviderOptions()],
    sessions: [
      {
        sessionId: "session-video-settings-stale-provider-options",
        displayName: "Video settings stale provider options",
        title: "Video settings stale provider options",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-settings-stale-provider-options-seed",
            text: "Open video settings with stale provider options.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({
    media: {
      image: null,
      video: {
        providerId: "byteplus",
        logicalModelId: "dreamina-seedance-2-0-260128",
        selections: {
          aspect_ratio: "2:1",
          duration_seconds: "30",
          resolution: "4k",
          stale_video_option: "remove-me"
        }
      }
    }
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Video settings stale provider options/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Video generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Video generation settings" });
  await expect(dialog).toBeVisible();
  await daemon.waitForRequest(
    "list_media_capabilities",
    (request) => request.params.kind === "video"
  );
  await expect(dialog.getByLabel("Aspect ratio")).toHaveValue("16:9");
  await expect(dialog.getByLabel("Duration")).toHaveValue("5");
  await expect(dialog.getByLabel("Resolution")).toHaveValue("720p");

  await dialog.getByRole("button", { name: "Save" }).click();
  const update = await daemon.waitForRequest(
    "update_config",
    (request) => "media" in request.params
  );
  expect(update.params).toEqual({
    media: {
      image: null,
      video: {
        providerId: "byteplus",
        logicalModelId: "dreamina-seedance-2-0-260128",
        selections: {
          aspect_ratio: "16:9",
          duration_seconds: "5",
          resolution: "720p"
        }
      }
    }
  });
});

test("composer media generation settings marks stale saved image model invalid", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-stale-media-settings",
        displayName: "Stale media settings",
        title: "Stale media settings",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "stale-media-settings-seed",
            text: "Check stale media settings here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({
    media: {
      image: {
        providerId: "openai",
        logicalModelId: "old-image-model",
        selections: {
          size: "1024x1024",
          quality: "auto",
          output_format: "png"
        }
      },
      video: null
    }
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Stale media settings/);

  await page.getByRole("button", { name: "Add content" }).click();
  await page.getByRole("menuitem", { name: "Image generation settings" }).click();

  const dialog = page.getByRole("dialog", { name: "Image generation settings" });
  await expect(dialog.getByText("Saved model is no longer available.")).toBeVisible();
  await expect(
    dialog.locator(".pf-media-field").filter({ hasText: "Model" }).locator("select")
  ).toHaveCount(1);
  await expect(dialog.getByLabel("Model")).toHaveValue(
    ["openai", "old-image-model"].join("\u0000")
  );
  await expect(dialog.getByRole("button", { name: "Save" })).toBeDisabled();
});

test("explicit image slash trigger routes to media generation", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-trigger",
        displayName: "Image trigger session",
        title: "Image trigger session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "image-trigger-seed",
            text: "Send image slash triggers here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({ media: configuredImageMedia });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Image trigger session/);

  await page.locator(".pf-composer textarea").fill("/image draw a compact icon");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest("generate_media");
  expect(request.params).toMatchObject({
    kind: "image",
    prompt: "draw a compact icon"
  });
  expect(daemon.requests.filter((candidate) => candidate.method === "run_agent_turn")).toHaveLength(0);
});

test("persisted media Bash results render as assistant image attachments", async ({ page }) => {
  const mediaOutput = {
    jobId: "job-generated-1",
    requestedCount: 1,
    artifacts: [
      {
        artifactId: "artifact-generated-1",
        index: 0,
        path: "/tmp/puffer/.puffer/media/images/artifact-generated-1.png",
        mimeType: "image/png",
        size: onePixelPngBytes.length
      }
    ],
    status: "succeeded"
  };
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-generated-media",
        displayName: "Generated media",
        title: "Generated media",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 2,
        timeline: [
          {
            kind: "tool_call",
            id: "tool-image",
            toolId: "Bash",
            status: "ok",
            inputText: JSON.stringify({
              command: "puffer internal-tool image-generation --prompt 'draw an icon' --count 1",
              timeout: 600000
            }),
            outputText: JSON.stringify({
              stdout: JSON.stringify(mediaOutput),
              stderr: "",
              interrupted: false
            }),
            createdAtMs: baseTime - 20_000
          },
          {
            kind: "assistant_message",
            id: "assistant-image",
            text: "Done",
            createdAtMs: baseTime - 10_000,
            attachments: [
              {
                id: "generated-image:artifact-generated-1",
                name: "Generated image",
                mimeType: "image/png",
                size: onePixelPngBytes.length,
                extension: "PNG",
                kind: "image",
                state: "available",
                source: {
                  kind: "generated_media",
                  jobId: "job-generated-1",
                  artifactId: "artifact-generated-1",
                  index: 0
                }
              }
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedMediaPreview("session-generated-media", "artifact-generated-1", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Generated media/);

  const thumbnail = page.getByRole("button", { name: "Open image attachment Generated image" });
  await expect(thumbnail).toBeVisible();
  await expect(thumbnail.getByAltText("Generated image")).toBeVisible();
  await expect(page.getByText("/tmp/puffer/.puffer/media/images")).toHaveCount(0);
  await thumbnail.click();
  await expect(page.getByTestId("attachment-overlay")).toBeVisible();
});

test("generated video attachment overlay renders a playable card with folder action", async ({
  page
}) => {
  const sessionId = "session-generated-video-card";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Generated video card",
        title: "Generated video card",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "generated-video-message",
            text: "Generated a video.",
            createdAtMs: baseTime - 30_000,
            attachments: [
              {
                id: "generated-video:artifact-video-card",
                name: "Generated video",
                mimeType: "video/mp4",
                size: 9,
                extension: "MP4",
                kind: "video",
                state: "available",
                source: {
                  kind: "generated_media",
                  jobId: "job-video-card",
                  artifactId: "artifact-video-card",
                  index: 0,
                  localPath: "/tmp/puffer/.puffer/media/videos/artifact-video-card/generated.mp4"
                }
              }
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedVideoAccess(sessionId, "artifact-video-card", {
    state: "available",
    path: "/media/generated-video/fake-video-card",
    mimeType: "video/mp4",
    size: 9,
    expiresAtMs: baseTime + 60_000,
    bytes: Buffer.from("mp4-bytes")
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-video-card", {
    state: "available",
    mimeType: "image/jpeg",
    bytes: onePixelJpegBytes
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Generated video card/);

  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === sessionId &&
      request.params.artifactId === "artifact-video-card"
  );
  expect(daemon.requests.filter((request) => request.method === "create_generated_video_access"))
    .toHaveLength(0);
  const card = page.getByRole("button", { name: "Open video attachment Generated video" });
  await expect(card).toBeVisible();
  await expect(card.getByRole("img", { name: "Generated video" })).toBeVisible();
  await expect(card.locator("video")).toHaveCount(0);
  await expect(card.locator('[data-testid="video-play-indicator"]')).toBeVisible();
  await expect(page.locator(".pf-msg").filter({ has: card })).not.toContainText("/tmp/puffer");

  await card.click();
  await daemon.waitForRequest(
    "create_generated_video_access",
    (request) =>
      request.params.sessionId === sessionId &&
      request.params.artifactId === "artifact-video-card"
  );
  const dialog = page.getByRole("dialog", { name: "Generated video" });
  await expect(dialog).toBeVisible();
  const actions = dialog.locator(".pf-attachment-dialog-actions");
  const folderButton = actions.getByRole("button", { name: "Open containing folder" });
  const closeButton = actions.getByRole("button", { name: "Close attachment preview" });
  await expect(folderButton).toBeVisible();
  await expectOverlayActionBeforeClose(folderButton, closeButton);
  const video = dialog.locator("video");
  await expect(video).toBeVisible();
  await expect(video).toHaveAttribute("controls", "");
  await expect(video).toHaveAttribute("autoplay", "");
});

test("missing generated video access falls back without exposing local paths", async ({ page }) => {
  const sessionId = "session-generated-video-missing-access";
  const localPath = "/tmp/puffer/.puffer/media/videos/artifact-video-missing/generated.mp4";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Missing generated video access",
        title: "Missing generated video access",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "generated-video-missing-message",
            text: "Generated a video that is unavailable.",
            createdAtMs: baseTime - 30_000,
            attachments: [
              {
                id: "generated-video:artifact-video-missing",
                name: "Missing video",
                mimeType: "video/mp4",
                size: 9,
                extension: "MP4",
                kind: "video",
                state: "available",
                source: {
                  kind: "generated_media",
                  jobId: "job-video-missing",
                  artifactId: "artifact-video-missing",
                  index: 0,
                  localPath
                }
              }
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedVideoAccess(sessionId, "artifact-video-missing", { state: "missing" });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Missing generated video access/);

  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === sessionId &&
      request.params.artifactId === "artifact-video-missing"
  );
  expect(daemon.requests.filter((request) => request.method === "create_generated_video_access"))
    .toHaveLength(0);
  const card = page.getByRole("button", { name: "Open video attachment Missing video" });
  await expect(card).toBeVisible();
  await expect(card.getByRole("img")).toHaveCount(0);
  await expect(card.locator("video")).toHaveCount(0);
  await expect(card.locator('[data-testid="video-play-indicator"]')).toHaveCount(0);
  const messageRow = page.locator(".pf-msg").filter({ has: card });
  await expect(messageRow).not.toContainText(localPath);

  await card.click();
  await daemon.waitForRequest(
    "create_generated_video_access",
    (request) =>
      request.params.sessionId === sessionId &&
      request.params.artifactId === "artifact-video-missing"
  );
  const dialog = page.getByRole("dialog", { name: "Missing video" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Preview unavailable for this attachment.")).toBeVisible();
});

test("shows two generated image attachments from one image generation result", async ({ page }) => {
  const sessionId = "session-generated-two";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Generated images",
        title: "Generated images",
        cwd: "/workspace/generated",
        folderPath: "/workspace/generated",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "assistant-generated-two",
            text: "",
            summary: "Generated 2 images",
            createdAtMs: baseTime - 10_000,
            attachments: [
              generatedAttachment("job-1", "artifact-1", 0),
              generatedAttachment("job-1", "artifact-2", 1)
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-1", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-2", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });

  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Generated images/);

  await expect(page.getByRole("img", { name: /Generated image/i })).toHaveCount(2);
});

test("assistant generated image paths render as links before generated thumbnails", async ({ page }) => {
  const sessionId = "session-generated-links";
  const firstPath = "/tmp/puffer/.puffer/media/images/generated-local-link-1.png";
  const secondPath = "/tmp/puffer/.puffer/media/images/generated-local-link-2.png";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Generated image links",
        title: "Generated image links",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "assistant-generated-links",
            text: `Generated images:\n${firstPath}\n${secondPath}`,
            createdAtMs: baseTime - 10_000,
            attachments: [
              generatedAttachment("job-links", "artifact-link-1", 0),
              generatedAttachment("job-links", "artifact-link-2", 1)
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-link-1", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-link-2", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  daemon.seedFile(firstPath, "first generated image bytes\n");
  daemon.seedFile(secondPath, "second generated image bytes\n");

  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Generated image links/);

  const firstPathLink = page.getByRole("link", { name: firstPath });
  const secondPathLink = page.getByRole("link", { name: secondPath });
  await expect(firstPathLink).toBeVisible();
  await expect(secondPathLink).toBeVisible();

  const firstThumbnail = page.getByRole("button", { name: "Open image attachment Generated image" }).first();
  await expect(firstThumbnail).toBeVisible();
  const thumbnailHandle = await firstThumbnail.elementHandle();
  expect(thumbnailHandle).not.toBeNull();
  try {
    expect(
      await firstPathLink.evaluate((link, thumbnail) => {
        return Boolean(link.compareDocumentPosition(thumbnail) & Node.DOCUMENT_POSITION_FOLLOWING);
      }, thumbnailHandle)
    ).toBe(true);
  } finally {
    await thumbnailHandle?.dispose();
  }

  await firstPathLink.click();
  await daemon.waitForRequest("read_file", (request) => request.params.path === firstPath);

  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Chat", exact: true }).click();
  await firstThumbnail.click();
  await expect(page.getByTestId("attachment-overlay")).toBeVisible();
});

test("inline-code image path opens Files preview while generated image attachment still opens overlay", async ({ page }) => {
  const sessionId = "session-inline-code-image-path";
  const imagePath = "/tmp/puffer/.puffer/media/images/artifact-1/image.jpeg";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Inline-code image path",
        title: "Inline-code image path",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "assistant-inline-code-image-path",
            text: `Image: \`${imagePath}\``,
            createdAtMs: baseTime - 10_000,
            attachments: [
              generatedAttachment("job-inline-code", "artifact-inline-code-thumb", 0)
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-inline-code-thumb", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  daemon.seedBinaryFile(imagePath, ONE_PIXEL_JPEG_BASE64);

  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Inline-code image path/);

  const inlinePathLink = page.getByRole("link", { name: imagePath });
  await expect(inlinePathLink).toBeVisible();
  await inlinePathLink.click();
  const readRequest = await daemon.waitForRequest(
    "read_file",
    (request) => request.params.path === imagePath
  );
  expect(readRequest.params.maxBytes).toBe(24 * 1024 * 1024);
  await expect(page.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".viewer").getByRole("img", { name: "image.jpeg" })).toBeVisible();

  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Chat", exact: true }).click();
  await page.getByRole("button", { name: "Open image attachment Generated image" }).click();
  await expect(page.getByTestId("attachment-overlay")).toBeVisible();
});

test("image slash success renders generated thumbnail without media metadata", async ({ page }) => {
  const generatedPath = "/tmp/puffer/.puffer/media/images/generated-icon.png";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-preview",
        displayName: "Image preview session",
        title: "Image preview session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "image-preview-seed",
            text: "Generate image previews here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({ media: configuredImageMedia });
  daemon.setGeneratedMediaResult({
    jobId: "media-job-preview-success",
    requestedCount: 1,
    artifacts: [
      {
        artifactId: "artifact-preview-success",
        index: 0,
        path: generatedPath,
        mimeType: "image/png",
        size: onePixelPngBytes.length
      }
    ],
    status: "succeeded"
  });
  daemon.seedGeneratedMediaPreview("session-image-preview", "artifact-preview-success", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Image preview session/);

  await page.locator(".pf-composer textarea").fill("/image draw a compact icon");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === "session-image-preview" &&
      request.params.artifactId === "artifact-preview-success"
  );
  const thumbnail = page.getByRole("button", { name: "Open image attachment Generated image" });
  await expect(thumbnail).toBeVisible();
  await expect(thumbnail.getByAltText("Generated image")).toBeVisible();

  const generatedRow = page.locator(".pf-msg").filter({ has: thumbnail });
  await expect(generatedRow).not.toContainText(generatedPath);
  await expect(generatedRow).not.toContainText("generated-icon.png");
  await expect(generatedRow).not.toContainText("media-job-preview-success");
  await expect(generatedRow).not.toContainText("artifact-preview-success");
  await expect(generatedRow).not.toContainText("openai");
  await expect(generatedRow).not.toContainText("gpt-image-1");

  await thumbnail.click();
  const previewDialog = page.getByRole("dialog", { name: "Generated image" });
  await expect(previewDialog).toBeVisible();
  await expect(previewDialog.getByAltText("Generated image")).toBeVisible();
  await expect(previewDialog).toContainText("PNG");
});

test("missing generated image preview shows unavailable thumbnail without path fallback", async ({ page }) => {
  const generatedPath = "/tmp/puffer/.puffer/media/images/missing-generated.png";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-preview-missing",
        displayName: "Missing image preview",
        title: "Missing image preview",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "missing-image-preview-seed",
            text: "Generate missing previews here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({ media: configuredImageMedia });
  daemon.setGeneratedMediaResult({
    jobId: "media-job-preview-missing",
    requestedCount: 1,
    artifacts: [
      {
        artifactId: "artifact-preview-missing",
        index: 0,
        path: generatedPath,
        mimeType: "image/png",
        size: onePixelPngBytes.length
      }
    ],
    status: "succeeded"
  });
  daemon.seedGeneratedMediaPreview("session-image-preview-missing", "artifact-preview-missing", {
    state: "missing"
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Missing image preview/);

  await page.locator(".pf-composer textarea").fill("/image draw a compact icon");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === "session-image-preview-missing" &&
      request.params.artifactId === "artifact-preview-missing"
  );
  const thumbnail = page.getByRole("button", { name: "Open image attachment Generated image" });
  await expect(thumbnail).toBeVisible();
  const generatedRow = page.locator(".pf-msg").filter({ has: thumbnail });
  await expect(generatedRow.locator('.pf-attachment-thumb[data-state="missing"]')).toBeVisible();
  await expect(generatedRow.locator(".pf-attachment-file-card")).toHaveCount(0);
  await expect(generatedRow).not.toContainText(generatedPath);
  await expect(generatedRow).not.toContainText("missing-generated.png");
  await expect(generatedRow).not.toContainText("media-job-preview-missing");
});

test("image slash success without preview shows unavailable thumbnail", async ({ page }) => {
  const generatedPath = "/tmp/puffer/.puffer/media/images/no-preview-generated.png";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-preview-no-path",
        displayName: "No path image preview",
        title: "No path image preview",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "no-path-image-preview-seed",
            text: "Generate no-path previews here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({ media: configuredImageMedia });
  daemon.setGeneratedMediaResult({
    jobId: "media-job-preview-no-path",
    requestedCount: 1,
    artifacts: [
      {
        artifactId: "artifact-preview-no-path",
        index: 0,
        path: generatedPath,
        mimeType: "image/png",
        size: onePixelPngBytes.length
      }
    ],
    status: "succeeded"
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /No path image preview/);

  await page.locator(".pf-composer textarea").fill("/image draw a compact icon");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === "session-image-preview-no-path" &&
      request.params.artifactId === "artifact-preview-no-path"
  );
  const thumbnail = page.getByRole("button", { name: "Open image attachment Generated image" });
  await expect(thumbnail).toBeVisible();
  const generatedRow = page.locator(".pf-msg").filter({ has: thumbnail });
  await expect(generatedRow.locator('.pf-attachment-thumb[data-state="missing"]')).toBeVisible();
  await expect(generatedRow.locator(".pf-attachment-file-card")).toHaveCount(0);
  await expect(generatedRow).not.toContainText("media-job-preview-no-path");
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "read_generated_media_preview" &&
        request.params.path !== undefined
    )
  ).toHaveLength(0);
});

test("generated image preview is not restored after session switch", async ({ page }) => {
  const generatedPath = "/tmp/puffer/.puffer/media/images/transient-generated.png";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-image-preview-transient",
        displayName: "Transient image preview",
        title: "Transient image preview",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "transient-image-preview-seed",
            text: "Generate transient previews here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-image-preview-other",
        displayName: "Other image preview session",
        title: "Other image preview session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 70_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "other-image-preview-seed",
            text: "Another session.",
            createdAtMs: baseTime - 40_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({ media: configuredImageMedia });
  daemon.setGeneratedMediaResult({
    jobId: "media-job-preview-transient",
    requestedCount: 1,
    artifacts: [
      {
        artifactId: "artifact-preview-transient",
        index: 0,
        path: generatedPath,
        mimeType: "image/png",
        size: onePixelPngBytes.length
      }
    ],
    status: "succeeded"
  });
  daemon.seedGeneratedMediaPreview("session-image-preview-transient", "artifact-preview-transient", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Transient image preview/);

  await page.locator(".pf-composer textarea").fill("/image draw a compact icon");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === "session-image-preview-transient" &&
      request.params.artifactId === "artifact-preview-transient"
  );
  const thumbnail = page.getByRole("button", { name: "Open image attachment Generated image" });
  await expect(thumbnail).toBeVisible();

  await openSession(page, /Other image preview session/);
  await expect(page.getByText("Another session.")).toBeVisible();
  await expect(thumbnail).toHaveCount(0);

  await openSession(page, /Transient image preview/);
  await expect(page.getByText("Generate transient previews here.")).toBeVisible();
  await expect(thumbnail).toHaveCount(0);
});

test("normal image text still routes to chat", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-normal-image-text",
        displayName: "Normal image text",
        title: "Normal image text",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "normal-image-text-seed",
            text: "Send normal chat here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Normal image text/);

  await page.locator(".pf-composer textarea").fill("please make an image of the plan");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest("run_agent_turn");
  expect(request.params.message).toBe("please make an image of the plan");
  expect(daemon.requests.filter((candidate) => candidate.method === "generate_media")).toHaveLength(0);
});

test("video slash success appends a generated video attachment", async ({ page }) => {
  const sessionId = "session-video-preview-success";
  const daemon = new FakeDaemon({
    mediaCapabilities: [singleOptionVideoCapability()],
    sessions: [
      {
        sessionId,
        displayName: "Video preview success",
        title: "Video preview success",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-preview-seed",
            text: "Generate videos here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.setSettingsConfig({
    media: {
      image: null,
      video: {
        providerId: "runway",
        logicalModelId: "gen-4",
        selections: { duration_seconds: "8", aspect_ratio: "16:9" }
      }
    }
  });
  daemon.setGeneratedMediaResult({
    jobId: "media-job-video-success",
    requestedCount: 1,
    kind: "video",
    artifacts: [
      {
        artifactId: "artifact-video-live",
        index: 0,
        path: "/tmp/puffer/.puffer/media/videos/artifact-video-live/generated.mp4",
        mimeType: "video/mp4",
        size: 9
      }
    ],
    status: "succeeded"
  });
  daemon.seedGeneratedVideoAccess(sessionId, "artifact-video-live", {
    state: "available",
    path: "/media/generated-video/fake-video-live",
    mimeType: "video/mp4",
    size: 9,
    expiresAtMs: baseTime + 60_000,
    bytes: Buffer.from("mp4-bytes")
  });
  daemon.seedGeneratedMediaPreview(sessionId, "artifact-video-live", {
    state: "available",
    mimeType: "image/jpeg",
    bytes: onePixelJpegBytes
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Video preview success/);

  await page.locator(".pf-composer textarea").fill("/video animate this logo");
  await page.getByRole("button", { name: "Send" }).click();

  const card = page.getByRole("button", { name: "Open video attachment Generated video" });
  await expect(card).toBeVisible();
  await daemon.waitForRequest(
    "read_generated_media_preview",
    (request) =>
      request.params.sessionId === sessionId &&
      request.params.artifactId === "artifact-video-live"
  );
  await expect(card.getByRole("img", { name: "Generated video" })).toBeVisible();
  await expect(card.locator("video")).toHaveCount(0);
  await expect(card.locator('[data-testid="video-play-indicator"]')).toBeVisible();
  expect(daemon.requests.filter((request) => request.method === "create_generated_video_access"))
    .toHaveLength(0);
  await expect(page.locator(".pf-msg").filter({ has: card })).not.toContainText("media-job-video-success");

  await card.click();
  await daemon.waitForRequest(
    "create_generated_video_access",
    (request) =>
      request.params.sessionId === sessionId &&
      request.params.artifactId === "artifact-video-live"
  );
  const dialog = page.getByRole("dialog", { name: "Generated video" });
  await expect(dialog.locator("video")).toBeVisible();
});

test("explicit video slash trigger fails clearly without capability", async ({ page }) => {
  const daemon = new FakeDaemon({
    mediaCapabilities: [],
    sessions: [
      {
        sessionId: "session-video-trigger",
        displayName: "Video trigger session",
        title: "Video trigger session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "video-trigger-seed",
            text: "Send video slash triggers here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Video trigger session/);

  await page.locator(".pf-composer textarea").fill("/video animate this logo");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest("generate_media");
  expect(request.params).toMatchObject({
    kind: "video",
    prompt: "animate this logo"
  });
  await expect(page.getByText("No video capabilities available.")).toBeVisible();
});

test("message attachments open image preview and file details", async ({ page }) => {
  const imageBuffer = Buffer.from(onePixelPngBytes);
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-attachment-open",
        displayName: "Attachment open targets",
        title: "Attachment open targets",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "attachment-open-seed",
            text: "Attach a screenshot and notes.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await installAttachmentStageHook(page);
  await daemon.open(page);

  await openSession(page, /Attachment open targets/);
  await page.getByRole("button", { name: "Add content" }).click();
  await page.locator('[data-testid="composer-file-input"]').setInputFiles([
    {
      name: "sample.png",
      mimeType: "image/png",
      buffer: imageBuffer
    },
    {
      name: "notes.md",
      mimeType: "text/markdown",
      buffer: Buffer.from("# Notes\n\nReview this.", "utf8")
    }
  ]);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-attachment-open" &&
      request.params.message === "[Image: sample.png]\n[File: notes.md]" &&
      Array.isArray(request.params.attachmentIds) &&
      request.params.attachmentIds.length === 2 &&
      request.params.attachments === undefined
  );

  const sampleAttachmentButton = page.getByRole("button", { name: "Open image attachment sample.png" });
  await sampleAttachmentButton.click();
  const previewDialog = page.getByRole("dialog", { name: "sample.png" });
  await expect(previewDialog).toBeVisible();
  await expect(previewDialog.getByAltText("sample.png")).toBeVisible();
  await expect(previewDialog).toContainText("PNG");
  await page.keyboard.press("Escape");
  await expect(page.locator('[data-testid="attachment-overlay"]')).toHaveCount(0);
  await expect(sampleAttachmentButton).toBeFocused();

  await page.getByRole("button", { name: "Open attachment details for notes.md" }).click();
  const detailsDialog = page.getByRole("dialog", { name: "notes.md" });
  await expect(detailsDialog).toBeVisible();
  await expect(detailsDialog).toContainText("Preview unavailable for this attachment.");
  await expect(detailsDialog).toContainText("text/markdown");
  await expect(page.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "false");
});

test("attachment overlay hides folder action for uploaded staged local image source", async ({ page }) => {
  const sessionId = "session-uploaded-local-image-overlay-action";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Uploaded local image overlay action",
        title: "Uploaded local image overlay action",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "uploaded-local-image-overlay-action-message",
            text: "Uploaded staged local image attachment.",
            createdAtMs: baseTime - 30_000,
            attachments: [
              {
                id: "uploaded-local-image-overlay-action",
                name: "uploaded-action.png",
                mimeType: "image/png",
                size: onePixelPngBytes.length,
                extension: "PNG",
                kind: "image",
                state: "available",
                source: {
                  kind: "local_file",
                  path: "/tmp/puffer/attachments/uploaded-action.png"
                }
              }
            ]
          }
        ]
      }
    ]
  });
  daemon.seedAttachmentPreview(sessionId, "uploaded-local-image-overlay-action", {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });

  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Uploaded local image overlay action/);

  const thumbnail = page.getByRole("button", { name: "Open image attachment uploaded-action.png" });
  await expect(thumbnail).toBeVisible();
  await thumbnail.click();

  const overlay = page.getByTestId("attachment-overlay");
  await expect(overlay).toBeVisible();
  await expect(overlay.getByAltText("uploaded-action.png")).toBeVisible();
  await expect(overlay).toContainText("PNG");
  await expect(overlay).toContainText("image/png");
  const actions = overlay.locator(".pf-attachment-dialog-actions");
  const closeButton = actions.getByRole("button", { name: "Close attachment preview" });
  await expect(closeButton).toBeVisible();
  await expect(actions.getByRole("button", { name: "Open containing folder" })).toHaveCount(0);
  await expect(actions.getByRole("button")).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(overlay).toHaveCount(0);
  await expect(thumbnail).toBeFocused();
});

test("attachment overlay shows folder action for generated media local path", async ({ page }) => {
  const sessionId = "session-generated-media-overlay-action";
  const artifactId = "artifact-generated-overlay-action";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Generated media overlay action",
        title: "Generated media overlay action",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "generated-media-overlay-action-message",
            text: "Generated media attachment.",
            createdAtMs: baseTime - 30_000,
            attachments: [
              {
                id: `generated-image:${artifactId}`,
                name: "Generated image",
                mimeType: "image/png",
                size: onePixelPngBytes.length,
                extension: "PNG",
                kind: "image",
                state: "available",
                source: {
                  kind: "generated_media",
                  jobId: "job-generated-overlay-action",
                  artifactId,
                  index: 0,
                  localPath: "/tmp/puffer/.puffer/media/images/artifact-generated-overlay-action.png"
                }
              }
            ]
          }
        ]
      }
    ]
  });
  daemon.seedGeneratedMediaPreview(sessionId, artifactId, {
    state: "available",
    mimeType: "image/png",
    bytes: onePixelPngBytes
  });

  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Generated media overlay action/);

  const thumbnail = page.getByRole("button", { name: "Open image attachment Generated image" });
  await expect(thumbnail).toBeVisible();
  await thumbnail.click();

  const overlay = page.getByTestId("attachment-overlay");
  await expect(overlay).toBeVisible();
  await expect(overlay.getByAltText("Generated image")).toBeVisible();
  const actions = overlay.locator(".pf-attachment-dialog-actions");
  const folderButton = actions.getByRole("button", { name: "Open containing folder" });
  const closeButton = actions.getByRole("button", { name: "Close attachment preview" });
  await expect(folderButton).toBeVisible();
  await expectOverlayActionBeforeClose(folderButton, closeButton);

  await page.keyboard.press("Escape");
  await expect(overlay).toHaveCount(0);
  await expect(thumbnail).toBeFocused();
});

test("attachment overlay shows download action for remote URL image source", async ({ page }) => {
  const sessionId = "session-remote-image-overlay-action";
  const remoteUrl = "https://images.example.test/remote-action.png";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Remote image overlay action",
        title: "Remote image overlay action",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "remote-image-overlay-action-message",
            text: "Remote image attachment.",
            createdAtMs: baseTime - 30_000,
            attachments: [
              {
                id: "remote-image-overlay-action",
                name: "remote-action.png",
                mimeType: "image/png",
                size: onePixelPngBytes.length,
                extension: "PNG",
                kind: "image",
                state: "available",
                source: {
                  kind: "remote_url",
                  url: remoteUrl
                }
              }
            ]
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);
  await openSession(page, /Remote image overlay action/);

  const thumbnail = page.getByRole("button", { name: "Open image attachment remote-action.png" });
  await expect(thumbnail).toBeVisible();
  await thumbnail.click();

  const overlay = page.getByTestId("attachment-overlay");
  await expect(overlay).toBeVisible();
  const actions = overlay.locator(".pf-attachment-dialog-actions");
  const downloadButton = actions.getByRole("button", { name: "Download image" });
  const closeButton = actions.getByRole("button", { name: "Close attachment preview" });
  await expect(downloadButton).toBeVisible();
  await expectOverlayActionBeforeClose(downloadButton, closeButton);
  expect(
    daemon.requests.some(
      (request) =>
        request.method === "read_chat_attachment_preview" &&
        request.params.attachmentId === "remote-image-overlay-action"
    )
  ).toBe(false);

  await page.keyboard.press("Escape");
  await expect(overlay).toHaveCount(0);
  await expect(thumbnail).toBeFocused();
});

test("restored pending attachment without preview opens unavailable detail", async ({ page }) => {
  const sessionId = "session-restored-attachment";
  await page.addInitScript(
    ({ key, expiresAtMs }) => {
      window.localStorage.setItem(
        key,
        JSON.stringify({
          expiresAtMs,
          item: {
            id: "pending-stale-image",
            kind: "user",
            createdAtMs: Date.now(),
            title: "User",
            summary: "stale.png",
            body: "[Image: stale.png]",
            meta: ["1 attachment"],
            attachments: [
              {
                id: "stale-image",
                name: "stale.png",
                mimeType: "image/png",
                size: 68,
                extension: "PNG",
                kind: "image",
                source: { kind: "local_file", path: "/tmp/puffer/attachments/stale.png" }
              }
            ]
          }
        })
      );
    },
    {
      key: `puffer-desktop:pending-submitted:${sessionId}`,
      expiresAtMs: Date.now() + 10 * 60_000
    }
  );

  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId,
        displayName: "Restored attachment",
        title: "Restored attachment",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Restored attachment/);
  await expect(page.getByText("stale.png")).toBeVisible();
  await expect(page.getByText("[Image: stale.png]")).toHaveCount(0);

  await page.getByRole("button", { name: "Open image attachment stale.png" }).click();
  const dialog = page.getByRole("dialog", { name: "stale.png" });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Preview unavailable for this attachment.");
});

test("missing persisted image attachment opens unavailable detail", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-missing-persisted-attachment",
        displayName: "Missing persisted attachment",
        title: "Missing persisted attachment",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "user_message",
            id: "missing-image-message",
            text: "[Image: lost.png]",
            createdAtMs: baseTime - 30_000,
            attachments: [
              {
                id: "missing-image",
                name: "lost.png",
                mimeType: "image/png",
                size: 68,
                extension: "PNG",
                kind: "image",
                state: "missing",
                source: { kind: "local_file", path: "/tmp/puffer/attachments/lost.png" }
              }
            ]
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Missing persisted attachment/);
  await expect(page.getByText("lost.png")).toBeVisible();
  await expect(page.getByText("[Image: lost.png]")).toHaveCount(0);

  await page.getByRole("button", { name: "Open image attachment lost.png" }).click();
  const dialog = page.getByRole("dialog", { name: "lost.png" });
  await expect(dialog).toBeVisible();
  await expect(dialog).toContainText("Preview unavailable for this attachment.");
  await expect(dialog.locator("img")).toHaveCount(0);
});

test("chat file targets route message paths and tool paths through Files", async ({ page }) => {
  const messagePath = "/tmp/puffer/src/main.rs";
  const toolPath = "/tmp/puffer/src/tool.rs";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-file-open-targets",
        displayName: "File open targets",
        title: "File open targets",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 2,
        timeline: [
          {
            kind: "assistant_message",
            id: "file-open-message",
            text: `Review ${messagePath}:2 before changing the helper.`,
            createdAtMs: baseTime - 30_000
          },
          {
            kind: "tool_call",
            id: "file-open-tool",
            toolId: "read_file",
            status: "success",
            summary: `Read ${toolPath}`,
            inputJson: { path: toolPath },
            outputText: JSON.stringify({ content: "pub fn helper() {}\n" }),
            createdAtMs: baseTime - 20_000
          }
        ]
      }
    ]
  });
  daemon.seedFile(messagePath, "fn main() {\n    let target = 42;\n}\n");
  daemon.seedFile(toolPath, "pub fn helper() {}\n");

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /File open targets/);
  await page.getByRole("link", { name: `${messagePath}:2` }).click();
  await expect(page.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".viewer")).toContainText(messagePath);
  await expect(page.locator(".viewer")).toContainText("let target = 42");

  await page.getByRole("button", { name: "Chat" }).click();
  await page.getByRole("button", { name: /Agent activity/ }).click();
  await page.getByRole("button", { name: /Read tool\.rs/ }).click();
  await page.getByRole("button", { name: toolPath, exact: true }).click();
  await expect(page.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
  await expect(page.locator(".viewer")).toContainText(toolPath);
  await expect(page.locator(".viewer")).toContainText("pub fn helper() {}");
});

test("chat surface drop attaches image and file drafts", async ({ page }) => {
  const imageBuffer = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAFgwJ/lzTnGQAAAABJRU5ErkJggg==",
    "base64"
  );
  const pdfBuffer = Buffer.from("%PDF-1.7\n", "utf8");
  const files = [
    {
      name: "drop.png",
      mimeType: "image/png",
      buffer: imageBuffer
    },
    {
      name: "drop.pdf",
      mimeType: "application/pdf",
      buffer: pdfBuffer
    }
  ];
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-drop-attachments",
        displayName: "Drop attachments",
        title: "Drop attachments",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "drop-seed",
            text: "Drop files here.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await installAttachmentStageHook(page);
  await daemon.open(page);

  await openSession(page, /Drop attachments/);
  await expect(page.getByText("Drop files here.")).toBeVisible();

  await dispatchFileDrag(page, "dragenter", files);
  await expect(page.getByText("Drop files to attach")).toBeVisible();
  await expect(page.getByText("Up to 10 files, 20 MiB each")).toBeVisible();

  await dispatchFileDrag(page, "drop", files);

  await expect(page.getByText("Drop files to attach")).toHaveCount(0);
  await expect(page.locator('[data-testid="composer-attachment-preview-strip"]')).toBeVisible();
  await expect(page.getByAltText("drop.png")).toBeVisible();
  await expect(page.getByText("drop.pdf")).toBeVisible();

  await page.locator(".pf-composer textarea").fill("review drop");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (candidate) =>
      candidate.params.sessionId === "session-drop-attachments" &&
      candidate.params.message === "review drop\n\n[Image: drop.png]\n[File: drop.pdf]" &&
      Array.isArray(candidate.params.attachmentIds) &&
      candidate.params.attachmentIds.length === 2 &&
      candidate.params.attachments === undefined
  );
  expect(request.params.attachmentIds).toHaveLength(2);
  await expect(page.locator('[data-testid="composer-attachment-preview-strip"]')).toHaveCount(0);
  await expect(page.getByAltText("drop.png")).toBeVisible();
  await expect(page.getByText("drop.pdf")).toBeVisible();
  await expect(page.getByText("[Image: drop.png]")).toHaveCount(0);
  await expect(page.getByText("[File: drop.pdf]")).toHaveCount(0);
});

test("turn completion reload does not leak live chat into a newly selected session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha",
        displayName: "Alpha session",
        title: "Alpha session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-seed",
            text: "Alpha seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta",
        displayName: "Beta session",
        title: "Beta session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-seed",
            text: "Beta seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("button", { name: /Alpha session/ }).first()).toBeVisible();
  await openSession(page, /Alpha session/);
  await expect(page.getByText("Alpha seed")).toBeVisible();

  await page.locator(".pf-composer textarea").fill("Race from alpha");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha" &&
      request.params.message === "Race from alpha"
  );

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-alpha",
    500
  );
  daemon.emit("session:session-alpha:event", {
    type: "turn-complete",
    turnId: "turn-session-alpha",
    assistantText: "Alpha completion should stay with alpha"
  });

  await openSession(page, /Beta session/);
  await expect(page.getByText("Beta seed")).toBeVisible();

  await page.waitForTimeout(650);
  await expect(page.getByText("Beta seed")).toBeVisible();
  await expect(page.getByText("Alpha completion should stay with alpha")).toHaveCount(0);
  await expect(page.getByText("Race from alpha")).toHaveCount(0);
});

test("late turn start responses do not leak into a switched session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-start",
        displayName: "Alpha start",
        title: "Alpha start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-start-seed",
            text: "Alpha start seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta-start",
        displayName: "Beta start",
        title: "Beta start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-start-seed",
            text: "Beta start seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-start",
    120
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha start/);
  await expect(page.getByText("Alpha start seed")).toBeVisible();
  await page.locator(".pf-composer textarea").fill("Alpha delayed prompt");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-start" &&
      request.params.message === "Alpha delayed prompt"
  );

  await openSession(page, /Beta start/);
  await expect(page.getByText("Beta start seed")).toBeVisible();

  await page.waitForTimeout(170);
  await expect(page.getByText("Beta start seed")).toBeVisible();
  await expect(page.getByText("Alpha delayed prompt")).toHaveCount(0);

  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Beta prompt after alpha race");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
});

test("pending submitted prompt survives switching away and back before turn id", async ({
  page
}) => {
  const prompt = "Alpha prompt survives round trip";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-pending-return",
        displayName: "Alpha pending return",
        title: "Alpha pending return",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-pending-return-seed",
            text: "Alpha pending return seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta-pending-return",
        displayName: "Beta pending return",
        title: "Beta pending return",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-pending-return-seed",
            text: "Beta pending return seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-pending-return",
    260
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha pending return/);
  await expect(page.getByText("Alpha pending return seed")).toBeVisible();
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-pending-return" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);

  await openSession(page, /Beta pending return/);
  await expect(page.getByText("Beta pending return seed")).toBeVisible();
  await expect(page.getByText(prompt)).toHaveCount(0);

  await openSession(page, /Alpha pending return/);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await page.waitForTimeout(320);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();
});

test("accepted prompt stays out of the draft when reopened after delayed turn start", async ({
  page
}) => {
  const prompt = "Alpha accepted while hidden";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-accepted-hidden",
        displayName: "Alpha accepted hidden",
        title: "Alpha accepted hidden",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-accepted-hidden-seed",
            text: "Alpha accepted hidden seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta-accepted-hidden",
        displayName: "Beta accepted hidden",
        title: "Beta accepted hidden",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-accepted-hidden-seed",
            text: "Beta accepted hidden seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-accepted-hidden",
    140
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha accepted hidden/);
  await expect(page.getByText("Alpha accepted hidden seed")).toBeVisible();
  const composer = page.locator(".pf-composer textarea");
  await composer.fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-accepted-hidden" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);

  await openSession(page, /Beta accepted hidden/);
  await expect(page.getByText("Beta accepted hidden seed")).toBeVisible();
  await page.waitForTimeout(190);
  await expect(page.getByText(prompt)).toHaveCount(0);

  await openSession(page, /Alpha accepted hidden/);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(composer).toHaveValue("");
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();
});

test("completed turn while away does not restore stale running controls", async ({
  page
}) => {
  const prompt = "Alpha finishes while hidden";
  const reply = "Alpha finished while another session was open.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-hidden-complete",
        displayName: "Alpha hidden complete",
        title: "Alpha hidden complete",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-hidden-complete",
        displayName: "Beta hidden complete",
        title: "Beta hidden complete",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-hidden-complete-seed",
            text: "Beta hidden complete seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha hidden complete/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-hidden-complete" &&
      request.params.message === prompt
  );
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();

  await openSession(page, /Beta hidden complete/);
  await expect(page.getByText("Beta hidden complete seed")).toBeVisible();
  daemon.setSessionTimeline("session-alpha-hidden-complete", [
    {
      kind: "user_message",
      id: "alpha-hidden-complete-user",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "alpha-hidden-complete-assistant",
      text: reply,
      createdAtMs: baseTime + 2
    }
  ]);

  await openSession(page, /Alpha hidden complete/);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);

  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Follow-up after hidden completion");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
});

test("pending turn start in one session does not disable another session composer", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-inflight",
        displayName: "Alpha inflight",
        title: "Alpha inflight",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-inflight",
        displayName: "Beta inflight",
        title: "Beta inflight",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-inflight",
    5_000
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha inflight/);
  await page.locator(".pf-composer textarea").fill("Alpha waits for turn id");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-inflight" &&
      request.params.message === "Alpha waits for turn id"
  );

  await openSession(page, /Beta inflight/);
  const betaComposer = page.locator(".pf-composer textarea");
  await betaComposer.fill("Beta should still send");
  const sendButton = page.getByRole("button", { name: "Send", exact: true });
  await expect(sendButton).toBeEnabled({ timeout: 500 });
  await sendButton.click();
  await page.waitForTimeout(100);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "run_agent_turn" &&
        request.params.sessionId === "session-beta-inflight" &&
        request.params.message === "Beta should still send"
    )
  ).toHaveLength(1);
});

test("sidebar keeps non-selected running agent live while another session is open", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-sidebar-live",
        displayName: "Alpha sidebar live",
        title: "Alpha sidebar live",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-sidebar-live",
        displayName: "Beta sidebar live",
        title: "Beta sidebar live",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha sidebar live/);
  await page.locator(".pf-composer textarea").fill("Keep alpha running in the sidebar");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-alpha-sidebar-live" &&
      request.params.message === "Keep alpha running in the sidebar"
  );

  const alphaRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Alpha sidebar live" });
  await expect(alphaRow.locator('.state[data-state="thinking"]')).toContainText("thinking");

  await openSession(page, /Beta sidebar live/);
  await expect(page.locator(".pf-agent-detail")).toBeVisible();
  await expect(alphaRow).toBeVisible();
  await expect(alphaRow.locator('.state[data-state="thinking"]')).toContainText("thinking");

  daemon.emit("session:session-alpha-sidebar-live:event", {
    type: "turn-complete",
    turnId: "turn-session-alpha-sidebar-live",
    assistantText: "Alpha sidebar turn complete"
  });
  await expect(alphaRow.locator('.state[data-state="idle"]')).toContainText("idle");

  await openSession(page, /Alpha sidebar live/);
  await expect(
    page.locator('.pf-msg[data-role="user"]').filter({ hasText: "Keep alpha running in the sidebar" })
  ).toHaveCount(1);
  await expect(alphaRow.locator('.state[data-state="idle"]')).toContainText("idle");
  await expect(page.locator(".pf-agent-status-pill")).toContainText("Idle");
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await page.locator(".pf-composer textarea").fill("Follow-up after sidebar completion");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
});

test("composer enter does not submit while IME composition is active", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-ime-compose",
        displayName: "IME compose",
        title: "IME compose",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /IME compose/);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();
  await composer.fill("zhong");

  await composer.evaluate((node) => {
    node.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    node.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        isComposing: true
      })
    );
    node.dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        bubbles: true,
        cancelable: true,
        keyCode: 229
      })
    );
  });

  await page.waitForTimeout(50);
  await expect(composer).toHaveValue("zhong");
  expect(daemon.requests.filter((request) => request.method === "run_agent_turn")).toHaveLength(0);
});

test("composer textarea autosizes long drafts and resets after send", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-autosize-alpha",
        displayName: "Autosize alpha",
        title: "Autosize alpha",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "autosize-alpha-seed",
            text: "Use the composer for a long prompt.",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-autosize-beta",
        displayName: "Autosize beta",
        title: "Autosize beta",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 55_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "autosize-beta-seed",
            text: "Second session for draft restoration.",
            createdAtMs: baseTime - 25_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Autosize alpha/);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();

  const initialMetrics = await composerTextareaMetrics(page);
  const multilinePrompt = [
    "Audit the current chat composer.",
    "Keep the implementation local.",
    "Preserve Enter and Shift+Enter behavior.",
    "Avoid shared abstractions.",
    "Add a focused regression test.",
    "Report any risks."
  ].join("\n");

  await composer.fill(multilinePrompt);
  await expect(composer).toHaveValue(multilinePrompt);
  await expect
    .poll(async () => (await composerTextareaMetrics(page)).height)
    .toBeGreaterThan(initialMetrics.height + 24);

  const grownMetrics = await composerTextareaMetrics(page);

  await openSession(page, /Autosize beta/);
  await expect(composer).toHaveValue("");

  await openSession(page, /Autosize alpha/);
  await expect(composer).toHaveValue(multilinePrompt);
  await expect
    .poll(async () => (await composerTextareaMetrics(page)).height)
    .toBeGreaterThanOrEqual(grownMetrics.height - 1);

  const longPrompt = Array.from(
    { length: 40 },
    (_, index) => `long composer line ${index + 1}`
  ).join("\n");

  await composer.fill(longPrompt);
  await expect(composer).toHaveValue(longPrompt);
  await expect
    .poll(async () => (await composerTextareaMetrics(page)).overflowY)
    .toBe("auto");

  const cappedMetrics = await composerTextareaMetrics(page);
  expect(cappedMetrics.height).toBeGreaterThan(160);
  expect(cappedMetrics.height).toBeLessThanOrEqual(205);
  expect(cappedMetrics.scrollHeight).toBeGreaterThan(cappedMetrics.clientHeight);
  expect(cappedMetrics.overflowY).toBe("auto");

  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-autosize-alpha" &&
      request.params.message === longPrompt
  );
  await expect(composer).toHaveValue("");
  await expect
    .poll(async () => (await composerTextareaMetrics(page)).height)
    .toBeLessThanOrEqual(initialMetrics.height + 4);
});

test("composer autosize keeps bottom thread content anchored", async ({ page }) => {
  const timeline = Array.from({ length: 36 }, (_, index) => [
    {
      kind: "user_message",
      id: `anchor-user-${index + 1}`,
      text: `Anchored composer user row ${index + 1}. `.repeat(6),
      createdAtMs: baseTime - 120_000 + index * 2_000
    },
    {
      kind: "assistant_message",
      id: `anchor-assistant-${index + 1}`,
      text: `Anchored composer assistant row ${index + 1}. `.repeat(10),
      createdAtMs: baseTime - 119_000 + index * 2_000
    }
  ]).flat();
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-autosize-anchor",
        displayName: "Autosize anchor",
        title: "Autosize anchor",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 120_000,
        eventCount: timeline.length,
        timeline
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Autosize anchor/);
  const composer = page.locator(".pf-composer textarea");
  const thread = page.locator(".pf-chat-thread");
  await expect(composer).toBeEnabled();
  await expect(page.getByText("Anchored composer assistant row 36.")).toBeVisible();
  const overflowMetrics = await chatThreadMetrics(page);
  expect(overflowMetrics.scrollHeight).toBeGreaterThan(overflowMetrics.clientHeight + 200);

  await thread.evaluate((node) => {
    node.scrollTop = node.scrollHeight;
  });
  await expect.poll(async () => (await chatThreadMetrics(page)).bottomGap).toBeLessThan(2);

  const pinnedBefore = await chatThreadMetrics(page);
  const multilinePrompt = Array.from(
    { length: 9 },
    (_, index) => `anchored composer line ${index + 1}`
  ).join("\n");

  await composer.fill(multilinePrompt);
  await expect
    .poll(async () => (await composerTextareaMetrics(page)).height)
    .toBeGreaterThan(120);
  await expect.poll(async () => (await chatThreadMetrics(page)).bottomGap).toBeLessThan(2);

  const pinnedAfter = await chatThreadMetrics(page);
  expect(pinnedAfter.scrollTop).toBeGreaterThan(pinnedBefore.scrollTop + 48);

  await thread.evaluate((node) => {
    node.scrollTop = Math.max(0, node.scrollTop - 320);
  });
  const scrolledBefore = await chatThreadMetrics(page);
  expect(scrolledBefore.bottomGap).toBeGreaterThan(260);

  await composer.fill("short prompt");
  await expect
    .poll(async () => (await composerTextareaMetrics(page)).height)
    .toBeLessThan(80);

  const scrolledAfter = await chatThreadMetrics(page);
  expect(scrolledAfter.bottomGap).toBeGreaterThan(120);
});

test("chat scroll-to-bottom button appears away from bottom and scrolls to latest message", async ({
  page
}) => {
  const timeline = Array.from({ length: 34 }, (_, index) => [
    {
      kind: "user_message",
      id: `scroll-button-user-${index + 1}`,
      text: `Scroll button user row ${index + 1}. `.repeat(6),
      createdAtMs: baseTime - 140_000 + index * 2_000
    },
    {
      kind: "assistant_message",
      id: `scroll-button-assistant-${index + 1}`,
      text: `Scroll button assistant row ${index + 1}. `.repeat(10),
      createdAtMs: baseTime - 139_000 + index * 2_000
    }
  ]).flat();
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-scroll-button",
        displayName: "Scroll button",
        title: "Scroll button",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 140_000,
        eventCount: timeline.length,
        timeline
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Scroll button/);
  const thread = page.locator(".pf-chat-thread");
  const scrollButton = page.getByRole("button", { name: "Scroll to bottom" });
  await expect(page.getByText("Scroll button assistant row 34.")).toBeVisible();

  const overflowMetrics = await chatThreadMetrics(page);
  expect(overflowMetrics.scrollHeight).toBeGreaterThan(overflowMetrics.clientHeight + 200);

  await thread.evaluate((node) => {
    const thread = node as HTMLDivElement;
    thread.scrollTop = thread.scrollHeight;
    thread.dispatchEvent(new Event("scroll"));
  });
  await expect.poll(async () => (await chatThreadMetrics(page)).bottomGap).toBeLessThan(2);
  await expect(scrollButton).toHaveCount(0);

  await thread.evaluate((node) => {
    const thread = node as HTMLDivElement;
    thread.scrollTop = Math.max(0, thread.scrollTop - 360);
    thread.dispatchEvent(new Event("scroll"));
  });
  await expect.poll(async () => (await chatThreadMetrics(page)).bottomGap).toBeGreaterThan(250);
  await expect(scrollButton).toBeVisible();

  await scrollButton.click();
  await expect(scrollButton).toHaveCount(0);
  await expect.poll(async () => (await chatThreadMetrics(page)).bottomGap).toBeLessThan(100);
});

test("chat scroll-to-bottom button stays hidden without meaningful overflow", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-scroll-button-short",
        displayName: "Short scroll button",
        title: "Short scroll button",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 20_000,
        eventCount: 2,
        timeline: [
          {
            kind: "user_message",
            id: "short-scroll-user",
            text: "A short prompt.",
            createdAtMs: baseTime - 10_000
          },
          {
            kind: "assistant_message",
            id: "short-scroll-assistant",
            text: "A short answer.",
            createdAtMs: baseTime - 9_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Short scroll button/);
  const thread = page.locator(".pf-chat-thread");
  await expect(page.getByText("A short answer.")).toBeVisible();

  const metrics = await chatThreadMetrics(page);
  expect(metrics.scrollHeight - metrics.clientHeight).toBeLessThanOrEqual(100);

  await thread.evaluate((node) => {
    const thread = node as HTMLDivElement;
    thread.scrollTop = 0;
    thread.dispatchEvent(new Event("scroll"));
  });
  await expect(page.getByRole("button", { name: "Scroll to bottom" })).toHaveCount(0);
});

test("composer moves submitted prompt into the thread while turn start is pending", async ({
  page
}) => {
  const prompt = "Render this send without a flash";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-smooth-send",
        displayName: "Smooth send",
        title: "Smooth send",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-smooth-send",
    250
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Smooth send/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-smooth-send" &&
      request.params.message === prompt
  );

  await page.waitForTimeout(50);
  expect(await composer.inputValue()).toBe("");
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toHaveCount(0);
});

test("rapid send activation submits the prompt only once", async ({ page }) => {
  const prompt = "Do not duplicate this prompt";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-rapid-send",
        displayName: "Rapid send",
        title: "Rapid send",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-rapid-send",
    220
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Rapid send/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-rapid-send" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "run_agent_turn" &&
        request.params.sessionId === "session-rapid-send" &&
        request.params.message === prompt
    )
  ).toHaveLength(1);
});

test("delayed initial session load preserves the first submitted prompt row", async ({
  page
}) => {
  const prompt = "First prompt while the session is still loading";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-auto-open-seed",
        displayName: "Auto open seed",
        title: "Auto open seed",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "auto-open-seed-message",
            text: "Auto-open seed transcript.",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-initial-load-race",
        displayName: "Initial load race",
        title: "Initial load race",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-initial-load-race",
    260
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Initial load race/);
  await expect(page.locator(".pf-agent-detail .primary-title")).toContainText("Initial load race");
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-initial-load-race" &&
      request.params.message === prompt
  );

  const userRow = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt });
  await expect(userRow).toHaveCount(1);
  await userRow.evaluate((node) => node.setAttribute("data-probe", "initial-local-user-row"));

  daemon.setSessionTimeline("session-initial-load-race", [
    {
      kind: "user_message",
      id: "persisted-initial-load-user",
      text: prompt,
      createdAtMs: baseTime + 1
    }
  ]);

  await page.waitForTimeout(360);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="user"][data-probe="initial-local-user-row"]')).toContainText(
    prompt
  );
});

test("early turn completion before RPC response does not leave composer stuck", async ({ page }) => {
  const prompt = "Complete before the start call returns";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-early-complete",
        displayName: "Early complete",
        title: "Early complete",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-early-complete",
    240
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Early complete/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-early-complete" &&
      request.params.message === prompt
  );

  daemon.emit("session:session-early-complete:event", {
    type: "turn-start",
    turnId: "turn-session-early-complete"
  });
  daemon.emit("session:session-early-complete:event", {
    type: "turn-complete",
    turnId: "turn-session-early-complete",
    assistantText: "Done before RPC returned."
  });

  await page.waitForTimeout(320);
  await page.locator(".pf-composer textarea").fill("Follow-up after early completion");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
});

test("early completed turn does not revive sidebar state after switching sessions", async ({ page }) => {
  const prompt = "Complete alpha before switching away";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-early-alpha",
        displayName: "Early alpha",
        title: "Early alpha",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-early-beta",
        displayName: "Early beta",
        title: "Early beta",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-early-alpha",
    240
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Early alpha/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-early-alpha" &&
      request.params.message === prompt
  );

  daemon.emit("session:session-early-alpha:event", {
    type: "turn-start",
    turnId: "turn-session-early-alpha"
  });
  daemon.emit("session:session-early-alpha:event", {
    type: "turn-complete",
    turnId: "turn-session-early-alpha",
    assistantText: "Alpha finished before RPC returned."
  });
  await openSession(page, /Early beta/);

  await page.waitForTimeout(320);
  const alphaRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Early alpha" });
  await expect(alphaRow.locator('.state[data-state="idle"]')).toContainText("idle");

  await openSession(page, /Early alpha/);
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await page.locator(".pf-composer textarea").fill("Follow-up after alpha finished");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeEnabled();
});

test("sidebar marks the selected agent thinking while turn start is pending", async ({ page }) => {
  const prompt = "Show sidebar thinking state";
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.message === prompt,
    240
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === prompt
  );

  const activeRow = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Browser regression" });
  await expect(activeRow).toContainText("thinking");
  await expect(activeRow.locator('.state[data-state="thinking"]')).toBeVisible();
});

test("persisted prompt during pending turn replaces the optimistic row", async ({ page }) => {
  const prompt = "Persist this prompt once during title reload";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-persist",
        displayName: "Pending persist",
        title: "Pending persist",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-pending-persist",
    300
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending persist/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-pending-persist" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-pending-persist"
  ).length;
  daemon.setSessionTimeline("session-pending-persist", [
    {
      kind: "user_message",
      id: "persisted-pending-user",
      text: prompt,
      createdAtMs: Date.now()
    }
  ]);
  daemon.emit("workspace:sessions:changed", {
    reason: "generated_title",
    sessionId: "session-pending-persist"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-pending-persist"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
});

test("same text can be submitted again after a recent earlier turn", async ({ page }) => {
  const prompt = "Repeatable prompt text";
  const earlierTurnAt = Date.now() - 60_000;
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-repeat-prompt",
        displayName: "Repeat prompt",
        title: "Repeat prompt",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: earlierTurnAt,
        eventCount: 1,
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "user_message",
            id: "old-repeat-user",
            text: prompt,
            createdAtMs: earlierTurnAt
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-repeat-prompt",
    240
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Repeat prompt/);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-repeat-prompt" &&
      request.params.message === prompt
  );

  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(2);
});

test("next turn start keeps previous live answer visible during reload", async ({ page }) => {
  const firstReply = "First answer should stay visible.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-next-turn",
        displayName: "Next turn",
        title: "Next turn",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Next turn/);
  daemon.emit("session:session-next-turn:event", { type: "turn-start", turnId: "turn-first" });
  daemon.emit("session:session-next-turn:event", {
    type: "text-delta",
    turnId: "turn-first",
    delta: firstReply
  });
  await expect(page.getByText(firstReply)).toBeVisible();

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-next-turn",
    360
  );
  daemon.emit("session:session-next-turn:event", {
    type: "turn-complete",
    turnId: "turn-first",
    assistantText: firstReply
  });
  await expect(page.getByText(firstReply)).toBeVisible();

  await page.locator(".pf-composer textarea").fill("Start the next turn");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Start the next turn"
  );
  daemon.emit("session:session-next-turn:event", {
    type: "turn-start",
    turnId: "turn-session-next-turn"
  });

  await expect(page.getByText(firstReply)).toBeVisible();
});

test("new turn can reuse a tool call id without replacing the previous live tool", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-tool-reuse",
        displayName: "Tool reuse",
        title: "Tool reuse",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Tool reuse/);
  daemon.emit("session:session-tool-reuse:event", {
    type: "turn-start",
    turnId: "turn-tool-first"
  });
  daemon.emit("session:session-tool-reuse:event", {
    type: "tool-invocations",
    turnId: "turn-tool-first",
    invocations: [
      {
        callId: "call-reused",
        toolId: "FirstTool",
        input: "{\"path\":\"first.txt\"}",
        output: "first output",
        success: true
      }
    ]
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "FirstTool" })).toHaveCount(1);

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-tool-reuse",
    360
  );
  daemon.emit("session:session-tool-reuse:event", {
    type: "turn-complete",
    turnId: "turn-tool-first",
    assistantText: ""
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "FirstTool" })).toHaveCount(1);

  daemon.emit("session:session-tool-reuse:event", {
    type: "turn-start",
    turnId: "turn-tool-second"
  });
  daemon.emit("session:session-tool-reuse:event", {
    type: "tool-invocations",
    turnId: "turn-tool-second",
    invocations: [
      {
        callId: "call-reused",
        toolId: "SecondTool",
        input: "{\"path\":\"second.txt\"}",
        output: "second output",
        success: true
      }
    ]
  });

  await expect(page.locator(".pf-tool").filter({ hasText: "FirstTool" })).toHaveCount(1);
  await expect(page.locator(".pf-tool").filter({ hasText: "SecondTool" })).toHaveCount(1);
});

test("transcript reload replaces pending live tool card when invocation event is missed", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-tool",
        displayName: "Pending tool",
        title: "Pending tool",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending tool/);
  daemon.emit("session:session-pending-tool:event", {
    type: "turn-start",
    turnId: "turn-pending-tool"
  });
  daemon.emit("session:session-pending-tool:event", {
    type: "tool-calls-requested",
    turnId: "turn-pending-tool",
    requests: [
      {
        callId: "call-pending",
        toolId: "Read",
        input: "{\"path\":\"README.md\"}"
      }
    ]
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "Read" })).toHaveCount(1);
  await expect(page.locator(".pf-tool").filter({ hasText: "running" })).toHaveCount(1);

  daemon.setSessionTimeline("session-pending-tool", [
    {
      kind: "tool_call",
      id: "persisted-tool-call",
      toolId: "Read",
      status: "success",
      inputText: "{\"path\":\"README.md\"}",
      inputJson: { path: "README.md" },
      outputText: "{\"content\":\"done\"}",
      createdAtMs: baseTime + 1
    }
  ]);
  daemon.emit("session:session-pending-tool:event", {
    type: "turn-complete",
    turnId: "turn-pending-tool",
    assistantText: ""
  });

  await expect(page.locator(".pf-tool").filter({ hasText: "Read" })).toHaveCount(1);
  await expect(page.locator(".pf-tool").filter({ hasText: "running" })).toHaveCount(0);
});

test("pending media Bash activity uses the standard command surface", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-image-activity",
        displayName: "Pending image activity",
        title: "Pending image activity",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "user_message",
            id: "pending-image-user",
            text: "Generate a small catalog image.",
            createdAtMs: baseTime - 30_000
          },
          {
            kind: "tool_call",
            id: "pending-image-tool",
            toolId: "Bash",
            status: "running",
            inputText: JSON.stringify({
              command:
                "puffer internal-tool image-generation --prompt 'A compact UI catalog card' --count 1",
              timeout: 600000
            }),
            inputJson: {
              command:
                "puffer internal-tool image-generation --prompt 'A compact UI catalog card' --count 1",
              timeout: 600000
            },
            outputText: "",
            createdAtMs: baseTime - 20_000
          },
          {
            kind: "assistant_message",
            id: "pending-image-assistant",
            text: "I am generating the image.",
            createdAtMs: baseTime - 10_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending image activity/);
  const activityGroup = page.locator(".activity-group").filter({ hasText: "Ran 1 command" });
  await expect(activityGroup).toBeVisible();
  await activityGroup.getByRole("button", { name: /Agent activity/ }).click();
  const shellAction = activityGroup.getByRole("button", { name: /Shell/ });
  await expect(shellAction).toContainText("running");
  await shellAction.click();
  await expect(
    activityGroup.getByText("puffer internal-tool image-generation").first()
  ).toBeVisible();
  await expect(activityGroup.getByText("(no output)")).toBeVisible();
});

test("transcript reload dedupes completed live tools by stable input signature", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-completed-tool-dedupe",
        displayName: "Completed tool dedupe",
        title: "Completed tool dedupe",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Completed tool dedupe/);
  daemon.emit("session:session-completed-tool-dedupe:event", {
    type: "turn-start",
    turnId: "turn-completed-tool-dedupe"
  });
  daemon.emit("session:session-completed-tool-dedupe:event", {
    type: "tool-invocations",
    turnId: "turn-completed-tool-dedupe",
    invocations: [
      {
        callId: "call-web-search",
        toolId: "web_search",
        input: "{\"query\":\"puffer timeline\"}",
        output: "live-only search output",
        success: true
      }
    ]
  });
  await expect(page.locator(".pf-tool").filter({ hasText: "web_search" })).toHaveCount(1);

  daemon.setSessionTimeline("session-completed-tool-dedupe", [
    {
      kind: "tool_call",
      id: "persisted-web-search",
      toolId: "WebSearch",
      status: "success",
      inputText: "{\"query\":\"puffer timeline\"}",
      inputJson: { query: "puffer timeline" },
      outputText: "persisted search output",
      createdAtMs: baseTime + 1
    }
  ]);
  daemon.emit("session:session-completed-tool-dedupe:event", {
    type: "turn-complete",
    turnId: "turn-completed-tool-dedupe",
    assistantText: ""
  });

  await expect(page.locator(".pf-tool").filter({ hasText: /web_search|WebSearch/ })).toHaveCount(1);
  await expect(page.getByText("live-only search output")).toHaveCount(0);
});

test("stop turn is disabled until the daemon returns a turn id", async ({ page }) => {
  const prompt = "Wait for a real turn id before cancel";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-pending-cancel",
        displayName: "Pending cancel",
        title: "Pending cancel",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-pending-cancel",
    240
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Pending cancel/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-pending-cancel" &&
      request.params.message === prompt
  );

  await expect(page.getByRole("button", { name: "Stop turn" })).toBeDisabled();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeEnabled({ timeout: 1_000 });
});

test("turn completion preserves live chat row identity after transcript reload", async ({ page }) => {
  const prompt = "Keep this row stable";
  const reply = "Stable streamed reply is visible.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-stable-chat",
        displayName: "Stable chat",
        title: "Stable chat",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Stable chat/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-stable-chat" &&
      request.params.message === prompt
  );

  const userRow = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt }).last();
  await expect(userRow).toBeVisible();
  await userRow.evaluate((node) => node.setAttribute("data-probe", "local-user-row"));

  const turnId = "turn-session-stable-chat";
  daemon.emit("session:session-stable-chat:event", { type: "turn-start", turnId });
  daemon.emit("session:session-stable-chat:event", {
    type: "text-delta",
    turnId,
    delta: reply
  });

  const agentRow = page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply }).last();
  await expect(agentRow).toBeVisible();
  await agentRow.evaluate((node) => node.setAttribute("data-probe", "live-agent-row"));

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-stable-chat"
  ).length;
  daemon.setSessionTimeline("session-stable-chat", [
    {
      kind: "user_message",
      id: "persisted-user-different-id",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-assistant-different-id",
      text: reply,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-stable-chat:event", {
    type: "turn-complete",
    turnId,
    assistantText: reply
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-stable-chat"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.locator('.pf-msg[data-role="user"][data-probe="local-user-row"]')).toContainText(
    prompt
  );
  await expect(page.locator('.pf-msg[data-role="agent"][data-probe="live-agent-row"]')).toContainText(
    reply
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("turn completion replaces partial streamed text after transcript reload", async ({ page }) => {
  const prompt = "Replace the partial stream";
  const partialText = "Draft fragment only";
  const finalText = "Final complete answer replaces the draft fragment.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-partial-stream",
        displayName: "Partial stream",
        title: "Partial stream",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Partial stream/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-partial-stream" &&
      request.params.message === prompt
  );

  const turnId = "turn-session-partial-stream";
  daemon.emit("session:session-partial-stream:event", { type: "turn-start", turnId });
  daemon.emit("session:session-partial-stream:event", {
    type: "text-delta",
    turnId,
    delta: partialText
  });
  await expect(page.getByText(partialText)).toBeVisible();

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-partial-stream"
  ).length;
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-partial-stream",
    180
  );
  daemon.setSessionTimeline("session-partial-stream", [
    {
      kind: "user_message",
      id: "persisted-partial-user",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-partial-assistant",
      text: finalText,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-partial-stream:event", {
    type: "turn-complete",
    turnId,
    assistantText: finalText
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-partial-stream"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await page.waitForTimeout(240);

  await expect(page.getByText(finalText)).toBeVisible();
  await expect(page.getByText(partialText)).toHaveCount(0);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: finalText })).toHaveCount(1);
});

test("settled live assistant text stays before the next submitted prompt", async ({ page }) => {
  const firstPrompt = "First prompt creates delayed live text";
  const firstReply = "Previous turn live reply should stay above the next prompt.";
  const secondPrompt = "Second prompt must not inherit previous activity";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-live-carryover",
        displayName: "Live carryover",
        title: "Live carryover",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Live carryover/);
  await page.locator(".pf-composer textarea").fill(firstPrompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-live-carryover" &&
      request.params.message === firstPrompt
  );

  const turnId = "turn-session-live-carryover";
  daemon.emit("session:session-live-carryover:event", { type: "turn-start", turnId });
  daemon.emit("session:session-live-carryover:event", {
    type: "text-delta",
    turnId,
    delta: firstReply
  });
  await expect(page.getByText(firstReply)).toBeVisible();

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-live-carryover",
    360
  );
  daemon.emit("session:session-live-carryover:event", {
    type: "turn-complete",
    turnId,
    assistantText: firstReply
  });
  await page.waitForTimeout(20);

  await page.locator(".pf-composer textarea").fill(secondPrompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-live-carryover" &&
      request.params.message === secondPrompt
  );

  const rowOrder = await page.locator(".pf-msg").evaluateAll((nodes) =>
    nodes.map((node) => (node.textContent ?? "").replace(/\s+/g, " ").trim())
  );
  const firstPromptIndex = rowOrder.findIndex((text) => text.includes(firstPrompt));
  const firstReplyIndex = rowOrder.findIndex((text) => text.includes(firstReply));
  const secondPromptIndex = rowOrder.findIndex((text) => text.includes(secondPrompt));

  expect(firstPromptIndex).toBeGreaterThanOrEqual(0);
  expect(firstReplyIndex).toBeGreaterThan(firstPromptIndex);
  expect(secondPromptIndex).toBeGreaterThan(firstReplyIndex);
});

test("turn-tagged intermediate messages stay with their original prompt", async ({ page }) => {
  const firstTurnId = "turn-old-intermediates";
  const secondTurnId = "turn-new-reply";
  const firstPrompt = "Search for hedy";
  const firstIntermediateOne = "What do you want me to do with hedy on DoorDash?";
  const firstIntermediateTwo = "I will treat hedy as a DoorDash search term unless you meant something else.";
  const firstReply = "DoorDash is showing a Cloudflare security verification.";
  const secondPrompt = "gm";
  const secondReply = "Good morning.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-turn-tagged-intermediates",
        displayName: "Turn tagged intermediates",
        title: "Turn tagged intermediates",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 6,
        timeline: [
          {
            kind: "user_message",
            id: "turn-old-user",
            text: firstPrompt,
            createdAtMs: baseTime + 1,
            turnId: firstTurnId
          },
          {
            kind: "assistant_message",
            id: "turn-old-final",
            text: firstReply,
            createdAtMs: baseTime + 40,
            turnId: firstTurnId
          },
          {
            kind: "user_message",
            id: "turn-new-user",
            text: secondPrompt,
            createdAtMs: baseTime + 50,
            turnId: secondTurnId
          },
          {
            kind: "assistant_message",
            id: "turn-old-intermediate-one",
            text: firstIntermediateOne,
            createdAtMs: baseTime + 10,
            turnId: firstTurnId
          },
          {
            kind: "assistant_message",
            id: "turn-old-intermediate-two",
            text: firstIntermediateTwo,
            createdAtMs: baseTime + 20,
            turnId: firstTurnId
          },
          {
            kind: "assistant_message",
            id: "turn-new-final",
            text: secondReply,
            createdAtMs: baseTime + 60,
            turnId: secondTurnId
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Turn tagged intermediates/);

  const firstAgentRow = page.locator('.pf-msg[data-role="agent"]').filter({ hasText: firstReply });
  const secondAgentRow = page.locator('.pf-msg[data-role="agent"]').filter({ hasText: secondReply });
  await expect(firstAgentRow).toHaveCount(1);
  await expect(secondAgentRow).toHaveCount(1);
  await expect(firstAgentRow.getByRole("button", { name: /Agent activity/ })).toContainText(
    "2 intermediate messages"
  );
  await firstAgentRow.getByRole("button", { name: /Agent activity/ }).click();
  await expect(firstAgentRow).toContainText(firstIntermediateOne);
  await expect(firstAgentRow).toContainText(firstIntermediateTwo);
  await expect(secondAgentRow).not.toContainText(firstIntermediateOne);
  await expect(secondAgentRow).not.toContainText(firstIntermediateTwo);

  const rowOrder = await page.locator(".pf-msg").evaluateAll((nodes) =>
    nodes.map((node) => (node.textContent ?? "").replace(/\s+/g, " ").trim())
  );
  const firstPromptIndex = rowOrder.findIndex((text) => text.includes(firstPrompt));
  const firstReplyIndex = rowOrder.findIndex((text) => text.includes(firstReply));
  const secondPromptIndex = rowOrder.findIndex((text) => text.includes(secondPrompt));
  const secondReplyIndex = rowOrder.findIndex((text) => text.includes(secondReply));

  expect(firstPromptIndex).toBeGreaterThanOrEqual(0);
  expect(firstReplyIndex).toBeGreaterThan(firstPromptIndex);
  expect(secondPromptIndex).toBeGreaterThan(firstReplyIndex);
  expect(secondReplyIndex).toBeGreaterThan(secondPromptIndex);
});

test("generated title reload does not duplicate the first submitted prompt", async ({ page }) => {
  const prompt = "First prompt should not flash twice";
  const reply = "First reply stays single.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-title-race",
        displayName: "Title race",
        title: "Title race",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Title race/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-title-race" &&
      request.params.message === prompt
  );
  const userRows = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt });
  await expect(userRows).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-title-race"
  ).length;
  daemon.setSessionTimeline("session-title-race", [
    {
      kind: "user_message",
      id: "persisted-first-user",
      text: prompt,
      createdAtMs: baseTime + 1
    }
  ]);
  daemon.emit("workspace:sessions:changed", {
    reason: "generated_title",
    sessionId: "session-title-race"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-title-race"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(userRows).toHaveCount(1);

  daemon.setSessionTimeline("session-title-race", [
    {
      kind: "user_message",
      id: "persisted-first-user",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-first-assistant",
      text: reply,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-title-race:event", {
    type: "turn-complete",
    turnId: "turn-session-title-race",
    assistantText: reply
  });

  await expect(userRows).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("empty generated title reload keeps the first submitted prompt mounted", async ({ page }) => {
  const prompt = "Generated title reload arrives before persistence";
  const reply = "Persistence eventually catches up.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-title-empty-reload",
        displayName: "Title empty reload",
        title: "Title empty reload",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Title empty reload/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-title-empty-reload" &&
      request.params.message === prompt
  );

  const userRows = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt });
  await expect(userRows).toHaveCount(1);
  await userRows.first().evaluate((node) => node.setAttribute("data-probe", "first-prompt-row"));

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-title-empty-reload"
  ).length;
  daemon.emit("workspace:sessions:changed", {
    reason: "generated_title",
    sessionId: "session-title-empty-reload"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-title-empty-reload"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toHaveCount(0);
  await expect(page.locator('.pf-msg[data-role="user"][data-probe="first-prompt-row"]')).toContainText(
    prompt
  );
  await expect(userRows).toHaveCount(1);

  daemon.setSessionTimeline("session-title-empty-reload", [
    {
      kind: "user_message",
      id: "persisted-empty-reload-user",
      text: prompt,
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-empty-reload-assistant",
      text: reply,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-title-empty-reload:event", {
    type: "turn-complete",
    turnId: "turn-session-title-empty-reload",
    assistantText: reply
  });

  await expect(page.locator('.pf-msg[data-role="user"][data-probe="first-prompt-row"]')).toContainText(
    prompt
  );
  await expect(userRows).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("clock-skewed transcript reload does not duplicate the submitted prompt", async ({
  page
}) => {
  const prompt = "Clock skew should not duplicate me";
  const reply = "Clock skew reply stays single.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-clock-skew",
        displayName: "Clock skew",
        title: "Clock skew",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Clock skew/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-clock-skew" &&
      request.params.message === prompt
  );

  const userRows = page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt });
  await expect(userRows).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-clock-skew"
  ).length;
  daemon.setSessionTimeline("session-clock-skew", [
    {
      kind: "user_message",
      id: "persisted-clock-skew-user",
      text: prompt,
      createdAtMs: baseTime - 10 * 60_000
    },
    {
      kind: "assistant_message",
      id: "persisted-clock-skew-assistant",
      text: reply,
      createdAtMs: baseTime - 10 * 60_000 + 1
    }
  ]);
  daemon.emit("session:session-clock-skew:event", {
    type: "turn-complete",
    turnId: "turn-session-clock-skew",
    assistantText: reply
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-clock-skew"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(userRows).toHaveCount(1);
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: reply })).toHaveCount(1);
});

test("failed turn start keeps composer draft and avoids an unsent user row", async ({ page }) => {
  const prompt = "Do not lose this failed prompt";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-failed-start",
        displayName: "Failed start",
        title: "Failed start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Failed start/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill(prompt);
  daemon.failNext("run_agent_turn", "daemon unavailable");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-failed-start" &&
      request.params.message === prompt
  );

  await expect(page.getByText("Agent start failed")).toBeVisible();
  await expect(composer).toHaveValue(prompt);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
});

test("turn errors keep the submitted prompt visible when it is not persisted", async ({ page }) => {
  const prompt = "Keep my prompt after turn error";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-turn-error",
        displayName: "Turn error",
        title: "Turn error",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Turn error/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-turn-error" &&
      request.params.message === prompt
  );
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);

  const loadRequestsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-turn-error"
  ).length;
  daemon.emit("session:session-turn-error:event", {
    type: "turn-start",
    turnId: "turn-session-turn-error"
  });
  daemon.emit("session:session-turn-error:event", {
    type: "turn-error",
    turnId: "turn-session-turn-error",
    error: "provider exploded before transcript append"
  });

  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-turn-error"
      ).length
    )
    .toBe(loadRequestsBefore + 1);
  await expect(page.locator('.pf-msg[data-role="user"]').filter({ hasText: prompt })).toHaveCount(1);
  await expect(page.getByText("provider exploded before transcript append")).toBeVisible();
});

test("rapid turn errors keep separate inline error rows", async ({ page }) => {
  await page.addInitScript(() => {
    Date.now = () => 1_700_000_000_000;
  });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await expect(page.getByText("Ready to exercise the managed browser.")).toBeVisible();
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-browser",
    400
  );
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-browser",
    400
  );

  daemon.emit("session:session-browser:event", {
    type: "turn-error",
    turnId: "turn-error-first",
    error: "First rapid turn failure."
  });
  daemon.emit("session:session-browser:event", {
    type: "turn-error",
    turnId: "turn-error-second",
    error: "Second rapid turn failure."
  });

  await expect(page.getByText("First rapid turn failure.")).toBeVisible();
  await expect(page.getByText("Second rapid turn failure.")).toBeVisible();
});

test("unsent composer drafts are preserved per session while switching", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-draft",
        displayName: "Alpha draft",
        title: "Alpha draft",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "alpha-draft-seed",
            text: "Alpha draft seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-beta-draft",
        displayName: "Beta draft",
        title: "Beta draft",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-draft-seed",
            text: "Beta draft seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha draft/);
  await expect(page.getByText("Alpha draft seed")).toBeVisible();
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("alpha-only draft");
  await expect(composer).toHaveValue("alpha-only draft");

  await openSession(page, /Beta draft/);
  await expect(page.getByText("Beta draft seed")).toBeVisible();
  await expect(composer).toHaveValue("");
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();
  await composer.fill("beta-only draft");
  await expect(composer).toHaveValue("beta-only draft");

  await openSession(page, /Alpha draft/);
  await expect(page.getByText("Alpha draft seed")).toBeVisible();
  await expect(composer).toHaveValue("alpha-only draft");

  await openSession(page, /Beta draft/);
  await expect(page.getByText("Beta draft seed")).toBeVisible();
  await expect(composer).toHaveValue("beta-only draft");

  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-beta-draft" &&
      request.params.message === "beta-only draft"
  );
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "run_agent_turn" &&
        request.params.sessionId === "session-alpha-draft"
    )
  ).toHaveLength(0);
});

test("unsent composer draft survives returning to the workspace board", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-workspace-draft",
        displayName: "Workspace draft",
        title: "Workspace draft",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Workspace draft/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Keep this draft while I check the project");
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.locator(".pf-pw-list")).toBeVisible();

  await openSession(page, /Workspace draft/);
  await expect(composer).toHaveValue("Keep this draft while I check the project");
});

test("resolved transcript permissions do not reappear as pending approvals", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-resolved-permission",
        displayName: "Resolved permission",
        title: "Resolved permission",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        timeline: [
          {
            kind: "user_message",
            id: "perm-user",
            text: "Run the command.",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "permission_dialog",
            id: "perm-allowed",
            toolId: "bash",
            state: "allowed",
            summary: "bash was allowed",
            reason: "User approved this earlier.",
            inputText: "echo ok",
            createdAtMs: baseTime - 45_000
          },
          {
            kind: "assistant_message",
            id: "perm-assistant",
            text: "The command finished.",
            createdAtMs: baseTime - 40_000
          }
        ]
      }
    ]
  });

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Resolved permission/);
  await expect(page.getByText("The command finished.")).toBeVisible();
  await expect(page.getByText("Approval needed")).toHaveCount(0);
  await expect(page.locator(".pf-agent-status-pill")).toHaveAttribute("data-status", "idle");
});

test("dismissed transcript permissions stay scoped to their session", async ({ page }) => {
  const permissionId = "timeline-1-permission";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-permission-alpha",
        displayName: "Permission Alpha",
        title: "Permission Alpha",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 2,
        timeline: [
          {
            kind: "assistant_message",
            id: "permission-alpha-seed",
            text: "Alpha needs approval",
            createdAtMs: baseTime - 50_000
          },
          {
            kind: "permission_dialog",
            id: permissionId,
            toolId: "bash",
            state: "pending",
            summary: "Alpha approval",
            reason: "Alpha pending approval.",
            inputText: "echo alpha",
            createdAtMs: baseTime - 45_000
          }
        ]
      },
      {
        sessionId: "session-permission-beta",
        displayName: "Permission Beta",
        title: "Permission Beta",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 2,
        timeline: [
          {
            kind: "assistant_message",
            id: "permission-beta-seed",
            text: "Beta needs approval",
            createdAtMs: baseTime - 90_000
          },
          {
            kind: "permission_dialog",
            id: permissionId,
            toolId: "bash",
            state: "pending",
            summary: "Beta approval",
            reason: "Beta pending approval.",
            inputText: "echo beta",
            createdAtMs: baseTime - 85_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Permission Alpha/);
  await expect(page.getByText("Alpha pending approval.")).toBeVisible();
  await page.getByRole("button", { name: "Deny" }).click();
  await expect(page.getByText("Alpha pending approval.")).toHaveCount(0);

  await openSession(page, /Permission Beta/);
  await expect(page.getByText("Beta pending approval.")).toBeVisible();
  await expect(page.getByText("Approval needed")).toBeVisible();
});

test("logging out the last provider clears active session state", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "anthropic",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "anthropic",
        displayName: "Claude",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-anthropic-history",
        displayName: "Claude history",
        title: "Claude history",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        providerId: "anthropic",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "anthropic-seed",
            text: "Anthropic seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Claude history/);
  await expect(page.getByText("Anthropic seed")).toBeVisible();

  await page.getByRole("button", { name: "Settings" }).click();
  const accountRow = page.locator(".pf-settings-row").filter({ hasText: "Account" });
  await accountRow
    .locator("div", { hasText: /^anthropic\s*·/ })
    .getByRole("button", { name: "Sign out" })
    .click();
  const logout = await daemon.waitForRequest("logout_provider");
  expect(logout.params).toMatchObject({ providerId: "anthropic" });

  await expect(page.getByText("0 providers connected")).toBeVisible();
  await expect(page.getByText("Connect a provider before starting an agent.")).toBeVisible();
  await expect(page.getByRole("button", { name: /Claude history/ })).toHaveCount(0);
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await page.keyboard.press("Enter");
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "run_agent_turn")
  ).toHaveLength(0);
});

test("reconnecting a provider re-enables an existing blocked session", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    sessions: [
      {
        sessionId: "session-anthropic-reconnect",
        displayName: "Claude reconnect",
        title: "Claude reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        providerId: "anthropic",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "anthropic-reconnect-seed",
            text: "Anthropic reconnect seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Claude reconnect/);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeDisabled();
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Reconnect Claude to continue this session."
  );

  await page.getByRole("button", { name: "Settings" }).click();
  await page.getByRole("button", { name: "Providers" }).click();
  await page
    .locator(".provider-card")
    .filter({ hasText: "Anthropic" })
    .getByRole("button", { name: "Add connect" })
    .click();
  const modal = page.getByRole("dialog", { name: "Connect Anthropic" });
  await modal.getByLabel("API key for Anthropic").fill("sk-reconnected");
  await modal.getByRole("button", { name: "Add connect" }).click();
  const login = await daemon.waitForRequest("login_with_api_key");
  expect(login.params).toMatchObject({
    providerId: "anthropic",
    apiKey: "sk-reconnected"
  });
  await expect(modal).toHaveCount(0);

  await openSession(page, /Claude reconnect/);
  await expect(composer).toBeEnabled();
  await expect(page.locator(".pf-composer-hint")).not.toContainText(
    "Reconnect Claude to continue this session."
  );
  await composer.fill("Continue after reconnect");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
  await page.getByRole("button", { name: "Send" }).click();
  const turn = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Continue after reconnect"
  );
  expect(turn.params).toMatchObject({
    sessionId: "session-anthropic-reconnect",
    providerId: "anthropic",
    modelId: "test-model"
  });
});

test("failed permission responses keep the approval prompt retryable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission",
    requestId: "permission-1",
    toolId: "bash",
    summary: "Run shell command",
    reason: "Needs workspace write access."
  });

  await expect(page.getByText("Approval needed")).toBeVisible();
  daemon.delayFailure("resolve_permission", () => true, "permission channel closed", 200);
  const deny = page.getByRole("button", { name: "Deny" });
  await deny.click();

  const request = await daemon.waitForRequest("resolve_permission");
  await expect(deny).toBeDisabled();
  expect(request.params).toMatchObject({
    turnId: "turn-permission",
    requestId: "permission-1",
    action: "deny"
  });
  await expect(page.getByText("Approval needed")).toBeVisible();
  await expect(deny).toBeEnabled();
});

test("background permission request is available when returning to that session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-permission-background-a",
        displayName: "Permission background A",
        title: "Permission background A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-permission-background-b",
        displayName: "Permission background B",
        title: "Permission background B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Permission background A/);
  await page.locator(".pf-composer textarea").fill("Ask permission in background");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-permission-background-a" &&
      request.params.message === "Ask permission in background"
  );

  const alphaRow = page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: "Permission background A" });
  await openSession(page, /Permission background B/);
  await expect(alphaRow.locator('.state[data-state="thinking"]')).toContainText("thinking");

  daemon.emit("session:session-permission-background-a:event", {
    type: "permission-request",
    turnId: "turn-session-permission-background-a",
    requestId: "permission-background-1",
    toolId: "bash",
    summary: "Run background command",
    reason: "Needs approval after switching away."
  });
  await expect(alphaRow.locator('.state[data-state="awaiting"]')).toContainText("awaiting");

  await openSession(page, /Permission background A/);
  await expect(page.getByText("Approval needed")).toBeVisible();
  await page.getByRole("button", { name: "Approve once" }).click();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-session-permission-background-a",
    requestId: "permission-background-1",
    action: "allow_once"
  });
});

test("background streamed assistant text is restored when switching back before persistence", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-bg-stream",
        displayName: "Alpha bg stream",
        title: "Alpha bg stream",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-bg-stream",
        displayName: "Beta bg stream",
        title: "Beta bg stream",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-bg-stream-seed",
            text: "Beta seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha bg stream/);
  await page.locator(".pf-composer textarea").fill("Alpha prompt");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-bg-stream"
  );

  await openSession(page, /Beta bg stream/);
  await expect(page.getByText("Beta seed")).toBeVisible();

  daemon.emit("session:session-alpha-bg-stream:event", {
    type: "text-delta",
    turnId: "turn-session-alpha-bg-stream",
    delta: "Streamed while Alpha was hidden."
  });

  await openSession(page, /Alpha bg stream/);
  await expect(page.getByText("Streamed while Alpha was hidden.")).toBeVisible();
});

test("background tool activity is restored when switching back before persistence", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-bg-tool",
        displayName: "Alpha bg tool",
        title: "Alpha bg tool",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-bg-tool",
        displayName: "Beta bg tool",
        title: "Beta bg tool",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-bg-tool-seed",
            text: "Beta tool seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha bg tool/);
  await page.locator(".pf-composer textarea").fill("Alpha tool prompt");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-bg-tool"
  );

  await openSession(page, /Beta bg tool/);
  await expect(page.getByText("Beta tool seed")).toBeVisible();

  daemon.emit("session:session-alpha-bg-tool:event", {
    type: "tool-calls-requested",
    turnId: "turn-session-alpha-bg-tool",
    requests: [
      {
        callId: "call-bg-tool",
        toolId: "HiddenTool",
        input: "{\"target\":\"background\"}"
      }
    ]
  });
  daemon.emit("session:session-alpha-bg-tool:event", {
    type: "tool-invocations",
    turnId: "turn-session-alpha-bg-tool",
    invocations: [
      {
        callId: "call-bg-tool",
        toolId: "HiddenTool",
        input: "{\"target\":\"background\"}",
        output: "hidden tool finished",
        success: true
      }
    ]
  });

  await openSession(page, /Alpha bg tool/);
  const tool = page.locator(".pf-tool").filter({ hasText: "HiddenTool" });
  await expect(tool).toHaveCount(1);
  await expect(tool).toContainText("done");
  await tool.getByRole("button", { name: /Expand tool output/ }).click();
  await expect(tool).toContainText("hidden tool finished");
});

test("completed background turns keep tool activity until persistence catches up", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-bg-tool-complete",
        displayName: "Alpha bg complete tool",
        title: "Alpha bg complete tool",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-beta-bg-tool-complete",
        displayName: "Beta bg complete tool",
        title: "Beta bg complete tool",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "beta-bg-complete-tool-seed",
            text: "Beta complete tool seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha bg complete tool/);
  await page.locator(".pf-composer textarea").fill("Alpha completed tool prompt");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-alpha-bg-tool-complete"
  );

  await openSession(page, /Beta bg complete tool/);
  await expect(page.getByText("Beta complete tool seed")).toBeVisible();

  daemon.emit("session:session-alpha-bg-tool-complete:event", {
    type: "tool-invocations",
    turnId: "turn-session-alpha-bg-tool-complete",
    invocations: [
      {
        callId: "call-bg-complete-tool",
        toolId: "CompletedHiddenTool",
        input: "{\"target\":\"background\"}",
        output: "completed hidden tool output",
        success: true
      }
    ]
  });
  daemon.emit("session:session-alpha-bg-tool-complete:event", {
    type: "turn-complete",
    turnId: "turn-session-alpha-bg-tool-complete",
    assistantText: "Completed background answer."
  });
  daemon.emit("workspace:sessions:changed", {
    sessionId: "session-alpha-bg-tool-complete",
    reason: "turn_complete"
  });

  await openSession(page, /Alpha bg complete tool/);
  await expect(page.getByText("Completed background answer.")).toBeVisible();
  const activity = page.getByRole("button", { name: /Agent activity/ });
  await expect(activity).toContainText("Used 1 tool");
  await activity.click();
  const action = page.locator(".activity-action").filter({ hasText: "CompletedHiddenTool" });
  await expect(action).toBeVisible();
  await action.click();
  const panel = page.locator(".activity-panel").filter({ hasText: "CompletedHiddenTool" });
  await expect(panel).toContainText("completed hidden tool output");
});

test("verified skill gate events render inside agent activity with check details", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-gate-activity",
        displayName: "Gate activity",
        title: "Gate activity",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Gate activity/);
  await page.locator(".pf-composer textarea").fill("Use the arxiv verified skill");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-gate-activity"
  );

  daemon.emit("session:session-gate-activity:event", {
    type: "lambda-gate",
    turnId: "turn-session-gate-activity",
    callId: "gate-admit",
    toolId: "LambdaHostCall",
    gateEvent: "host_call_admitted",
    hostTool: "arxiv_search",
    hostArgs: { query: "au:\"Hanzhi Liu\"", maxresults: 10 },
    concreteTool: "Bash",
    concreteInput: { command: "python3 arxiv_search.py" }
  });
  daemon.emit("session:session-gate-activity:event", {
    type: "tool-invocations",
    turnId: "turn-session-gate-activity",
    invocations: [
      {
        callId: "gate-bash",
        toolId: "Bash",
        input: "{\"command\":\"python3 arxiv_search.py\"}",
        output: "arxiv result",
        success: true
      }
    ]
  });
  daemon.emit("session:session-gate-activity:event", {
    type: "lambda-gate",
    turnId: "turn-session-gate-activity",
    callId: "gate-commit",
    toolId: "Bash",
    gateEvent: "host_call_committed",
    hostTool: "arxiv_search",
    hostArgs: { query: "au:\"Hanzhi Liu\"", maxresults: 10 },
    concreteTool: "Bash",
    concreteInput: { command: "python3 arxiv_search.py" },
    registeredFacts: [{ pred: "searched", args: ["au:\"Hanzhi Liu\""] }]
  });

  await expect(page.locator(".gate-toast")).toHaveCount(0);
  await expect(page.locator(".gate-detail-panel")).toHaveCount(0);
  const activity = page.getByRole("button", { name: /Agent activity/ });
  await expect(activity).toContainText("Checked 2 gates");
  await activity.click();

  const actions = page.locator(".activity-action");
  await expect(actions).toHaveCount(3);
  await expect(actions.nth(0)).toContainText("Gate admitted");
  await expect(actions.nth(1)).toContainText("Shell");
  await expect(actions.nth(2)).toContainText("Gate committed");

  const admitted = page.locator(".activity-action").filter({ hasText: "Gate admitted" });
  await expect(admitted).toContainText("arxiv_search -> Bash");
  await admitted.click();
  const panel = page.locator(".activity-panel").filter({ hasText: "Host args" });
  await expect(panel).toContainText("Verified LambdaHostCall may bind formal host tool arxiv_search");
  await expect(panel).toContainText('"query":"au:\\"Hanzhi Liu\\""');
  await expect(panel).toContainText("Concrete input");
  await expect(panel).toContainText("Compare concrete_tool with the next activity row's tool name");

  daemon.setSessionTimeline("session-gate-activity", [
    {
      kind: "user_message",
      id: "gate-user",
      text: "Use the arxiv verified skill",
      createdAtMs: baseTime
    },
    {
      kind: "tool_call",
      id: "gate-skill-tool",
      toolId: "Skill",
      status: "ok",
      summary: "Skill success",
      inputText: "{\"skill\":\"arxiv\",\"args\":\"Find the latest arXiv paper.\"}",
      outputText: "Prepared arxiv_search host call.",
      createdAtMs: baseTime + 1
    },
    {
      kind: "tool_call",
      id: "gate-host-call",
      toolId: "LambdaHostCall",
      status: "ok",
      summary: "Lambda host call admitted: arxiv_search",
      inputText:
        "{\"host_tool\":\"arxiv_search\",\"args\":{\"query\":\"au:\\\"Hanzhi Liu\\\"\",\"maxresults\":10},\"tool\":\"Bash\",\"input\":{\"command\":\"python3 arxiv_search.py\"}}",
      outputText: "",
      createdAtMs: baseTime + 2,
      metadata: {
        lambda_skill: {
          event: "host_call_admitted",
          host_tool: "arxiv_search",
          host_args: { query: "au:\"Hanzhi Liu\"", maxresults: 10 },
          concrete_tool: "Bash",
          concrete_input: { command: "python3 arxiv_search.py" }
        }
      }
    },
    {
      kind: "system_message",
      id: "gate-host-call-lambda-gate",
      text:
        "Verified Skill Gate\n" +
        "event: host_call_admitted\n" +
        "check: Verified LambdaHostCall may bind formal host tool arxiv_search to concrete tool Bash, and recorded the exact concrete input that must run next.\n" +
        "host_tool: arxiv_search\n" +
        "host_args: {\"query\":\"au:\\\"Hanzhi Liu\\\"\",\"maxresults\":10}\n" +
        "concrete_tool: Bash\n" +
        "concrete_input: {\"command\":\"python3 arxiv_search.py\"}\n" +
        "confirmation: Compare concrete_tool with the next activity row's tool name and concrete_input with that tool's input.",
      createdAtMs: baseTime + 3
    },
    {
      kind: "tool_call",
      id: "gate-bash-tool",
      toolId: "Bash",
      status: "ok",
      summary: "Command: python3 arxiv_search.py",
      inputText: "{\"command\":\"python3 arxiv_search.py\"}",
      outputText: "arxiv result",
      createdAtMs: baseTime + 4,
      metadata: {
        lambda_skill: {
          event: "host_call_committed",
          host_tool: "arxiv_search",
          host_args: { query: "au:\"Hanzhi Liu\"", maxresults: 10 },
          concrete_tool: "Bash",
          concrete_input: { command: "python3 arxiv_search.py" }
        }
      }
    },
    {
      kind: "system_message",
      id: "gate-bash-tool-lambda-gate",
      text:
        "Verified Skill Gate\n" +
        "event: host_call_committed\n" +
        "check: Confirmed the concrete Bash call matched the pending LambdaHostCall bridge for formal host tool arxiv_search.\n" +
        "host_tool: arxiv_search\n" +
        "host_args: {\"query\":\"au:\\\"Hanzhi Liu\\\"\",\"maxresults\":10}\n" +
        "concrete_tool: Bash\n" +
        "confirmation: Puffer observed the declared concrete tool succeed, then committed the Lambda gate and any registered facts.",
      createdAtMs: baseTime + 5
    },
    {
      kind: "assistant_message",
      id: "gate-assistant",
      text: "Found the arXiv paper.",
      createdAtMs: baseTime + 6
    }
  ]);
  daemon.emit("session:session-gate-activity:event", {
    type: "turn-complete",
    turnId: "turn-session-gate-activity",
    assistantText: "Found the arXiv paper."
  });
  await expect(page.getByText("Found the arXiv paper.")).toBeVisible();
  await expect(page.locator('.pf-msg[data-role="system"]').filter({ hasText: "Verified Skill Gate" })).toHaveCount(0);
  await expect(page.getByText("MCP · Bash")).toHaveCount(0);
  await expect(page.getByText("No result returned.")).toHaveCount(0);
  await expect(page.getByRole("button", { name: /Agent activity/ })).toContainText("Checked 2 gates");
});

test("daemon-running background sessions receive approval events", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-daemon-running-a",
        displayName: "Daemon running A",
        title: "Daemon running A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "running",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-daemon-running-b",
        displayName: "Daemon running B",
        title: "Daemon running B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        activityStatus: "running",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  const alphaRow = page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: "Daemon running A" });
  await expect(alphaRow.locator('.state[data-state="running"]')).toContainText("running");
  await openSession(page, /Daemon running B/);

  daemon.emit("session:session-daemon-running-a:event", {
    type: "permission-request",
    turnId: "turn-session-daemon-running-a",
    requestId: "permission-daemon-running",
    toolId: "bash",
    summary: "Approve daemon-started command",
    reason: "This approval started before the UI opened the session."
  });
  await expect(alphaRow.locator('.state[data-state="awaiting"]')).toContainText("awaiting");

  await openSession(page, /Daemon running A/);
  await expect(page.getByText("This approval started before the UI opened the session.")).toBeVisible();
  await page.getByRole("button", { name: "Approve once" }).click();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-session-daemon-running-a",
    requestId: "permission-daemon-running",
    action: "allow_once"
  });
});

test("completed background turns ignore replayed approval events", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-background-complete-a",
        displayName: "Background complete A",
        title: "Background complete A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "running",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-background-complete-b",
        displayName: "Background complete B",
        title: "Background complete B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "background-complete-b-seed",
            text: "Background complete B seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Background complete B/);
  await expect(page.getByText("Background complete B seed")).toBeVisible();
  const approvalEvent = {
    type: "permission-request",
    turnId: "turn-background-complete-a",
    requestId: "permission-background-complete",
    toolId: "bash",
    summary: "Approve stale background command",
    reason: "This approval should disappear when the turn completes."
  };
  daemon.emit("session:session-background-complete-a:event", approvalEvent);
  const alphaRow = page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: "Background complete A" });
  await expect(alphaRow.locator('.state[data-state="awaiting"]')).toContainText("awaiting");

  daemon.emit("session:session-background-complete-a:event", {
    type: "turn-complete",
    turnId: "turn-background-complete-a",
    assistantText: "Background turn finished."
  });
  daemon.emit("session:session-background-complete-a:event", {
    ...approvalEvent,
    replay: true
  });

  await openSession(page, /Background complete A/);
  await expect(page.getByText("Background turn finished.")).toBeVisible();
  await expect(page.getByText("This approval should disappear when the turn completes.")).toHaveCount(0);
  await expect(page.getByText("Approval needed")).toHaveCount(0);
});

test("background turn errors are restored when switching back before persistence", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-background-error-a",
        displayName: "Background error A",
        title: "Background error A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "running",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-background-error-b",
        displayName: "Background error B",
        title: "Background error B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "background-error-b-seed",
            text: "Background error B seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Background error B/);
  await expect(page.getByText("Background error B seed")).toBeVisible();
  daemon.emit("session:session-background-error-a:event", {
    type: "turn-error",
    turnId: "turn-background-error-a",
    error: "background provider exploded"
  });

  await openSession(page, /Background error A/);
  await expect(page.getByText("Agent error")).toBeVisible();
  await expect(page.getByText("background provider exploded")).toBeVisible();
});

test("late permission response failures do not leak into a switched session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-permission-stale-a",
        displayName: "Permission stale A",
        title: "Permission stale A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "permission-stale-a-seed",
            text: "Permission stale A seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-permission-stale-b",
        displayName: "Permission stale B",
        title: "Permission stale B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "permission-stale-b-seed",
            text: "Permission stale B seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Permission stale A/);
  await expect(page.getByText("Permission stale A seed")).toBeVisible();
  daemon.emit("session:session-permission-stale-a:event", {
    type: "permission-request",
    turnId: "turn-permission-stale",
    requestId: "permission-stale",
    toolId: "bash",
    summary: "Run stale permission command",
    reason: "This delayed failure belongs to A."
  });

  await expect(page.getByText("This delayed failure belongs to A.")).toBeVisible();
  daemon.delayFailure(
    "resolve_permission",
    (request) =>
      request.params.turnId === "turn-permission-stale" &&
      request.params.requestId === "permission-stale",
    "permission failed after switch",
    200
  );
  await page.getByRole("button", { name: "Deny" }).click();
  await daemon.waitForRequest(
    "resolve_permission",
    (request) =>
      request.params.turnId === "turn-permission-stale" &&
      request.params.requestId === "permission-stale"
  );

  await openSession(page, /Permission stale B/);
  await expect(page.getByText("Permission stale B seed")).toBeVisible();
  await page.waitForTimeout(260);

  await expect(page.getByText("Permission stale B seed")).toBeVisible();
  await expect(page.getByText("permission failed after switch")).toHaveCount(0);
});

test("successful permission response clears the awaiting approval hint", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-success",
    requestId: "permission-success",
    toolId: "bash",
    summary: "Run approved shell command",
    reason: "Needs workspace write access."
  });

  await expect(page.getByText("Approval needed")).toBeVisible();
  await expect(page.getByText(/Awaiting approval/)).toBeVisible();
  await page.getByRole("button", { name: "Approve once" }).click();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-permission-success",
    requestId: "permission-success",
    action: "allow_once"
  });
  await expect(page.getByText("Approval needed")).toHaveCount(0);
  await expect(page.getByText(/Awaiting approval/)).toHaveCount(0);
  await expect(page.getByText(/Running/)).toBeVisible();
});

test("successful permission response stays dismissed after switching away before it returns", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-permission-success-switch-a",
        displayName: "Permission success switch A",
        title: "Permission success switch A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-permission-success-switch-b",
        displayName: "Permission success switch B",
        title: "Permission success switch B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "permission-success-switch-b-seed",
            text: "Permission success switch B seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ]
  });
  daemon.delayResponse(
    "resolve_permission",
    (request) =>
      request.params.turnId === "turn-permission-success-switch" &&
      request.params.requestId === "permission-success-switch",
    250
  );

  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Permission success switch A/);
  daemon.emit("session:session-permission-success-switch-a:event", {
    type: "permission-request",
    turnId: "turn-permission-success-switch",
    requestId: "permission-success-switch",
    toolId: "bash",
    summary: "Approve delayed command",
    reason: "This approval should not reappear after success."
  });

  await expect(page.getByText("This approval should not reappear after success.")).toBeVisible();
  await page.getByRole("button", { name: "Approve once" }).click();
  await daemon.waitForRequest(
    "resolve_permission",
    (request) =>
      request.params.turnId === "turn-permission-success-switch" &&
      request.params.requestId === "permission-success-switch"
  );

  await openSession(page, /Permission success switch B/);
  await expect(page.getByText("Permission success switch B seed")).toBeVisible();
  await page.waitForTimeout(300);

  await openSession(page, /Permission success switch A/);
  await expect(page.getByText("This approval should not reappear after success.")).toHaveCount(0);
  await expect(page.getByText("Approval needed")).toHaveCount(0);
  await expect(page.getByText(/Awaiting approval/)).toHaveCount(0);
  expect(
    daemon.requests.filter((request) => request.method === "resolve_permission")
  ).toHaveLength(1);
});

test("permission responses ignore duplicate clicks while the choice is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.delayResponse("resolve_permission", () => true, 500);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-duplicate",
    requestId: "permission-duplicate",
    toolId: "bash",
    summary: "Run duplicate approval command",
    reason: "Needs a single approval."
  });

  await expect(page.getByText("Needs a single approval.")).toBeVisible();
  const allowOnce = page.getByRole("button", { name: "Approve once" });
  await allowOnce.click();
  await expect(allowOnce).toBeDisabled();

  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-permission-duplicate",
    requestId: "permission-duplicate",
    action: "allow_once"
  });
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "resolve_permission")
  ).toHaveLength(1);
});

test("new turn can reuse a permission request id after earlier approval", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-first",
    requestId: "permission-reused",
    toolId: "bash",
    summary: "Approve first command",
    reason: "First turn needs approval."
  });

  await expect(page.getByText("First turn needs approval.")).toBeVisible();
  await page.getByRole("button", { name: "Approve once" }).click();
  await daemon.waitForRequest("resolve_permission", (request) =>
    request.params.turnId === "turn-permission-first" &&
    request.params.requestId === "permission-reused"
  );
  daemon.emit("session:session-browser:event", {
    type: "turn-complete",
    turnId: "turn-permission-first",
    assistantText: ""
  });

  daemon.emit("session:session-browser:event", { type: "turn-start", turnId: "turn-permission-second" });
  daemon.emit("session:session-browser:event", {
    type: "permission-request",
    turnId: "turn-permission-second",
    requestId: "permission-reused",
    toolId: "bash",
    summary: "Approve second command",
    reason: "Second turn reuses the backend request id."
  });

  await expect(page.getByText("Second turn reuses the backend request id.")).toBeVisible();
  await expect(page.getByText("Approval needed")).toBeVisible();
});

test("ask user question live tool events render only the question prompt", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "turn-start",
    turnId: "turn-question-tool"
  });
  daemon.emit("session:session-browser:event", {
    type: "tool-calls-requested",
    turnId: "turn-question-tool",
    requests: [
      {
        callId: "ask-question-call",
        toolId: "AskUserQuestion",
        input: JSON.stringify({
          questions: [
            {
              question: "Which branch should I use?",
              options: [{ label: "main" }, { label: "feature" }]
            }
          ]
        })
      }
    ]
  });
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-tool",
    requestId: "question-tool-1",
    questions: [
      {
        question: "Which branch should I use?",
        options: [{ label: "main" }, { label: "feature" }]
      }
    ]
  });

  await expect(page.getByText("Which branch should I use?")).toBeVisible();
  await expect(page.locator(".pf-tool").filter({ hasText: "AskUserQuestion" })).toHaveCount(0);
});

test("failed question responses keep the question prompt retryable", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question",
    requestId: "question-1",
    questions: [
      {
        header: "Path",
        question: "Which path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      }
    ]
  });

  await expect(page.getByText("Which path should I use?")).toBeVisible();
  await page.getByPlaceholder("Type another answer").fill("examples");
  daemon.delayFailure("resolve_user_question", () => true, "question channel closed", 200);
  const submit = page.getByRole("button", { name: "Send answer" });
  await submit.click();

  const request = await daemon.waitForRequest("resolve_user_question");
  await expect(submit).toBeDisabled();
  expect(request.params).toMatchObject({
    turnId: "turn-question",
    requestId: "question-1",
    answers: { "Which path should I use?": "examples" },
    annotations: {}
  });
  await expect(page.getByText("Which path should I use?")).toBeVisible();
  await expect(submit).toBeEnabled();
});

test("custom-only question requires a typed answer", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-custom-only",
    requestId: "question-custom-only",
    questions: [
      {
        header: "Details",
        question: "What exact command should I run?",
        options: []
      }
    ]
  });

  await expect(page.getByText("What exact command should I run?")).toBeVisible();
  const submit = page.getByRole("button", { name: "Send answer" });
  await expect(submit).toBeDisabled();
  await page.getByPlaceholder("Type another answer").fill("npm test");
  await expect(submit).toBeEnabled();
  await submit.click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question-custom-only",
    requestId: "question-custom-only",
    answers: { "What exact command should I run?": "npm test" },
    annotations: {}
  });
});

test("input user questions render a direct answer field", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-input",
    requestId: "question-input",
    questions: [
      {
        type: "input",
        header: "Connection",
        question: "What exact connection name should Puffer use?",
        options: []
      }
    ]
  });

  const block = page.locator(".pf-question-block").filter({ hasText: "What exact connection name should Puffer use?" });
  await expect(block.getByPlaceholder("Type answer")).toBeVisible();
  await expect(block.locator(".pf-question-other")).toHaveCount(0);
  const submit = page.getByRole("button", { name: "Send answer" });
  await expect(submit).toBeDisabled();
  await block.getByPlaceholder("Type answer").fill("telegram-user");
  await expect(submit).toBeEnabled();
  await submit.click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question-input",
    requestId: "question-input",
    answers: { "What exact connection name should Puffer use?": "telegram-user" },
    annotations: {}
  });
});

test("user questions render markdown images in question text and previews", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const image =
    "data:image/svg+xml;base64,PHN2ZyB4bWxucz0iaHR0cDovL3d3dy53My5vcmcvMjAwMC9zdmciIHdpZHRoPSIyMCIgaGVpZ2h0PSIyMCI+PHJlY3Qgd2lkdGg9IjIwIiBoZWlnaHQ9IjIwIiBmaWxsPSJ3aGl0ZSIvPjxyZWN0IHg9IjQiIHk9IjQiIHdpZHRoPSIxMiIgaGVpZ2h0PSIxMiIgZmlsbD0iYmxhY2siLz48L3N2Zz4=";

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-image",
    requestId: "question-image",
    questions: [
      {
        header: "Approve",
        question: `Scan this code.\n![Telegram QR](${image})`,
        options: [
          {
            label: "Approved",
            description: "I approved the login request.",
            preview: `![Preview QR](${image})\n\ntg://login?token=abc`
          },
          { label: "Cancel", description: "Stop setup." }
        ]
      }
    ]
  });

  const block = page.locator(".pf-question-block").filter({ hasText: "Scan this code." });
  await expect(block.getByRole("img", { name: "Telegram QR" })).toBeVisible();
  await expect(block.getByRole("img", { name: "Preview QR" })).toBeVisible();
  await expect(block.getByText("tg://login?token=abc")).toBeVisible();
});

test("searchable user question choices filter connector options", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-searchable",
    requestId: "question-searchable",
    questions: [
      {
        type: "choice",
        header: "Connector",
        question: "Which connector should Puffer connect?",
        searchable: true,
        options: [
          { label: "telegram-login", description: "Telegram personal account connector" },
          { label: "email", description: "IMAP and SMTP email connector" },
          { label: "slack-login", description: "Slack login connector" }
        ]
      }
    ]
  });

  const block = page.locator(".pf-question-block").filter({ hasText: "Which connector should Puffer connect?" });
  await expect(block.getByPlaceholder("Search options")).toBeVisible();
  await expect(block.locator(".pf-question-other")).toHaveCount(0);
  await expect(block.locator(".pf-question-search-status")).toHaveText("3 options");

  await block.getByPlaceholder("Search options").fill("matrix connector");
  await expect(block.locator(".pf-question-search-status")).toHaveText("0/3 matches");
  await expect(block.locator(".pf-question-option")).toHaveCount(0);
  await expect(block.locator(".pf-question-empty")).toHaveText('No options match "matrix connector".');
  await expect(page.getByRole("button", { name: "Send answer" })).toBeDisabled();

  await block.getByPlaceholder("Search options").fill("personal connector");
  await expect(block.locator(".pf-question-search-status")).toHaveText("1/3 match");
  const telegram = block.locator(".pf-question-option").filter({ hasText: "telegram-login" });
  await expect(telegram).toBeVisible();
  await expect(block.locator(".pf-question-option").filter({ hasText: "slack-login" })).toHaveCount(0);
  await expect(block.locator(".pf-question-option").filter({ hasText: "email" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send answer" })).toBeDisabled();

  await telegram.click();
  await page.getByRole("button", { name: "Send answer" }).click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question-searchable",
    requestId: "question-searchable",
    answers: { "Which connector should Puffer connect?": "telegram-login" },
    annotations: {}
  });
});

test("duplicate question text keeps prompt draft state independent", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-duplicate-text",
    requestId: "question-duplicate-text",
    questions: [
      {
        header: "First",
        question: "Which path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      },
      {
        header: "Second",
        question: "Which path should I use?",
        options: [
          { label: "docs", description: "Use documentation." },
          { label: "examples", description: "Use examples." }
        ]
      }
    ]
  });

  const blocks = page.locator(".pf-question-block");
  await expect(blocks).toHaveCount(2);
  const firstBlock = blocks.nth(0);
  const secondBlock = blocks.nth(1);
  const firstSrc = firstBlock.locator(".pf-question-option").filter({ hasText: "src" });
  const secondDocs = secondBlock.locator(".pf-question-option").filter({ hasText: "docs" });

  await firstSrc.click();
  await expect(firstSrc).toHaveAttribute("data-selected", "true");
  await expect(secondDocs).toHaveAttribute("data-selected", "false");
  await expect(page.getByRole("button", { name: "Send answer" })).toBeDisabled();

  await secondDocs.click();
  await expect(firstSrc).toHaveAttribute("data-selected", "true");
  await expect(secondDocs).toHaveAttribute("data-selected", "true");
});

test("multiple user questions submit daemon-compatible text-keyed answers", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-multiple",
    requestId: "question-multiple",
    questions: [
      {
        header: "Source",
        question: "Which source path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      },
      {
        header: "Output",
        question: "Which output path should I use?",
        options: [
          { label: "docs", description: "Use documentation." },
          { label: "examples", description: "Use examples." }
        ]
      }
    ]
  });

  const submit = page.getByRole("button", { name: "Send answer" });
  await page
    .locator(".pf-question-block")
    .nth(0)
    .locator(".pf-question-option")
    .filter({ hasText: "src" })
    .click();
  await expect(submit).toBeDisabled();
  await page
    .locator(".pf-question-block")
    .nth(1)
    .locator(".pf-question-option")
    .filter({ hasText: "examples" })
    .click();
  await expect(submit).toBeEnabled();
  await submit.click();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question-multiple",
    requestId: "question-multiple",
    answers: {
      "Which source path should I use?": "src",
      "Which output path should I use?": "examples"
    },
    annotations: {}
  });
});

test("late question response failures do not leak into a switched session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-question-stale-a",
        displayName: "Question stale A",
        title: "Question stale A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "question-stale-a-seed",
            text: "Question stale A seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-question-stale-b",
        displayName: "Question stale B",
        title: "Question stale B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "question-stale-b-seed",
            text: "Question stale B seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Question stale A/);
  await expect(page.getByText("Question stale A seed")).toBeVisible();
  daemon.emit("session:session-question-stale-a:event", {
    type: "user-question-request",
    turnId: "turn-question-stale",
    requestId: "question-stale",
    questions: [
      {
        header: "Path",
        question: "Which stale path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      }
    ]
  });

  await expect(page.getByText("Which stale path should I use?")).toBeVisible();
  await page.getByPlaceholder("Type another answer").fill("examples");
  daemon.delayFailure(
    "resolve_user_question",
    (request) =>
      request.params.turnId === "turn-question-stale" &&
      request.params.requestId === "question-stale",
    "question failed after switch",
    200
  );
  await page.getByRole("button", { name: "Send answer" }).click();
  await daemon.waitForRequest(
    "resolve_user_question",
    (request) =>
      request.params.turnId === "turn-question-stale" &&
      request.params.requestId === "question-stale"
  );

  await openSession(page, /Question stale B/);
  await expect(page.getByText("Question stale B seed")).toBeVisible();
  await page.waitForTimeout(260);

  await expect(page.getByText("Question stale B seed")).toBeVisible();
  await expect(page.getByText("question failed after switch")).toHaveCount(0);
});

test("successful question responses stay dismissed after switching sessions", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-question-success-a",
        displayName: "Question success A",
        title: "Question success A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "question-success-a-seed",
            text: "Question success A seed",
            createdAtMs: baseTime - 30_000
          }
        ]
      },
      {
        sessionId: "session-question-success-b",
        displayName: "Question success B",
        title: "Question success B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        timeline: [
          {
            kind: "assistant_message",
            id: "question-success-b-seed",
            text: "Question success B seed",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Question success A/);
  await expect(page.getByText("Question success A seed")).toBeVisible();
  daemon.emit("session:session-question-success-a:event", {
    type: "user-question-request",
    turnId: "turn-question-success",
    requestId: "question-success",
    questions: [
      {
        header: "Path",
        question: "Which success path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      }
    ]
  });

  await expect(page.getByText("Which success path should I use?")).toBeVisible();
  await page.getByPlaceholder("Type another answer").fill("examples");
  daemon.delayResponse(
    "resolve_user_question",
    (request) =>
      request.params.turnId === "turn-question-success" &&
      request.params.requestId === "question-success",
    200
  );
  await page.getByRole("button", { name: "Send answer" }).click();
  await daemon.waitForRequest(
    "resolve_user_question",
    (request) =>
      request.params.turnId === "turn-question-success" &&
      request.params.requestId === "question-success"
  );

  await openSession(page, /Question success B/);
  await expect(page.getByText("Question success B seed")).toBeVisible();
  await page.waitForTimeout(260);
  await openSession(page, /Question success A/);

  await expect(page.getByText("Question success A seed")).toBeVisible();
  await expect(page.getByText("Which success path should I use?")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send answer" })).toHaveCount(0);
});

test("question responses ignore duplicate sends while the answer is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.delayResponse("resolve_user_question", () => true, 500);
  daemon.emit("session:session-browser:event", {
    type: "user-question-request",
    turnId: "turn-question-duplicate",
    requestId: "question-duplicate",
    questions: [
      {
        header: "Path",
        question: "Which duplicate path should I use?",
        options: [
          { label: "src", description: "Use the src directory." },
          { label: "tests", description: "Use the tests directory." }
        ]
      }
    ]
  });

  await expect(page.getByText("Which duplicate path should I use?")).toBeVisible();
  await page.getByPlaceholder("Type another answer").fill("examples");
  const submit = page.getByRole("button", { name: "Send answer" });
  await submit.click();
  await expect(submit).toBeDisabled();

  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-question-duplicate",
    requestId: "question-duplicate",
    answers: { "Which duplicate path should I use?": "examples" },
    annotations: {}
  });
  await page.waitForTimeout(50);
  expect(
    daemon.requests.filter((request) => request.method === "resolve_user_question")
  ).toHaveLength(1);
});

test("replayed approval and question events do not duplicate live prompts", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const permissionEvent = {
    type: "permission-request",
    turnId: "turn-replayed-prompts",
    requestId: "permission-replayed",
    toolId: "bash",
    summary: "Run repeated shell command",
    reason: "Run repeated shell command"
  };
  daemon.emit("session:session-browser:event", permissionEvent);
  daemon.emit("session:session-browser:event", permissionEvent);

  await expect(page.locator(".pf-approval")).toHaveCount(1);
  await expect(page.locator(".pf-approval")).toContainText("Run repeated shell command");

  const questionEvent = {
    type: "user-question-request",
    turnId: "turn-replayed-prompts",
    requestId: "question-replayed",
    questions: [
      {
        header: "Target",
        question: "Which target should I use?",
        options: [
          { label: "src", description: "Use source." },
          { label: "tests", description: "Use tests." }
        ]
      }
    ]
  };
  daemon.emit("session:session-browser:event", questionEvent);
  daemon.emit("session:session-browser:event", questionEvent);

  await expect(page.locator(".pf-question")).toHaveCount(1);
  await expect(page.locator(".pf-question")).toContainText("Which target should I use?");
});

test("composer sends selected thinking option with the turn request", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const thinkingSelect = page.getByLabel("Thinking level");
  await expect(thinkingSelect).toBeEnabled();
  await expect(thinkingSelect).toHaveValue("");
  await thinkingSelect.selectOption("high");

  await page.locator(".pf-composer textarea").fill("Use high reasoning");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use high reasoning"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "test-model",
    thinkingOptionId: "high"
  });
});

test("composer thinking and access controls stay scoped to each session", async ({ page }) => {
  const model = {
    id: "test-model",
    displayName: "Test model",
    provider: "codex",
    api: "openai-responses",
    contextWindow: 128000,
    maxOutputTokens: 4096,
    supportsReasoning: true,
    thinkingOptions: [
      {
        id: "low",
        label: "Low",
        description: "Use low reasoning effort for this turn.",
        isDefault: true
      },
      {
        id: "high",
        label: "High",
        description: "Use high reasoning effort for this turn.",
        isDefault: false
      }
    ],
    defaultThinkingOptionId: "low",
    isDefault: true
  };
  const sessionInput = (sessionId: string, title: string) => ({
    sessionId,
    displayName: title,
    title,
    cwd: "/tmp/puffer",
    folderPath: "/tmp/puffer",
    updatedAtMs: baseTime,
    createdAtMs: baseTime - 60_000,
    eventCount: 0,
    providerId: "codex",
    modelId: "test-model",
    timeline: []
  });
  const daemon = new FakeDaemon({
    sessions: [
      sessionInput("session-controls-alpha", "Controls Alpha"),
      sessionInput("session-controls-beta", "Controls Beta")
    ],
    providerModels: {
      codex: [model]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Controls Alpha/);
  const thinkingSelect = page.getByLabel("Thinking level");
  const accessSelect = page.getByLabel("Codex permissions");
  await expect(thinkingSelect).toHaveValue("");
  await expect(accessSelect).toHaveValue("workspace-write");
  await thinkingSelect.selectOption("high");
  await accessSelect.selectOption("full-access");

  await openSession(page, /Controls Beta/);
  await expect(thinkingSelect).toHaveValue("");
  await expect(accessSelect).toHaveValue("workspace-write");

  await openSession(page, /Controls Alpha/);
  await expect(thinkingSelect).toHaveValue("high");
  await expect(accessSelect).toHaveValue("full-access");

  await page.locator(".pf-composer textarea").fill("Use session scoped controls");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use session scoped controls"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "test-model",
    thinkingOptionId: "high",
    permissionMode: "full-access"
  });
});

test("composer thinking default clears the saved session override", async ({ page }) => {
  const model = {
    id: "test-model",
    displayName: "Test model",
    provider: "codex",
    api: "openai-responses",
    contextWindow: 128000,
    maxOutputTokens: 4096,
    supportsReasoning: true,
    thinkingOptions: [
      {
        id: "low",
        label: "Low",
        description: "Use low reasoning effort for this turn.",
        isDefault: true
      },
      {
        id: "high",
        label: "High",
        description: "Use high reasoning effort for this turn.",
        isDefault: false
      }
    ],
    defaultThinkingOptionId: "low",
    isDefault: true
  };
  const sessionInput = (sessionId: string, title: string) => ({
    sessionId,
    displayName: title,
    title,
    cwd: "/tmp/puffer",
    folderPath: "/tmp/puffer",
    updatedAtMs: baseTime,
    createdAtMs: baseTime - 60_000,
    eventCount: 0,
    providerId: "codex",
    modelId: "test-model",
    timeline: []
  });
  const daemon = new FakeDaemon({
    sessions: [
      sessionInput("session-thinking-default-alpha", "Thinking Default Alpha"),
      sessionInput("session-thinking-default-beta", "Thinking Default Beta")
    ],
    providerModels: {
      codex: [model]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Thinking Default Alpha/);
  const thinkingSelect = page.getByLabel("Thinking level");
  await expect(thinkingSelect).toHaveValue("");
  await thinkingSelect.selectOption("high");
  await expect(thinkingSelect).toHaveValue("high");
  await thinkingSelect.selectOption("");
  await expect(thinkingSelect).toHaveValue("");

  await openSession(page, /Thinking Default Beta/);
  await expect(thinkingSelect).toHaveValue("");

  await openSession(page, /Thinking Default Alpha/);
  await expect(thinkingSelect).toHaveValue("");

  await page.locator(".pf-composer textarea").fill("Use provider default thinking");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use provider default thinking"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "test-model",
    thinkingOptionId: null
  });
});

test("composer routing controls stay scoped to each session", async ({ page }) => {
  const model = (provider: string, id: string, displayName = id) => ({
    id,
    displayName,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const sessionInput = (sessionId: string, title: string) => ({
    sessionId,
    displayName: title,
    title,
    cwd: "/tmp/puffer",
    folderPath: "/tmp/puffer",
    updatedAtMs: baseTime,
    createdAtMs: baseTime - 60_000,
    eventCount: 0,
    providerId: "codex",
    modelId: "codex-default",
    timeline: []
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      sessionInput("session-routing-alpha", "Routing Alpha"),
      sessionInput("session-routing-beta", "Routing Beta")
    ],
    providerModels: {
      codex: [model("codex", "codex-default", "Codex Default")],
      openrouter: [
        model("openrouter", "google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Routing Alpha/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  await picker.getByRole("button", { name: "OpenRouter", exact: true }).click();
  await expect(picker.locator(".trigger")).toContainText("google/gemini-3.5-flash");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );

  await openSession(page, /Routing Beta/);
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(Codex\)/
  );

  await openSession(page, /Routing Alpha/);
  await expect(picker.locator(".trigger")).toContainText("google/gemini-3.5-flash");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );

  await page.locator(".pf-composer textarea").fill("Use session scoped routing");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use session scoped routing"
  );
  expect(request.params).toMatchObject({
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });

  await page.evaluate(() => {
    window.localStorage.removeItem("puffer-agent:session:session-routing-alpha:routing");
  });
  await daemon.open(page);

  await openSession(page, /Routing Alpha/);
  await expect(picker.locator(".trigger")).toContainText("google/gemini-3.5-flash");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );
});

test("started sessions ignore stale local routing preference", async ({ page }) => {
  const model = (provider: string, id: string, displayName = id) => ({
    id,
    displayName,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-started-routing",
        displayName: "Started routing",
        title: "Started routing",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        providerId: "codex",
        modelId: "codex-default",
        timeline: [
          {
            kind: "user_message",
            id: "started-routing-user",
            text: "This session already started on Codex.",
            createdAtMs: baseTime - 30_000
          }
        ]
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default", "Codex Default")],
      openrouter: [
        model("openrouter", "google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")
      ]
    }
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-agent:session:session-started-routing:routing",
      JSON.stringify({
        providerId: "openrouter",
        modelId: "google/gemini-3.5-flash"
      })
    );
  });
  await daemon.open(page);

  await openSession(page, /Started routing/);
  const picker = page.locator(".pf-composer .picker");
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(picker.locator(".providers")).toHaveCount(0);
  await page.locator(".pf-composer textarea").fill("Continue locked session");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Continue locked session"
  );
  expect(request.params).toMatchObject({
    sessionId: "session-started-routing",
    providerId: "codex",
    modelId: "codex-default"
  });
});

test("stale provider model fetch does not rewrite a switched session", async ({ page }) => {
  const model = (provider: string, id: string, displayName = id) => ({
    id,
    displayName,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const sessionInput = (sessionId: string, title: string) => ({
    sessionId,
    displayName: title,
    title,
    cwd: "/tmp/puffer",
    folderPath: "/tmp/puffer",
    updatedAtMs: baseTime,
    createdAtMs: baseTime - 60_000,
    eventCount: 0,
    providerId: "codex",
    modelId: "codex-default",
    timeline: []
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      sessionInput("session-provider-race-alpha", "Provider Race Alpha"),
      sessionInput("session-provider-race-beta", "Provider Race Beta")
    ],
    providerModels: {
      codex: [model("codex", "codex-default", "Codex Default")],
      openrouter: [
        model("openrouter", "google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")
      ]
    }
  });
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "openrouter",
    260
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Provider Race Alpha/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  await picker.getByRole("button", { name: "OpenRouter", exact: true }).click();
  await expect(picker.locator(".trigger")).toContainText("OpenRouter");

  await openSession(page, /Provider Race Beta/);
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(Codex\)/
  );
  await page.waitForTimeout(340);

  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(Codex\)/
  );

  await page.locator(".pf-composer textarea").fill("Use beta routing");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use beta routing"
  );
  expect(request.params).toMatchObject({
    sessionId: "session-provider-race-beta",
    providerId: "codex",
    modelId: "codex-default"
  });
});

test("default routing refresh updates an open session without explicit routing", async ({ page }) => {
  const model = (provider: string, id: string, displayName = id) => ({
    id,
    displayName,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-default-refresh",
        displayName: "Default refresh",
        title: "Default refresh",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: null,
        modelId: null,
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default", "Codex Default")],
      openrouter: [
        model("openrouter", "google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "codex",
    defaultModel: "codex-default"
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Default refresh/);
  const picker = page.locator(".pf-composer .picker");
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(Codex\)/
  );

  const settingsLoadsBefore = daemon.requests.filter(
    (request) => request.method === "load_settings_snapshot"
  ).length;
  daemon.setSettingsConfig({
    defaultProvider: "openrouter",
    defaultModel: "google/gemini-3.5-flash"
  });
  await daemon.dropConnections();
  const banner = page.locator(".connection-banner");
  await expect(banner).toContainText("Puffer backend disconnected.");
  daemon.allowConnections();
  await banner.getByRole("button", { name: "Reconnect backend" }).click();
  await expect.poll(() =>
    daemon.requests.filter((request) => request.method === "load_settings_snapshot").length
  ).toBeGreaterThan(settingsLoadsBefore);

  await expect(picker.locator(".trigger")).toContainText("google/gemini-3.5-flash");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );

  await page.locator(".pf-composer textarea").fill("Use refreshed default route");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use refreshed default route"
  );
  expect(request.params).toMatchObject({
    sessionId: "session-default-refresh",
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
});

test("session routing events refresh the open provider and model", async ({ page }) => {
  const model = (provider: string, id: string, displayName = id) => ({
    id,
    displayName,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-route-event-refresh",
        displayName: "Route event refresh",
        title: "Route event refresh",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default", "Codex Default")],
      openrouter: [
        model("openrouter", "google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Route event refresh/);
  const picker = page.locator(".pf-composer .picker");
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(Codex\)/
  );

  const loadsBefore = daemon.requests.filter(
    (request) =>
      request.method === "load_session_detail" &&
      request.params.sessionId === "session-route-event-refresh"
  ).length;
  daemon.updateSessionMetadata("session-route-event-refresh", {
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
  daemon.emit("workspace:sessions:changed", {
    reason: "session_routing",
    sessionId: "session-route-event-refresh"
  });
  await expect
    .poll(() =>
      daemon.requests.filter(
        (request) =>
          request.method === "load_session_detail" &&
          request.params.sessionId === "session-route-event-refresh"
      ).length
    )
    .toBe(loadsBefore + 1);

  await expect(picker.locator(".trigger")).toContainText("google/gemini-3.5-flash");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );

  await page.locator(".pf-composer textarea").fill("Use routed provider");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use routed provider"
  );
  expect(request.params).toMatchObject({
    sessionId: "session-route-event-refresh",
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
});

test("send in flight stays pending across backend reconnect until transcript reload", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-submit-reconnect",
        displayName: "Submit reconnect",
        title: "Submit reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-submit-reconnect",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Submit reconnect/);
  await page.locator(".pf-composer textarea").fill("Keep turn through reconnect");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Keep turn through reconnect"
  );

  await daemon.dropConnections();
  await expect(page.locator(".connection-banner")).toContainText("Puffer backend disconnected.");
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeDisabled();
  await expect(page.getByText("Agent start failed")).toHaveCount(0);

  const recoveredAt = Date.now();
  daemon.setSessionTimeline("session-submit-reconnect", [
    {
      kind: "user_message",
      id: "submit-reconnect-user",
      text: "Keep turn through reconnect",
      createdAtMs: recoveredAt
    },
    {
      kind: "assistant_message",
      id: "submit-reconnect-assistant",
      text: "Recovered answer after reconnect.",
      createdAtMs: recoveredAt + 1
    }
  ]);

  daemon.allowConnections();
  await page.getByRole("button", { name: "Reconnect backend" }).click();
  await expect(page.locator(".connection-banner")).toHaveCount(0);
  await expect(page.getByText("Recovered answer after reconnect.")).toBeVisible();
  await expect(page.getByText("Agent start failed")).toHaveCount(0);
  await page.locator(".pf-composer textarea").fill("Second turn after recovery");
  await page.getByRole("button", { name: "Send" }).click();
  const recoveredRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Second turn after recovery"
  );
  expect(recoveredRequest.params).toMatchObject({
    sessionId: "session-submit-reconnect"
  });
});

test("composer blocks new submits while backend is disconnected", async ({ page }) => {
  const prompt = "Wait for backend before sending";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-disconnected-submit",
        displayName: "Disconnected submit",
        title: "Disconnected submit",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Disconnected submit/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill(prompt);
  await daemon.dropConnections();
  await expect(page.locator(".connection-banner")).toContainText("Puffer backend disconnected.");
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeDisabled();
  await expect(page.locator(".pf-composer-hint")).toContainText("Reconnect the Puffer backend");
  await page.getByRole("button", { name: "Send", exact: true }).click({ force: true });

  await page.waitForTimeout(80);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "run_agent_turn" &&
        request.params.sessionId === "session-disconnected-submit"
    )
  ).toHaveLength(0);
  await expect(composer).toHaveValue(prompt);
});

test("backend reconnect re-subscribes the active session event stream", async ({ page }) => {
  await page.addInitScript(() => {
    const win = window as typeof window & {
      __PUFFER_DESKTOP_TEST_HOOKS__?: {
        beforeSessionSubscribe?: (sessionId: string) => void | Promise<void>;
      };
      __subscribeAttempts?: string[];
    };
    win.__subscribeAttempts = [];
    win.__PUFFER_DESKTOP_TEST_HOOKS__ = {
      beforeSessionSubscribe(sessionId: string) {
        win.__subscribeAttempts?.push(sessionId);
      }
    };
  });
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-resubscribe-reconnect",
        displayName: "Resubscribe reconnect",
        title: "Resubscribe reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Resubscribe reconnect/);
  await expect
    .poll(() =>
      page.evaluate(() =>
        ((window as typeof window & { __subscribeAttempts?: string[] }).__subscribeAttempts ?? [])
          .filter((sessionId) => sessionId === "session-resubscribe-reconnect")
          .length
      )
    )
    .toBe(1);

  await reconnectBackend(page, daemon);
  await expect
    .poll(() =>
      page.evaluate(() =>
        ((window as typeof window & { __subscribeAttempts?: string[] }).__subscribeAttempts ?? [])
          .filter((sessionId) => sessionId === "session-resubscribe-reconnect")
          .length
      )
    )
    .toBeGreaterThanOrEqual(2);
});

test("lost turn-start response clears pending start after idle reconnect", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-lost-start",
        displayName: "Lost start",
        title: "Lost start",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-lost-start",
    500
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Lost start/);
  await page.locator(".pf-composer textarea").fill("lost during start");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "lost during start"
  );

  await daemon.dropConnections();
  await expect(page.locator(".connection-banner")).toContainText("Puffer backend disconnected.");
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeDisabled();

  daemon.allowConnections();
  await page.getByRole("button", { name: "Reconnect backend" }).click();
  await expect(page.locator(".connection-banner")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(page.getByText("lost during start")).toBeVisible();

  await page.locator(".pf-composer textarea").fill("retry after reconnect");
  await page.getByRole("button", { name: "Send" }).click();
  const retryRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "retry after reconnect"
  );
  expect(retryRequest.params).toMatchObject({
    sessionId: "session-lost-start"
  });
});

test("permission prompt remains actionable after backend reconnect", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-permission-reconnect",
        displayName: "Permission reconnect",
        title: "Permission reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Permission reconnect/);
  await page.locator(".pf-composer textarea").fill("Need approval after reconnect");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-permission-reconnect"
  );

  await reconnectBackend(page, daemon);
  daemon.emit("session:session-permission-reconnect:event", {
    type: "permission-request",
    turnId: "turn-session-permission-reconnect",
    requestId: "permission-after-reconnect",
    toolId: "edit",
    summary: "Approval needed after reconnect",
    reason: "Reconnect approval probe"
  });

  await expect(page.getByText("Approval needed")).toBeVisible();
  await page.getByRole("button", { name: "Approve once" }).click();
  const request = await daemon.waitForRequest("resolve_permission");
  expect(request.params).toMatchObject({
    turnId: "turn-session-permission-reconnect",
    requestId: "permission-after-reconnect",
    action: "allow_once"
  });
});

test("question prompt remains actionable after backend reconnect", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-question-reconnect",
        displayName: "Question reconnect",
        title: "Question reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Question reconnect/);
  await page.locator(".pf-composer textarea").fill("Ask after reconnect");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-question-reconnect"
  );

  await reconnectBackend(page, daemon);
  daemon.emit("session:session-question-reconnect:event", {
    type: "user-question-request",
    turnId: "turn-session-question-reconnect",
    requestId: "question-after-reconnect",
    questions: [
      {
        question: "Which path should I use?",
        header: "Path",
        options: [{ label: "src", description: "Source tree" }]
      }
    ]
  });

  await expect(page.getByText("Which path should I use?")).toBeVisible();
  await page.locator(".pf-question-option").filter({ hasText: "src" }).click();
  await page.getByRole("button", { name: "Send answer" }).click();
  const request = await daemon.waitForRequest("resolve_user_question");
  expect(request.params).toMatchObject({
    turnId: "turn-session-question-reconnect",
    requestId: "question-after-reconnect"
  });
});

test("turn cancellation completion restores send after backend reconnect", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-reconnect",
        displayName: "Cancel reconnect",
        title: "Cancel reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Cancel reconnect/);
  await page.locator(".pf-composer textarea").fill("Cancel around reconnect");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-cancel-reconnect"
  );
  await page.getByRole("button", { name: "Stop turn" }).click();
  await daemon.waitForRequest(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-reconnect"
  );

  await reconnectBackend(page, daemon);
  daemon.emit("session:session-cancel-reconnect:event", {
    type: "turn-error",
    turnId: "turn-session-cancel-reconnect",
    error: "cancelled"
  });

  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
  await expect(page.locator(".pf-composer textarea")).toBeEnabled();
});

test("canceled turn live tool calls do not append after the next message", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-live-tools",
        displayName: "Cancel live tools",
        title: "Cancel live tools",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Cancel live tools/);
  await page.locator(".pf-composer textarea").fill("Start a tool-heavy turn");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-cancel-live-tools"
  );

  daemon.emit("session:session-cancel-live-tools:event", {
    type: "tool-calls-requested",
    turnId: "turn-session-cancel-live-tools",
    requests: [
      {
        callId: "old-tool",
        toolId: "Bash",
        input: "{\"command\":\"stale-cancel-tool\"}"
      }
    ]
  });
  const staleTool = page.locator(".pf-tool").filter({ hasText: "stale-cancel-tool" });
  await expect(staleTool).toBeVisible();

  await page.getByRole("button", { name: "Stop turn" }).click();
  await daemon.waitForRequest(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-live-tools"
  );
  daemon.emit("session:session-cancel-live-tools:event", {
    type: "turn-complete",
    turnId: "turn-session-cancel-live-tools",
    assistantText: ""
  });

  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(staleTool).toHaveCount(0);

  await page.locator(".pf-composer textarea").fill("Follow up after cancel");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Follow up after cancel"
  );

  await expect(
    page.locator('.pf-msg[data-role="user"]').filter({ hasText: "Follow up after cancel" })
  ).toBeVisible();
  await expect(staleTool).toHaveCount(0);
});

test("turn completion restores send after backend reconnect", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-complete-reconnect",
        displayName: "Complete reconnect",
        title: "Complete reconnect",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Complete reconnect/);
  await page.locator(".pf-composer textarea").fill("Complete around reconnect");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-complete-reconnect"
  );

  await reconnectBackend(page, daemon);
  daemon.emit("session:session-complete-reconnect:event", {
    type: "turn-complete",
    turnId: "turn-session-complete-reconnect",
    assistantText: "Completed after reconnect."
  });

  await expect(page.getByText("Completed after reconnect.")).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Send", exact: true })).toBeVisible();
});

test("composer sends fast mode and permission mode with the turn request", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-fast-controls",
        displayName: "Fast controls",
        title: "Fast controls",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      codex: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "codex",
          api: "openai-responses",
          contextWindow: 128000,
          maxOutputTokens: 4096,
          supportsReasoning: true,
          thinkingOptions: [
            {
              id: "medium",
              label: "Medium",
              description: "Use medium reasoning effort.",
              isDefault: true
            }
          ],
          defaultThinkingOptionId: "medium",
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Fast controls/);
  const fastToggle = page.locator(".pf-toggle-chip").filter({ hasText: "Fast" });
  await expect(fastToggle.locator("input")).toBeEnabled();
  await fastToggle.click();
  await page.getByLabel("Codex permissions").selectOption("full-access");

  await page.locator(".pf-composer textarea").fill("Use fast full access");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use fast full access"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "gpt-5",
    fastMode: true,
    permissionMode: "full-access"
  });
});

test("composer controls handle provider-prefixed session model ids", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-prefixed-model",
        displayName: "Prefixed model",
        title: "Prefixed model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "openai/gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          contextWindow: 128000,
          maxOutputTokens: 4096,
          supportsReasoning: true,
          thinkingOptions: [
            {
              id: "medium",
              label: "Medium",
              description: "Use medium reasoning effort.",
              isDefault: true
            },
            {
              id: "high",
              label: "High",
              description: "Use high reasoning effort."
            }
          ],
          defaultThinkingOptionId: "medium",
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Prefixed model/);
  const fastToggle = page.locator(".pf-toggle-chip").filter({ hasText: "Fast" });
  await expect(fastToggle.locator("input")).toBeEnabled();
  const thinkingSelect = page.getByLabel("Thinking level");
  await expect(thinkingSelect).toBeEnabled();
  await expect(thinkingSelect).toHaveValue("");
  await thinkingSelect.selectOption("high");

  await page.locator(".pf-composer textarea").fill("Use normalized model");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use normalized model"
  );
  expect(request.params).toMatchObject({
    providerId: "openai",
    modelId: "gpt-5",
    thinkingOptionId: "high"
  });
});

test("configured OpenAI provider label is not rewritten to Codex", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openai-label",
        displayName: "OpenAI label",
        title: "OpenAI label",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5.4",
        timeline: []
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5.4",
          displayName: "GPT-5.4",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenAI label/);
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenAI\)/
  );
});

test("exact OpenAI provider keeps its own model catalog when Codex alias also exists", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openai-exact-catalog",
        displayName: "OpenAI exact catalog",
        title: "OpenAI exact catalog",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5.4",
        timeline: []
      }
    ],
    providerModels: {
      codex: [
        {
          id: "codex-default",
          displayName: "Codex Default",
          provider: "codex",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ],
      openai: [
        {
          id: "gpt-5.4",
          displayName: "GPT-5.4",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenAI exact catalog/);
  const catalogRequest = await daemon.waitForRequest(
    "list_provider_models",
    (request) =>
      request.params.providerId === "codex" || request.params.providerId === "openai"
  );
  expect(catalogRequest.params).toMatchObject({ providerId: "openai" });
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenAI\)/
  );
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Use exact OpenAI catalog");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Use exact OpenAI catalog"
  );
  expect(turnRequest.params).toMatchObject({
    providerId: "openai",
    modelId: "gpt-5.4"
  });
});

test("model picker only offers authenticated agent providers", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "github",
        displayName: "GitHub",
        baseUrl: "",
        defaultApi: "oauth",
        modelCount: 0,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-provider-picker-agent-only",
        displayName: "Agent provider picker",
        title: "Agent provider picker",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Agent provider picker/);
  await page.locator(".pf-composer .picker .trigger").click();

  const providerList = page.locator(".pf-composer .picker .providers");
  await expect(providerList.getByRole("button", { name: "Codex", exact: true })).toBeVisible();
  await expect(providerList.getByRole("button", { name: "GitHub", exact: true })).toHaveCount(0);
});

test("native agent providers are available without stored auth", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [],
    providers: [
      {
        id: "puffer",
        displayName: "Puffer Native",
        baseUrl: "",
        defaultApi: "puffer",
        modelCount: 1,
        authModes: ["native"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-native-provider",
        displayName: "Native provider",
        title: "Native provider",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "puffer",
        modelId: "puffer-native",
        timeline: []
      }
    ],
    providerModels: {
      puffer: [
        {
          id: "puffer-native",
          displayName: "Puffer Native",
          provider: "puffer",
          api: "puffer",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "puffer",
    defaultModel: "puffer-native"
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Native provider/);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();
  await expect(page.locator(".pf-composer-hint")).not.toContainText("Reconnect");
  await composer.fill("Use native provider");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use native provider"
  );
  expect(request.params).toMatchObject({
    providerId: "puffer",
    modelId: "puffer-native"
  });
});

test("model picker provider buttons expose selected state", async ({ page }) => {
  const model = (provider: string, id: string) => ({
    id,
    displayName: id,
    provider,
    api: provider === "anthropic" ? "anthropic-messages" : "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "anthropic",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Anthropic",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-picker-provider-state",
        displayName: "Picker provider state",
        title: "Picker provider state",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      anthropic: [model("anthropic", "anthropic-default")]
    }
  });
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "anthropic",
    250
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Picker provider state/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  const codex = picker.getByRole("button", { name: "Codex", exact: true });
  const anthropic = picker.getByRole("button", { name: "Anthropic", exact: true });
  await expect(codex).toHaveAttribute("aria-pressed", "true");
  await expect(anthropic).toHaveAttribute("aria-pressed", "false");

  await anthropic.click();
  await expect(codex).toHaveAttribute("aria-pressed", "false");
  await expect(anthropic).toHaveAttribute("aria-pressed", "true");
});

test("model picker loads inactive provider models only after provider selection", async ({ page }) => {
  const model = (provider: string, id: string) => ({
    id,
    displayName: id,
    provider,
    api: provider === "anthropic" ? "anthropic-messages" : "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "anthropic",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Anthropic",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-lazy-picker-models",
        displayName: "Lazy picker models",
        title: "Lazy picker models",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      anthropic: [model("anthropic", "anthropic-default")]
    }
  });
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "anthropic",
    250
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Lazy picker models/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  await expect(picker.getByRole("button", { name: "Anthropic", exact: true })).toBeVisible();
  await page.waitForTimeout(80);
  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "list_provider_models" &&
        request.params.providerId === "anthropic"
    )
  ).toHaveLength(0);

  await picker.getByRole("button", { name: "Anthropic", exact: true }).click();
  await daemon.waitForRequest(
    "list_provider_models",
    (request) => request.params.providerId === "anthropic"
  );
  await expect(picker.locator(".trigger")).toContainText("anthropic-default");
});

test("model picker shows pending OpenRouter state and updates chat labels", async ({ page }) => {
  const model = (provider: string, id: string, displayName = id) => ({
    id,
    displayName,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openrouter-label",
        displayName: "OpenRouter label",
        title: "OpenRouter label",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      openrouter: [model("openrouter", "google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")]
    }
  });
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "openrouter",
    260
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenRouter label/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  await picker.getByRole("button", { name: "OpenRouter", exact: true }).click();

  await expect(picker.locator(".trigger")).toContainText("Loading models");
  await expect(picker.locator(".trigger")).toContainText("OpenRouter");
  await expect(picker.getByText(/Loading OpenRouter models/)).toBeVisible();
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();

  await expect(picker.locator(".trigger")).toContainText("google/gemini-3.5-flash");
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );
});

test("model picker keeps the selected provider visible when no models load", async ({ page }) => {
  const model = (provider: string, id: string) => ({
    id,
    displayName: id,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openrouter-empty-models",
        displayName: "OpenRouter empty models",
        title: "OpenRouter empty models",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      openrouter: []
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenRouter empty models/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  await picker.getByRole("button", { name: "OpenRouter", exact: true }).click();
  await daemon.waitForRequest(
    "list_provider_models",
    (request) => request.params.providerId === "openrouter"
  );

  await expect(picker.locator(".trigger")).toContainText("Pick model");
  await expect(picker.locator(".trigger")).toContainText("OpenRouter");
  await expect(picker.getByText("No OpenRouter models available.")).toBeVisible();
  await expect(page.locator(".pf-composer textarea")).toHaveAttribute(
    "placeholder",
    /Engineer \(OpenRouter\)/
  );
});

test("model picker keeps current provider after provider switch load failure", async ({ page }) => {
  const model = (provider: string, id: string) => ({
    id,
    displayName: id,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      },
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-provider-switch-failure",
        displayName: "Provider switch failure",
        title: "Provider switch failure",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      openrouter: [model("openrouter", "google/gemini-3.5-flash")]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Provider switch failure/);
  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(picker.locator(".row-name").filter({ hasText: /^codex-default$/ })).toBeVisible();

  daemon.failNext("list_provider_models", "transient OpenRouter model load failure");
  daemon.failNext("list_provider_models", "transient OpenRouter model validation failure");
  await picker.getByRole("button", { name: "OpenRouter", exact: true }).click();
  await daemon.waitForRequest(
    "list_provider_models",
    (request) => request.params.providerId === "openrouter"
  );

  await expect(picker.getByText("Some models failed to load")).toBeVisible();
  await expect(picker.locator(".trigger")).toContainText("codex-default");
  await expect(picker.locator(".trigger")).toContainText("Codex");
  await page.locator(".pf-composer textarea").fill("Keep using Codex");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Keep using Codex"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "codex-default"
  });
});

test("model picker marks alias-equivalent provider models as selected", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "codex",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "codex",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-model-picker-alias",
        displayName: "Model picker alias",
        title: "Model picker alias",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      codex: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "codex",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Model picker alias/);
  await page.locator(".pf-composer .picker .trigger").click();

  const currentRow = page.locator(".pf-composer .picker .row").filter({ hasText: "GPT-5" });
  await expect(currentRow).toHaveAttribute("aria-selected", "true");
  await expect(currentRow.locator(".row-name")).toHaveText("GPT-5");
});

test("model picker refreshes current provider models on reopen", async ({ page }) => {
  const model = (id: string, displayName = id) => ({
    id,
    displayName,
    provider: "openai",
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-model-picker-refresh",
        displayName: "Model picker refresh",
        title: "Model picker refresh",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [model("gpt-5", "GPT-5")]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Model picker refresh/);
  const picker = page.locator(".pf-composer .picker");
  const trigger = picker.locator(".trigger");
  await trigger.click();
  await expect(picker.locator(".row").filter({ hasText: "GPT-5" })).toBeVisible();

  await page.locator(".pf-composer textarea").click();
  await expect(picker.locator(".menu")).toHaveCount(0);
  daemon.setProviderModels("openai", [model("gpt-5.4", "GPT-5.4")]);
  await trigger.click();

  await expect(picker.locator(".row-name").filter({ hasText: /^GPT-5\.4$/ })).toBeVisible();
  await expect(picker.locator(".row-name").filter({ hasText: /^GPT-5$/ })).toHaveCount(0);
});

test("model picker keeps cached models after refresh failure", async ({ page }) => {
  const model = (id: string, displayName = id) => ({
    id,
    displayName,
    provider: "openai",
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-model-picker-refresh-failure",
        displayName: "Model picker refresh failure",
        title: "Model picker refresh failure",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [model("gpt-5", "GPT-5")]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Model picker refresh failure/);
  const picker = page.locator(".pf-composer .picker");
  const trigger = picker.locator(".trigger");
  await trigger.click();
  await expect(picker.locator(".row-name").filter({ hasText: /^GPT-5$/ })).toBeVisible();

  await page.locator(".pf-composer textarea").click();
  await expect(picker.locator(".menu")).toHaveCount(0);

  const modelLoadsBefore = daemon.requests.filter(
    (request) =>
      request.method === "list_provider_models" &&
      request.params.providerId === "openai"
  ).length;
  daemon.failNext("list_provider_models", "transient model discovery failure");
  await trigger.click();

  await expect
    .poll(
      () =>
        daemon.requests.filter(
          (request) =>
            request.method === "list_provider_models" &&
            request.params.providerId === "openai"
        ).length
    )
    .toBe(modelLoadsBefore + 1);
  await expect(picker.getByText("Some models failed to load")).toBeVisible();
  await expect(picker.locator(".row-name").filter({ hasText: /^GPT-5$/ })).toBeVisible();
});

test("composer replaces stale same-provider catalog model before submit", async ({ page }) => {
  const model = (id: string, displayName = id) => ({
    id,
    displayName,
    provider: "openai",
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "OpenAI",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-stale-openai-model",
        displayName: "Stale OpenAI model",
        title: "Stale OpenAI model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "gpt-5",
        timeline: []
      }
    ],
    providerModels: {
      openai: [model("gpt-5.4", "GPT-5.4")]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Stale OpenAI model/);
  await expect(page.locator(".pf-composer .picker .trigger")).toContainText("gpt-5.4");
  await page.locator(".pf-composer textarea").fill("Use fresh model");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use fresh model"
  );
  expect(request.params).toMatchObject({
    providerId: "openai",
    modelId: "gpt-5.4"
  });
});

test("composer waits for stale OpenRouter model validation before submit", async ({ page }) => {
  const model = (id: string, displayName = id) => ({
    id,
    displayName,
    provider: "openrouter",
    api: "openai-responses",
    contextWindow: null,
    maxOutputTokens: null,
    supportsReasoning: false,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-stale-openrouter-model",
        displayName: "Stale OpenRouter model",
        title: "Stale OpenRouter model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openrouter",
        modelId: "openrouter/owl-alpha",
        timeline: []
      }
    ],
    providerModels: {
      openrouter: [model("google/gemini-3.5-flash", "Google: Gemini 3.5 Flash")]
    }
  });
  daemon.delayResponse(
    "list_provider_models",
    (request) => request.params.providerId === "openrouter",
    260
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Stale OpenRouter model/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Use validated OpenRouter model");
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Loading OpenRouter models before sending."
  );

  await expect(page.locator(".pf-composer .picker .trigger")).toContainText(
    "google/gemini-3.5-flash"
  );
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use validated OpenRouter model"
  );
  expect(request.params).toMatchObject({
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
});

test("composer can send persisted model when catalog validation fails", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-catalog-validation-failure",
        displayName: "Catalog validation failure",
        title: "Catalog validation failure",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openrouter",
        modelId: "google/gemini-3.5-flash",
        timeline: []
      }
    ]
  });
  daemon.failNext("list_provider_models", "OpenRouter catalog unavailable");
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Catalog validation failure/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Use persisted model");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use persisted model"
  );
  expect(request.params).toMatchObject({
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
});

test("composer skips OpenRouter models that do not support agent tools", async ({ page }) => {
  const model = (
    id: string,
    displayName = id,
    supportsTools = true,
    isDefault = false
  ) => ({
    id,
    displayName,
    provider: "openrouter",
    api: "openai-responses",
    supportsTools,
    contextWindow: null,
    maxOutputTokens: null,
    supportsReasoning: false,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 2,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openrouter-no-tool-default",
        displayName: "OpenRouter tool model",
        title: "OpenRouter tool model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openrouter",
        modelId: "openrouter/owl-alpha",
        timeline: []
      }
    ],
    providerModels: {
      openrouter: [
        model("openrouter/owl-alpha", "OpenRouter: Owl Alpha", false, true),
        model("google/gemini-3.5-flash", "Google: Gemini 3.5 Flash", true, false)
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenRouter tool model/);
  await expect(page.locator(".pf-composer .picker .trigger")).toContainText(
    "google/gemini-3.5-flash"
  );

  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  const unsupportedRow = picker.locator(".row").filter({ hasText: "OpenRouter: Owl Alpha" });
  await expect(unsupportedRow).toBeDisabled();
  await expect(unsupportedRow).toContainText("No agent tools");

  await page.locator(".pf-composer textarea").fill("Use an agent-capable OpenRouter model");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use an agent-capable OpenRouter model"
  );
  expect(request.params).toMatchObject({
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
});

test("composer does not treat cataloged OpenRouter colon models as custom ids", async ({ page }) => {
  const model = (
    id: string,
    displayName = id,
    supportsTools = true,
    isDefault = false
  ) => ({
    id,
    displayName,
    provider: "openrouter",
    api: "openai-responses",
    supportsTools,
    contextWindow: null,
    maxOutputTokens: null,
    supportsReasoning: false,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault
  });
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 2,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openrouter-colon-no-tools",
        displayName: "OpenRouter colon model",
        title: "OpenRouter colon model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openrouter",
        modelId: "baidu/cobuddy:free",
        timeline: []
      }
    ],
    providerModels: {
      openrouter: [
        model("baidu/cobuddy:free", "Baidu Qianfan: CoBuddy (free)", false, true),
        model("google/gemini-3.5-flash", "Google: Gemini 3.5 Flash", true, false)
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenRouter colon model/);
  await expect(page.locator(".pf-composer .picker .trigger")).toContainText(
    "google/gemini-3.5-flash"
  );

  const picker = page.locator(".pf-composer .picker");
  await picker.locator(".trigger").click();
  const unsupportedRow = picker.locator(".row").filter({ hasText: "Baidu Qianfan" });
  await expect(unsupportedRow).toBeDisabled();
  await expect(unsupportedRow).toContainText("No agent tools");

  await page.locator(".pf-composer textarea").fill("Use a tool-capable colon fallback");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use a tool-capable colon fallback"
  );
  expect(request.params).toMatchObject({
    providerId: "openrouter",
    modelId: "google/gemini-3.5-flash"
  });
});

test("model picker closes with Escape and can reopen", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Browser regression/);
  const picker = page.locator(".pf-composer .picker");
  const trigger = picker.locator(".trigger");
  await expect(trigger).toHaveAttribute("aria-haspopup", "listbox");
  await expect(trigger).toHaveAttribute("aria-expanded", "false");

  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  await expect(picker.locator(".menu")).toBeVisible();

  await page.keyboard.press("Escape");
  await expect(trigger).toHaveAttribute("aria-expanded", "false");
  await expect(picker.locator(".menu")).toHaveCount(0);

  await trigger.click();
  await expect(trigger).toHaveAttribute("aria-expanded", "true");
  await expect(picker.locator(".menu")).toBeVisible();
});

test("model picker ignores stale provider switch responses", async ({ page }) => {
  const auth = [
    {
      providerId: "codex",
      kind: "oauth",
      email: "tester@example.com",
      expiresAtMs: null,
      scopes: [],
      planType: "test",
      organizationName: null
    },
    {
      providerId: "anthropic",
      kind: "api_key",
      email: null,
      expiresAtMs: null,
      scopes: [],
      planType: null,
      organizationName: null
    },
    {
      providerId: "puffer",
      kind: "oauth",
      email: "tester@example.com",
      expiresAtMs: null,
      scopes: [],
      planType: "test",
      organizationName: null
    }
  ];
  const providers = [
    {
      id: "codex",
      displayName: "Codex",
      baseUrl: "",
      defaultApi: "openai-responses",
      modelCount: 1,
      authModes: ["oauth"],
      sourceKind: "test",
      sourcePath: null
    },
    {
      id: "anthropic",
      displayName: "Anthropic",
      baseUrl: "",
      defaultApi: "anthropic-messages",
      modelCount: 1,
      authModes: ["api_key"],
      sourceKind: "test",
      sourcePath: null
    },
    {
      id: "puffer",
      displayName: "Puffer",
      baseUrl: "",
      defaultApi: "puffer",
      modelCount: 1,
      authModes: ["oauth"],
      sourceKind: "test",
      sourcePath: null
    }
  ];
  const model = (provider: string, id: string) => ({
    id,
    displayName: id,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth,
    providers,
    sessions: [
      {
        sessionId: "session-model-picker-race",
        displayName: "Model picker race",
        title: "Model picker race",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      anthropic: [model("anthropic", "anthropic-default")],
      puffer: [model("puffer", "puffer-default")]
    }
  });
  daemon.delayResponse("list_provider_models", (request) => request.params.providerId === "anthropic", 260);
  daemon.delayResponse("list_provider_models", (request) => request.params.providerId === "anthropic", 260);
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Model picker race/);
  const picker = page.locator(".pf-composer .picker");
  const trigger = picker.locator(".trigger");
  await expect(trigger).toContainText("codex-default");
  await trigger.click();

  const providerList = picker.locator(".providers");
  await providerList.getByRole("button", { name: "Anthropic", exact: true }).click();
  await providerList.getByRole("button", { name: "Puffer", exact: true }).click();
  await expect(trigger).toContainText("puffer-default");

  await page.waitForTimeout(340);
  await expect(trigger).toContainText("puffer-default");

  await page.locator(".pf-composer textarea").fill("Use the final provider");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use the final provider"
  );
  expect(request.params).toMatchObject({
    providerId: "puffer",
    modelId: "puffer-default"
  });
});

test("model picker rolls back to the last stable route when a rapid provider switch fails", async ({
  page
}) => {
  const auth = [
    {
      providerId: "codex",
      kind: "oauth",
      email: "tester@example.com",
      expiresAtMs: null,
      scopes: [],
      planType: "test",
      organizationName: null
    },
    {
      providerId: "anthropic",
      kind: "api_key",
      email: null,
      expiresAtMs: null,
      scopes: [],
      planType: null,
      organizationName: null
    },
    {
      providerId: "puffer",
      kind: "oauth",
      email: "tester@example.com",
      expiresAtMs: null,
      scopes: [],
      planType: "test",
      organizationName: null
    }
  ];
  const providers = [
    {
      id: "codex",
      displayName: "Codex",
      baseUrl: "",
      defaultApi: "openai-responses",
      modelCount: 1,
      authModes: ["oauth"],
      sourceKind: "test",
      sourcePath: null
    },
    {
      id: "anthropic",
      displayName: "Anthropic",
      baseUrl: "",
      defaultApi: "anthropic-messages",
      modelCount: 1,
      authModes: ["api_key"],
      sourceKind: "test",
      sourcePath: null
    },
    {
      id: "puffer",
      displayName: "Puffer",
      baseUrl: "",
      defaultApi: "puffer",
      modelCount: 1,
      authModes: ["oauth"],
      sourceKind: "test",
      sourcePath: null
    }
  ];
  const model = (provider: string, id: string) => ({
    id,
    displayName: id,
    provider,
    api: "openai-responses",
    supportsTools: true,
    supportsVision: false,
    contextWindow: null,
    maxOutputTokens: null,
    thinkingOptions: [],
    defaultThinkingOptionId: null,
    isDefault: true
  });
  const daemon = new FakeDaemon({
    auth,
    providers,
    sessions: [
      {
        sessionId: "session-model-picker-failed-race",
        displayName: "Model picker failed race",
        title: "Model picker failed race",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "codex-default",
        timeline: []
      }
    ],
    providerModels: {
      codex: [model("codex", "codex-default")],
      anthropic: [model("anthropic", "anthropic-default")],
      puffer: [model("puffer", "puffer-default")]
    }
  });
  daemon.delayResponse("list_provider_models", (request) => request.params.providerId === "anthropic", 260);
  daemon.delayFailure(
    "list_provider_models",
    (request) => request.params.providerId === "puffer",
    "Puffer models are temporarily unavailable",
    40
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Model picker failed race/);
  const picker = page.locator(".pf-composer .picker");
  const trigger = picker.locator(".trigger");
  await expect(trigger).toContainText("codex-default");
  await trigger.click();

  const providerList = picker.locator(".providers");
  await providerList.getByRole("button", { name: "Anthropic", exact: true }).click();
  await providerList.getByRole("button", { name: "Puffer", exact: true }).click();
  await daemon.waitForRequest(
    "list_provider_models",
    (request) => request.params.providerId === "puffer"
  );

  await expect(picker.getByText("Some models failed to load")).toBeVisible();
  await expect(trigger).toContainText("codex-default");
  await expect(trigger).toContainText("Codex");

  await page.waitForTimeout(340);
  await expect(trigger).toContainText("codex-default");
  await expect(trigger).toContainText("Codex");

  await page.locator(".pf-composer textarea").fill("Use the stable provider");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "Use the stable provider"
  );
  expect(request.params).toMatchObject({
    providerId: "codex",
    modelId: "codex-default"
  });
});

for (const scenario of [
  {
    label: "Codex",
    providerId: "codex",
    canonicalProviderId: "openai",
    authKind: "oauth",
    providerName: /Codex/,
    assistantText: "Codex reply is visible in the UI."
  },
  {
    label: "Claude",
    providerId: "claude",
    canonicalProviderId: "anthropic",
    authKind: "api_key",
    providerName: /Claude/,
    assistantText: "Claude reply is visible in the UI."
  }
]) {
  test(`new ${scenario.label} agent can send a turn and render the reply`, async ({ page }) => {
    const daemon = new FakeDaemon({
      sessions: [],
      auth: [
        {
          providerId: scenario.providerId,
          kind: scenario.authKind,
          email: scenario.authKind === "oauth" ? "tester@example.com" : null,
          expiresAtMs: null,
          scopes: [],
          planType: scenario.authKind === "oauth" ? "test" : null,
          organizationName: null
        }
      ],
      providers: [
        {
          id: scenario.providerId,
          displayName: scenario.label,
          baseUrl: "",
          defaultApi:
            scenario.canonicalProviderId === "openai"
              ? "openai-responses"
              : "anthropic-messages",
          modelCount: 1,
          authModes: [scenario.authKind],
          sourceKind: "test",
          sourcePath: null
        }
      ]
    });
    await daemon.install(page);
    await daemon.open(page);

    await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
    await page.getByRole("button", { name: "New agent in default workspace" }).click();
    const dialog = page.getByRole("dialog", { name: "New agent" });
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("radio", { name: scenario.providerName })).toBeVisible();
    await dialog.getByRole("button", { name: "Start agent" }).click();

    const createRequest = await daemon.waitForRequest("create_session");
    expect(createRequest.params).toMatchObject({
      cwd: "/tmp/puffer",
      providerId: scenario.canonicalProviderId
    });

    const composer = page.locator(".pf-composer textarea");
    await expect(page.getByText(/Reconnect .* to continue this session\./)).toHaveCount(0);
    await expect(composer).toBeEnabled();
    await composer.fill(`Hello from ${scenario.label}`);
    await page.getByRole("button", { name: "Send" }).click();

    const turnRequest = await daemon.waitForRequest(
      "run_agent_turn",
      (request) => request.params.message === `Hello from ${scenario.label}`
    );
    expect(turnRequest.params).toMatchObject({
      sessionId: "session-created-1",
      providerId: scenario.canonicalProviderId,
      modelId: "test-model"
    });

    const turnId = "turn-session-created-1";
    daemon.emit("session:session-created-1:event", { type: "turn-start", turnId });
    daemon.emit("session:session-created-1:event", {
      type: "text-delta",
      turnId,
      delta: scenario.assistantText
    });
    await expect(page.getByText(scenario.assistantText)).toBeVisible();
  });
}

test("new agent creation failures stay visible in the modal", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  daemon.failNext("create_session", "provider unavailable");
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
  await page.getByRole("button", { name: "New agent in default workspace" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer",
    providerId: "openai"
  });

  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("alert")).toContainText("Failed to create session: provider unavailable");
  await expect(dialog.getByRole("button", { name: "Start agent" })).toBeEnabled();
  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
});

test("new empty agent keeps first-message composer usable if detail load fails", async ({
  page
}) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
  await page.getByRole("button", { name: "New agent in default workspace" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  daemon.failNext("load_session_detail", "detail temporarily unavailable");
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer",
    providerId: "openai"
  });

  const composer = page.locator(".pf-composer textarea");
  await expect(page.getByText("Conversation load failed")).toBeVisible();
  await expect(page.getByText("detail temporarily unavailable")).toBeVisible();
  await expect(composer).toBeEnabled();
  await composer.fill("First prompt after detail failure");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "First prompt after detail failure"
  );
  expect(turnRequest.params).toMatchObject({
    sessionId: "session-created-1",
    providerId: "openai",
    modelId: "test-model"
  });
});

test("empty agent can recover by switching away from a disconnected provider", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-empty-disconnected-provider",
        displayName: "Disconnected empty agent",
        title: "Disconnected empty agent",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "anthropic",
        modelId: "claude-sonnet-4-5",
        timeline: []
      }
    ],
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openai",
        displayName: "Codex",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Claude",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Disconnected empty agent/);
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toBeVisible();
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();
  await composer.fill("Use the connected provider");
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Switch to a connected provider"
  );
  await expect(page.getByRole("button", { name: "Send" })).toBeDisabled();

  await page.locator(".pf-composer .picker .trigger").click();
  await page.getByRole("button", { name: "Codex" }).click();
  await expect(page.locator(".pf-composer .picker .trigger")).toContainText("gpt-5");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Use the connected provider"
  );
  expect(turnRequest.params).toMatchObject({
    sessionId: "session-empty-disconnected-provider",
    providerId: "openai",
    modelId: "gpt-5"
  });
});

test("auth-free local provider session can submit", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-local-ollama",
        displayName: "Local Ollama agent",
        title: "Local Ollama agent",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "ollama",
        modelId: "llama3.2",
        timeline: []
      }
    ],
    auth: [],
    providers: [
      {
        id: "ollama",
        displayName: "Ollama",
        baseUrl: "http://localhost:11434/v1",
        defaultApi: "openai-completions",
        modelCount: 1,
        authModes: [],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    providerModels: {
      ollama: [
        {
          id: "llama3.2",
          displayName: "Llama 3.2",
          provider: "ollama",
          api: "openai-completions",
          supportsTools: true,
          supportsVision: false,
          contextWindow: null,
          maxOutputTokens: null,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await openSession(page, /Local Ollama agent/);
  const composer = page.locator(".pf-composer textarea");
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toBeVisible();
  await expect(composer).toBeEnabled();
  await composer.fill("Ask local model");
  await expect(page.getByRole("button", { name: "Send" })).toBeEnabled();
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Ask local model"
  );
  expect(turnRequest.params).toMatchObject({
    sessionId: "session-local-ollama",
    providerId: "ollama",
    modelId: "llama3.2"
  });
});

test("empty agent does not recover through non-agent provider credentials", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-empty-github-only",
        displayName: "GitHub only empty agent",
        title: "GitHub only empty agent",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "anthropic",
        modelId: "claude-sonnet-4-5",
        timeline: []
      }
    ],
    auth: [
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    providers: [
      {
        id: "github",
        displayName: "GitHub",
        baseUrl: "",
        defaultApi: "oauth",
        modelCount: 0,
        authModes: ["oauth"],
        sourceKind: "test",
        sourcePath: null
      },
      {
        id: "anthropic",
        displayName: "Claude",
        baseUrl: "",
        defaultApi: "anthropic-messages",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await openSession(page, /GitHub only empty agent/);
  const composer = page.locator(".pf-composer textarea");
  await expect(page.getByText("No messages in this session yet. Send a prompt to get started.")).toBeVisible();
  await expect(composer).toBeDisabled();
  await expect(page.locator(".pf-composer-hint")).toContainText(
    "Reconnect Claude to continue this session."
  );
  await expect(page.locator(".pf-composer .picker .trigger")).toBeDisabled();
  await page.getByRole("button", { name: "Send" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
  });

  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "run_agent_turn")).toHaveLength(0);
});

test("stop turn requests cancellation for the active turn", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill("Cancel this turn");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Cancel this turn"
  );
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();
  await page.getByRole("button", { name: "Stop turn" }).click();

  const cancelRequest = await daemon.waitForRequest("cancel_turn");
  expect(cancelRequest.params).toMatchObject({
    turnId: "turn-session-browser"
  });
});

test("stop turn is disabled while cancellation is in flight", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-browser",
    240
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill("Cancel this turn once");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Cancel this turn once"
  );
  const stop = page.getByRole("button", { name: "Stop turn" });
  await expect(stop).toBeEnabled();

  await stop.click();
  await daemon.waitForRequest("cancel_turn");
  await expect(stop).toBeDisabled();
  await stop.click({ force: true });
  await page.waitForTimeout(40);

  expect(daemon.requests.filter((request) => request.method === "cancel_turn")).toHaveLength(1);
});

test("stop turn stays disabled when delayed start response follows cancel request", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-start-race",
        displayName: "Cancel start race",
        title: "Cancel start race",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-cancel-start-race",
    260
  );
  daemon.delayResponse(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-start-race",
    520
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Cancel start race/);
  await page.locator(".pf-composer textarea").fill("Cancel after early turn start");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-cancel-start-race" &&
      request.params.message === "Cancel after early turn start"
  );

  daemon.emit("session:session-cancel-start-race:event", {
    type: "turn-start",
    turnId: "turn-session-cancel-start-race"
  });

  const stop = page.getByRole("button", { name: "Stop turn" });
  await expect(stop).toBeEnabled();
  await stop.click();
  await daemon.waitForRequest(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-start-race"
  );
  await expect(stop).toBeDisabled();

  await page.waitForTimeout(320);
  await expect(stop).toBeDisabled();
  await stop.click({ force: true });
  await page.waitForTimeout(40);

  expect(
    daemon.requests.filter(
      (request) =>
        request.method === "cancel_turn" &&
        request.params.turnId === "turn-session-cancel-start-race"
    )
  ).toHaveLength(1);
});

test("workspace settled event clears selected canceled turn state", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-workspace-settled",
        displayName: "Cancel workspace settled",
        title: "Cancel workspace settled",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Cancel workspace settled/);
  await page.locator(".pf-composer textarea").fill("Cancel via workspace event");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-cancel-workspace-settled" &&
      request.params.message === "Cancel via workspace event"
  );

  await page.getByRole("button", { name: "Stop turn" }).click();
  await daemon.waitForRequest(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-workspace-settled"
  );

  daemon.emit("workspace:sessions:changed", {
    sessionId: "session-cancel-workspace-settled",
    reason: "turn_complete"
  });

  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(page.locator(".pf-composer textarea")).toBeEnabled();
  await expect(page.locator(".pf-agent-status-pill")).toContainText("Idle");
});

test("canceled selected idle session stays idle when reopened from workspace", async ({ page }) => {
  const prompt = "Cancel same selected session";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-same-reopen",
        displayName: "Cancel same reopen",
        title: "Cancel same reopen",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Cancel same reopen/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-cancel-same-reopen" &&
      request.params.message === prompt
  );

  await page.getByRole("button", { name: "Stop turn" }).click();
  await daemon.waitForRequest(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-same-reopen"
  );

  await page.getByRole("button", { name: "Back" }).click();

  const card = page.locator(".pf-pw-agent").filter({ hasText: "Cancel same reopen" });
  await expect(card.locator('.status-pill[data-status="idle"]')).toContainText("idle");

  await card.click();
  await expect(page.locator(".pf-agent-status-pill")).toContainText("Idle");
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
  await expect(page.locator(".pf-composer textarea")).toBeEnabled();
});

test("canceled idle session does not revive running state when reopened", async ({ page }) => {
  const prompt = "Cancel then reopen idle";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-reopen",
        displayName: "Canceled idle target",
        title: "Canceled idle target",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: []
      },
      {
        sessionId: "session-cancel-reopen-other",
        displayName: "Other idle session",
        title: "Other idle session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 1,
        activityStatus: "idle",
        providerId: "codex",
        modelId: "test-model",
        timeline: [
          {
            kind: "assistant_message",
            id: "cancel-reopen-other-seed",
            text: "Other session is idle.",
            createdAtMs: baseTime - 90_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Canceled idle target/);
  await page.locator(".pf-composer textarea").fill(prompt);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) =>
      request.params.sessionId === "session-cancel-reopen" &&
      request.params.message === prompt
  );

  await page.getByRole("button", { name: "Stop turn" }).click();
  await daemon.waitForRequest(
    "cancel_turn",
    (request) => request.params.turnId === "turn-session-cancel-reopen"
  );

  await openSession(page, /Other idle session/);
  await expect(page.getByText("Other session is idle.")).toBeVisible();
  daemon.setSessionTimeline("session-cancel-reopen", [
    {
      kind: "user_message",
      id: "cancel-reopen-user",
      text: prompt,
      createdAtMs: baseTime + 1
    }
  ]);

  await openSession(page, /Canceled idle target/);

  const row = page.locator(".pf-sidebar-agent-row").filter({ hasText: "Canceled idle target" }).first();
  await expect(row.locator('.state[data-state="idle"]')).toContainText("idle");
  await expect(page.locator(".pf-agent-status-pill")).toContainText("Idle");
  await expect(page.getByRole("button", { name: "Stop turn" })).toHaveCount(0);
});

test("session title edit saves through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-title-edit",
        displayName: "Title edit",
        title: "Title edit",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Title edit/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Renamed mission");
  await page.getByRole("button", { name: "Save title" }).click();

  const request = await daemon.waitForRequest("rename_session");
  expect(request.params).toMatchObject({
    sessionId: "session-title-edit",
    title: "Renamed mission"
  });
  await expect(page.locator(".primary-title")).toHaveText("Renamed mission");
  await expect(page.getByRole("button", { name: /Renamed mission/ }).first()).toBeVisible();
});

test("late title rename responses do not overwrite a switched session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-rename",
        displayName: "Alpha rename",
        title: "Alpha rename",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      },
      {
        sessionId: "session-beta-rename",
        displayName: "Beta rename",
        title: "Beta rename",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "rename_session",
    (request) => request.params.sessionId === "session-alpha-rename",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha rename/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Alpha renamed late");
  await page.getByRole("button", { name: "Save title" }).click();
  await daemon.waitForRequest("rename_session", (request) =>
    request.params.sessionId === "session-alpha-rename" &&
    request.params.title === "Alpha renamed late"
  );

  await openSession(page, /Beta rename/);
  await expect(page.locator(".primary-title")).toHaveText("Beta rename");

  await page.waitForTimeout(170);
  await expect(page.locator(".primary-title")).toHaveText("Beta rename");
  await expect(page.getByText("Alpha renamed late")).toHaveCount(0);
});

test("late title save does not cancel editing in a switched session", async ({
  page
}) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-alpha-rename-editing",
        displayName: "Alpha rename editing",
        title: "Alpha rename editing",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      },
      {
        sessionId: "session-beta-rename-editing",
        displayName: "Beta rename editing",
        title: "Beta rename editing",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  daemon.delayResponse(
    "rename_session",
    (request) => request.params.sessionId === "session-alpha-rename-editing",
    160
  );
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Alpha rename editing/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Alpha saved late");
  await page.getByRole("button", { name: "Save title" }).click();
  await daemon.waitForRequest("rename_session", (request) =>
    request.params.sessionId === "session-alpha-rename-editing" &&
    request.params.title === "Alpha saved late"
  );

  await openSession(page, /Beta rename editing/);
  await page.getByRole("button", { name: "Edit session title" }).click();
  await page.getByLabel("Session title").fill("Beta still editing");
  await page.waitForTimeout(220);

  const titleInput = page.locator('input[aria-label="Session title"]');
  await expect(titleInput).toBeVisible();
  await expect(titleInput).toHaveValue("Beta still editing");
});

test("auto recap does not start a second turn while one is running", async ({ page }) => {
  await page.clock.install({ time: baseTime });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.locator(".pf-composer textarea").fill("Keep this turn running");
  await page.getByRole("button", { name: "Send" }).click();

  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Keep this turn running"
  );
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();

  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.fastForward(180_001);
  await page.evaluate(() => Promise.resolve());

  expect(
    daemon.requests.filter(
      (request) => request.method === "run_agent_turn" && request.params.message === "/recap"
    )
  ).toHaveLength(0);
});

test("recap renders as an expandable card without the slash command row", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-recap-card",
        displayName: "Recap card",
        title: "Recap card",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 3,
        timeline: [
          {
            kind: "user_message",
            id: "recap-user",
            text: "Tighten auth refresh.",
            createdAtMs: baseTime - 30_000
          },
          {
            kind: "command",
            id: "recap-command",
            commandName: "recap",
            commandArgs: "",
            createdAtMs: baseTime - 20_000
          },
          {
            kind: "system_message",
            id: "recap-system",
            text:
              "\u203B recap: Auth refresh is ready for the next test run.\n\n" +
              "OAuth refresh remains the active blocker.\n\n" +
              "Run cargo test after the status branch lands.",
            createdAtMs: baseTime - 10_000
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Recap card/);

  const recapCard = page.locator(".recap-card");
  await expect(recapCard).toBeVisible();
  await expect(recapCard).toContainText("Auth refresh is ready for the next test run.");
  await expect(page.getByText(/^recap$/)).toHaveCount(0);
  await expect(page.getByText("OAuth refresh remains the active blocker.")).toHaveCount(0);

  await recapCard.getByRole("button", { name: "Expand recap details" }).click();
  await expect(page.getByText("OAuth refresh remains the active blocker.")).toBeVisible();
  await expect(page.getByText("Run cargo test after the status branch lands.")).toBeVisible();
});

test("auto recap waits while the composer has an unsent draft", async ({ page }) => {
  await page.clock.install({ time: baseTime });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const composer = page.locator(".pf-composer textarea");
  await composer.fill("Half-written thought");

  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.fastForward(180_001);
  await page.evaluate(() => Promise.resolve());

  expect(
    daemon.requests.filter(
      (request) => request.method === "run_agent_turn" && request.params.message === "/recap"
    )
  ).toHaveLength(0);
  await expect(composer).toHaveValue("Half-written thought");
});

test("auto recap does not run for an empty session", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-empty-recap",
        displayName: "Empty recap",
        title: "Empty recap",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await page.addInitScript(() => {
    (window as unknown as { __RECAP_IDLE_MS_OVERRIDE: number }).__RECAP_IDLE_MS_OVERRIDE = 100;
  });
  await daemon.open(page);

  await openSession(page, /Empty recap/);
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.waitForTimeout(180);

  expect(
    daemon.requests.filter(
      (request) => request.method === "run_agent_turn" && request.params.message === "/recap"
    )
  ).toHaveLength(0);
});

test("auto recap does not run after returning to the workspace board", async ({ page }) => {
  await page.clock.install({ time: baseTime });
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  await page.getByRole("button", { name: "Back" }).click();
  await expect(page.locator(".pf-pw-list")).toBeVisible();

  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.clock.fastForward(180_001);
  await page.evaluate(() => Promise.resolve());

  expect(
    daemon.requests.filter(
      (request) => request.method === "run_agent_turn" && request.params.message === "/recap"
    )
  ).toHaveLength(0);
});

test("streamed assistant text stays visible through transcript reload", async ({ page }) => {
  const streamedText = "Streaming answer stays stable across reload.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-streaming",
        displayName: "Streaming session",
        title: "Streaming session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Streaming session/);
  await page.evaluate((phrase) => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    const samples: number[] = [];
    let stopped = false;
    const sample = () => {
      const text = document.querySelector(".pf-chat-thread")?.textContent ?? "";
      samples.push(text.split(phrase).length - 1);
      if (!stopped) window.requestAnimationFrame(sample);
    };
    win.__chatSamples = samples;
    win.__stopChatSampling = () => {
      stopped = true;
    };
    window.requestAnimationFrame(sample);
  }, streamedText);

  await page.locator(".pf-composer textarea").fill("Stream this answer");
  await page.getByRole("button", { name: "Send" }).click();
  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-streaming"
  );
  expect(turnRequest.params.message).toBe("Stream this answer");
  const turnId = "turn-session-streaming";
  daemon.emit("session:session-streaming:event", { type: "turn-start", turnId });
  daemon.emit("session:session-streaming:event", {
    type: "text-delta",
    turnId,
    delta: streamedText
  });

  await expect(page.getByText(streamedText)).toBeVisible();
  await expect(page.locator('.pf-msg[data-role="agent"]').filter({ hasText: /^Running\b/ })).toHaveCount(0);
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-streaming",
    180
  );
  daemon.setSessionTimeline("session-streaming", [
    {
      kind: "user_message",
      id: "persisted-user",
      text: "Stream this answer",
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-assistant",
      text: streamedText,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-streaming:event", {
    type: "turn-complete",
    turnId,
    assistantText: streamedText
  });

  await expect(page.getByText(streamedText)).toBeVisible();
  await page.waitForTimeout(260);
  const samples = await page.evaluate(() => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    win.__stopChatSampling?.();
    return win.__chatSamples ?? [];
  });
  const firstVisible = samples.findIndex((count) => count > 0);
  expect(firstVisible).toBeGreaterThanOrEqual(0);
  expect(samples.slice(firstVisible)).not.toContain(0);
  expect(Math.max(...samples.slice(firstVisible))).toBe(1);
});

test("reopening the active streaming agent keeps live turn state", async ({ page }) => {
  const streamedText = "Same agent reopen keeps the active stream.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-active-reopen",
        displayName: "Active reopen",
        title: "Active reopen",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Active reopen/);
  await page.locator(".pf-composer textarea").fill("Keep streaming while I reopen");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-active-reopen"
  );
  const turnId = "turn-session-active-reopen";
  daemon.emit("session:session-active-reopen:event", { type: "turn-start", turnId });
  daemon.emit("session:session-active-reopen:event", {
    type: "text-delta",
    turnId,
    delta: streamedText
  });

  await expect(page.getByText(streamedText)).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-active-reopen",
    120
  );
  await page
    .locator(".pf-sidebar-agent-row")
    .filter({ hasText: "Active reopen" })
    .getByRole("button", { name: /Active reopen/ })
    .click();

  await page.waitForTimeout(180);
  await expect(page.getByText(streamedText)).toBeVisible();
  await expect(page.getByRole("button", { name: "Stop turn" })).toBeVisible();
});

test("late session subscription does not leave duplicate event listeners", async ({ page }) => {
  await page.addInitScript(() => {
    const win = window as typeof window & {
      __PUFFER_DESKTOP_TEST_HOOKS__?: {
        beforeSessionSubscribe?: (sessionId: string) => void | Promise<void>;
      };
      __releaseDelayedSubscribe?: () => void;
      __subscribeAttempts?: string[];
    };
    let delayed = false;
    win.__subscribeAttempts = [];
    win.__PUFFER_DESKTOP_TEST_HOOKS__ = {
      beforeSessionSubscribe(sessionId: string) {
        win.__subscribeAttempts?.push(sessionId);
        if (sessionId !== "session-subscribe-alpha" || delayed) return;
        delayed = true;
        return new Promise<void>((resolve) => {
          win.__releaseDelayedSubscribe = resolve;
        });
      }
    };
  });

  const delta = "Beta listener should render once.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-subscribe-alpha",
        displayName: "Subscribe Alpha",
        title: "Subscribe Alpha",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      },
      {
        sessionId: "session-subscribe-beta",
        displayName: "Subscribe Beta",
        title: "Subscribe Beta",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1_000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        timeline: []
      },
      {
        sessionId: "session-subscribe-gamma",
        displayName: "Subscribe Gamma",
        title: "Subscribe Gamma",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 2_000,
        createdAtMs: baseTime - 180_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Subscribe Alpha/);
  await page.waitForFunction(() =>
    (window as typeof window & { __subscribeAttempts?: string[] })
      .__subscribeAttempts?.includes("session-subscribe-alpha")
  );
  await openSession(page, /Subscribe Beta/);
  await expect(page.locator(".pf-agent-detail .primary-title")).toHaveText("Subscribe Beta");
  await page.evaluate(() =>
    (window as typeof window & { __releaseDelayedSubscribe?: () => void })
      .__releaseDelayedSubscribe?.()
  );
  await openSession(page, /Subscribe Gamma/);
  await expect(page.locator(".pf-agent-detail .primary-title")).toHaveText("Subscribe Gamma");
  await openSession(page, /Subscribe Beta/);
  await expect(page.locator(".pf-agent-detail .primary-title")).toHaveText("Subscribe Beta");

  const turnId = "turn-subscribe-beta";
  daemon.emit("session:session-subscribe-beta:event", { type: "turn-start", turnId });
  daemon.emit("session:session-subscribe-beta:event", {
    type: "text-delta",
    turnId,
    delta
  });

  const assistantRow = page.locator('.pf-msg[data-role="agent"]').filter({ hasText: delta });
  await expect(assistantRow).toHaveCount(1);
  await expect(assistantRow.locator(".pf-msg-text")).toHaveText(delta);
});

test("final-only assistant text appears before delayed transcript reload", async ({ page }) => {
  const finalText = "Final-only answer appears before reload finishes.";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-final-only",
        displayName: "Final-only session",
        title: "Final-only session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Final-only session/);
  await page.evaluate((phrase) => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    const samples: number[] = [];
    let stopped = false;
    const sample = () => {
      const text = document.querySelector(".pf-chat-thread")?.textContent ?? "";
      samples.push(text.split(phrase).length - 1);
      if (!stopped) window.requestAnimationFrame(sample);
    };
    win.__chatSamples = samples;
    win.__stopChatSampling = () => {
      stopped = true;
    };
    window.requestAnimationFrame(sample);
  }, finalText);

  await page.locator(".pf-composer textarea").fill("Return a final-only answer");
  await page.getByRole("button", { name: "Send" }).click();
  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-final-only"
  );
  expect(turnRequest.params.message).toBe("Return a final-only answer");
  const turnId = "turn-session-final-only";
  daemon.emit("session:session-final-only:event", { type: "turn-start", turnId });

  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-final-only",
    220
  );
  daemon.setSessionTimeline("session-final-only", [
    {
      kind: "user_message",
      id: "persisted-user-final-only",
      text: "Return a final-only answer",
      createdAtMs: baseTime + 1
    },
    {
      kind: "assistant_message",
      id: "persisted-assistant-final-only",
      text: finalText,
      createdAtMs: baseTime + 2
    }
  ]);
  daemon.emit("session:session-final-only:event", {
    type: "turn-complete",
    turnId,
    assistantText: finalText
  });

  await page.waitForTimeout(80);
  const preReloadSamples = await page.evaluate(() => {
    const win = window as typeof window & {
      __chatSamples?: number[];
    };
    return win.__chatSamples ?? [];
  });
  expect(Math.max(...preReloadSamples)).toBeGreaterThan(0);

  await expect(page.getByText(finalText)).toBeVisible();
  await page.waitForTimeout(300);
  const samples = await page.evaluate(() => {
    const win = window as typeof window & {
      __chatSamples?: number[];
      __stopChatSampling?: () => void;
    };
    win.__stopChatSampling?.();
    return win.__chatSamples ?? [];
  });
  const firstVisible = samples.findIndex((count) => count > 0);
  expect(firstVisible).toBeGreaterThanOrEqual(0);
  expect(samples.slice(firstVisible)).not.toContain(0);
  expect(Math.max(...samples.slice(firstVisible))).toBe(1);
});

test("replayed turn-start does not clear visible streamed text", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId: "turn-replay",
    delta: "Visible text before replay."
  });
  await expect(page.getByText("Visible text before replay.")).toBeVisible();

  daemon.emit("session:session-browser:event", {
    type: "turn-start",
    turnId: "turn-replay",
    replay: true
  });

  await expect(page.getByText("Visible text before replay.")).toBeVisible();
});

test("replayed text deltas only fill missing streamed text", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /^Browser regression\b/);
  const turnId = "turn-replay-delta";
  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId,
    delta: "ha"
  });
  const latestAgentParagraph = page.locator('.pf-msg[data-role="agent"] p').last();
  await expect(latestAgentParagraph).toHaveText("ha");

  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId,
    delta: "ha",
    replay: true
  });
  await expect(latestAgentParagraph).toHaveText("ha");

  daemon.emit("session:session-browser:event", {
    type: "text-delta",
    turnId,
    delta: "ha",
    replay: true
  });
  await expect(latestAgentParagraph).toHaveText("haha");
  await expect(latestAgentParagraph).not.toHaveText("hahaha");
});

test("stale turn reloads do not clear the active streamed answer", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-overlap",
        displayName: "Overlap session",
        title: "Overlap session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Overlap session/);
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-overlap",
    180
  );
  daemon.setSessionTimeline("session-overlap", [
    {
      kind: "assistant_message",
      id: "persisted-old",
      text: "Persisted old answer.",
      createdAtMs: baseTime + 1
    }
  ]);

  daemon.emit("session:session-overlap:event", { type: "turn-start", turnId: "turn-old" });
  daemon.emit("session:session-overlap:event", {
    type: "text-delta",
    turnId: "turn-old",
    delta: "Transient old answer."
  });
  daemon.emit("session:session-overlap:event", {
    type: "turn-complete",
    turnId: "turn-old",
    assistantText: "Persisted old answer."
  });

  daemon.emit("session:session-overlap:event", { type: "turn-start", turnId: "turn-new" });
  daemon.emit("session:session-overlap:event", {
    type: "text-delta",
    turnId: "turn-new",
    delta: "Current answer must stay visible."
  });
  await expect(page.getByText("Current answer must stay visible.")).toBeVisible();

  await page.waitForTimeout(260);
  await expect(page.getByText("Current answer must stay visible.")).toBeVisible();

  daemon.emit("session:session-overlap:event", {
    type: "text-delta",
    turnId: "turn-old",
    delta: "Late stale text should be ignored."
  });
  await expect(page.getByText("Late stale text should be ignored.")).toHaveCount(0);
  await expect(page.getByText("Current answer must stay visible.")).toBeVisible();
});

test("late completion for an overlapped turn prevents later stale deltas", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-overlap-complete",
        displayName: "Overlap completion",
        title: "Overlap completion",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Overlap completion/);
  daemon.delayResponse(
    "load_session_detail",
    (request) => request.params.sessionId === "session-overlap-complete",
    180
  );
  daemon.setSessionTimeline("session-overlap-complete", [
    {
      kind: "assistant_message",
      id: "persisted-new",
      text: "Persisted new answer.",
      createdAtMs: baseTime + 2
    }
  ]);

  await page.locator(".pf-composer textarea").fill("start current turn");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.sessionId === "session-overlap-complete"
  );
  daemon.emit("session:session-overlap-complete:event", {
    type: "text-delta",
    turnId: "turn-session-overlap-complete",
    delta: "New transient answer."
  });
  await expect(page.getByText("New transient answer.")).toBeVisible();

  daemon.emit("session:session-overlap-complete:event", {
    type: "turn-complete",
    turnId: "turn-old",
    assistantText: "Persisted old answer."
  });
  daemon.emit("session:session-overlap-complete:event", {
    type: "turn-complete",
    turnId: "turn-session-overlap-complete",
    assistantText: "Persisted new answer."
  });

  await page.waitForTimeout(260);
  daemon.emit("session:session-overlap-complete:event", {
    type: "text-delta",
    turnId: "turn-old",
    delta: "Very late old delta should stay ignored."
  });
  await expect(page.getByText("Very late old delta should stay ignored.")).toHaveCount(0);
  await expect(page.getByText("Persisted new answer.")).toBeVisible();
});

test("streaming agent row keeps its DOM identity without a local user row", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-remote-stream",
        displayName: "Remote stream",
        title: "Remote stream",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Remote stream/);
  daemon.emit("session:session-remote-stream:event", {
    type: "turn-start",
    turnId: "turn-remote-stream"
  });
  daemon.emit("session:session-remote-stream:event", {
    type: "text-delta",
    turnId: "turn-remote-stream",
    delta: "Identity"
  });
  await expect(page.getByText("Identity")).toBeVisible();

  await page.evaluate(() => {
    const win = window as typeof window & {
      __agentRowStillConnected?: () => boolean;
    };
    const row = document.querySelector(".pf-msg[data-role='agent']");
    win.__agentRowStillConnected = () => row?.isConnected === true;
  });

  daemon.emit("session:session-remote-stream:event", {
    type: "text-delta",
    turnId: "turn-remote-stream",
    delta: " safe"
  });
  daemon.emit("session:session-remote-stream:event", {
    type: "text-delta",
    turnId: "turn-remote-stream",
    delta: " stream"
  });

  await expect(page.getByText("Identity safe stream")).toBeVisible();
  await expect.poll(() =>
    page.evaluate(() => {
      const win = window as typeof window & {
        __agentRowStillConnected?: () => boolean;
      };
      return win.__agentRowStillConnected?.() ?? false;
    })
  ).toBe(true);
});

test("model guard preserves session model not in provider advertised list", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openai",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ],
    sessions: [
      {
        sessionId: "session-custom-model",
        displayName: "Custom model session",
        title: "Custom model session",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openai",
        modelId: "ft:gpt-4o-2024-08-06:my-org::abc123",
        timeline: []
      }
    ],
    providerModels: {
      openai: [
        {
          id: "gpt-5",
          displayName: "GPT-5",
          provider: "openai",
          api: "openai-responses",
          contextWindow: 128000,
          maxOutputTokens: 4096,
          supportsReasoning: false,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  daemon.setSettingsConfig({
    defaultProvider: "openai",
    defaultModel: "gpt-5"
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Custom model session/);
  await page.locator(".pf-composer textarea").fill("hello");
  await page.getByRole("button", { name: "Send" }).click();
  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "hello"
  );
  expect(request.params).toMatchObject({
    providerId: "openai",
    modelId: "ft:gpt-4o-2024-08-06:my-org::abc123"
  });
});

test("model guard preserves OpenRouter auto route model ids", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "openrouter",
        kind: "api_key",
        email: null,
        expiresAtMs: null,
        scopes: [],
        planType: null,
        organizationName: null
      }
    ],
    providers: [
      {
        id: "openrouter",
        displayName: "OpenRouter",
        baseUrl: "",
        defaultApi: "openai-responses",
        modelCount: 1,
        authModes: ["api_key"],
        sourceKind: "test",
        sourcePath: null
      }
    ],
    sessions: [
      {
        sessionId: "session-openrouter-auto-model",
        displayName: "OpenRouter auto model",
        title: "OpenRouter auto model",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        providerId: "openrouter",
        modelId: "openrouter/auto",
        timeline: []
      }
    ],
    providerModels: {
      openrouter: [
        {
          id: "google/gemini-3.5-flash",
          displayName: "Google: Gemini 3.5 Flash",
          provider: "openrouter",
          api: "openai-responses",
          contextWindow: null,
          maxOutputTokens: null,
          supportsReasoning: false,
          thinkingOptions: [],
          defaultThinkingOptionId: null,
          isDefault: true
        }
      ]
    }
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /OpenRouter auto model/);
  await expect(page.locator(".pf-composer .picker .trigger")).toContainText("openrouter/auto");
  await page.locator(".pf-composer textarea").fill("use auto route");
  await page.getByRole("button", { name: "Send" }).click();

  const request = await daemon.waitForRequest(
    "run_agent_turn",
    (item) => item.params.message === "use auto route"
  );
  expect(request.params).toMatchObject({
    providerId: "openrouter",
    modelId: "openrouter/auto"
  });
});

test("cancel turn for already-completed turn clears stuck cancel state", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-cancel-stale",
        displayName: "Cancel stale",
        title: "Cancel stale",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Cancel stale/);
  await page.locator(".pf-composer textarea").fill("hello");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (r) => r.params.sessionId === "session-cancel-stale"
  );

  daemon.emit("session:session-cancel-stale:event", {
    type: "text-delta",
    turnId: "turn-session-cancel-stale",
    delta: "Streaming response."
  });
  await expect(page.getByText("Streaming response.")).toBeVisible();

  daemon.emit("session:session-cancel-stale:event", {
    type: "turn-complete",
    turnId: "turn-session-cancel-stale",
    assistantText: "Done."
  });
  await page.waitForTimeout(100);

  await page.locator(".pf-composer textarea").fill("second message");
  await page.getByRole("button", { name: "Send" }).click();
  await daemon.waitForRequest(
    "run_agent_turn",
    (r) => r.params.message === "second message"
  );

  daemon.emit("session:session-cancel-stale:event", {
    type: "turn-complete",
    turnId: "turn-session-cancel-stale",
    assistantText: "Second done."
  });
  await page.waitForTimeout(100);

  const stopButton = page.getByRole("button", { name: /stop/i });
  if (await stopButton.isVisible()) {
    await stopButton.click();
    await page.waitForTimeout(200);
  }

  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toBeEnabled();
});

test("session list renders when a session has null routing fields", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-good-routing",
        displayName: "Good routing",
        title: "Good routing",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 1,
        timeline: [],
        providerId: "codex",
        modelId: "claude-sonnet-4-6"
      },
      {
        sessionId: "session-null-routing",
        displayName: "Null routing",
        title: "Null routing",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        timeline: [],
        providerId: null,
        modelId: null
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("button", { name: /Good routing/ }).first()).toBeVisible();
  await expect(page.getByRole("button", { name: /Null routing/ }).first()).toBeVisible();
});

test("auto-recap timer does not fire on a different session after switch", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-recap-a",
        displayName: "Recap A",
        title: "Recap A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      },
      {
        sessionId: "session-recap-b",
        displayName: "Recap B",
        title: "Recap B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        updatedAtMs: baseTime - 1000,
        createdAtMs: baseTime - 120_000,
        eventCount: 0,
        timeline: []
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await openSession(page, /Recap A/);

  await page.evaluate(() => {
    (window as unknown as { __RECAP_IDLE_MS_OVERRIDE: number }).__RECAP_IDLE_MS_OVERRIDE = 200;
  });
  await page.evaluate(() => window.dispatchEvent(new Event("blur")));
  await page.waitForTimeout(50);

  await openSession(page, /Recap B/);
  await page.waitForTimeout(300);

  const recapRequests = daemon.requests.filter(
    (r) => r.method === "run_agent_turn" && r.params.message === "/recap"
  );
  expect(recapRequests.length).toBe(0);
});

test("remembered session with workspace root is not restored when current root is empty", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-remembered",
        displayName: "Remembered",
        title: "Remembered",
        cwd: "/projects/alpha",
        folderPath: "/projects/alpha",
        updatedAtMs: baseTime,
        createdAtMs: baseTime - 60_000,
        eventCount: 0,
        timeline: []
      }
    ],
    workspaceRoot: ""
  });
  await daemon.install(page);

  await page.addInitScript(() => {
    window.localStorage.setItem(
      "puffer-desktop:remembered-session",
      JSON.stringify({ sessionId: "session-remembered", workspaceRoot: "/projects/alpha" })
    );
    window.localStorage.setItem(
      "puffer-desktop:preferences",
      JSON.stringify({ rememberSession: true })
    );
  });
  await daemon.open(page);

  await page.waitForTimeout(500);
  const composer = page.locator(".pf-composer textarea");
  await expect(composer).toHaveCount(0);
});
