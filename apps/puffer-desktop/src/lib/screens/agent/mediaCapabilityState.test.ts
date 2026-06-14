import { expect, test } from "vitest";
import type { MediaCapabilityInfo, MediaKind } from "../../types";
import {
  availableMediaCapabilities,
  mediaCapabilityConnectStateMessage
} from "./mediaCapabilityState";

function capability(overrides: Partial<MediaCapabilityInfo> = {}): MediaCapabilityInfo {
  return {
    providerId: "relaydance",
    providerDisplayName: "Relaydance",
    modelId: "doubao-seedance-2-0-720p",
    modelDisplayName: "Seedance 2.0 (720p)",
    kind: "video",
    operation: "generate",
    axes: [
      {
        id: "resolution",
        label: "Mode",
        role: "param",
        control: { enum: { values: ["720p"], default: "720p" } }
      }
    ],
    status: "unavailable",
    source: "static",
    reason: "missing_auth",
    checkedAtMs: 42,
    ...overrides
  };
}

test("availableMediaCapabilities filters by kind and available status", () => {
  const capabilities = [
    capability({ status: "available", reason: null }),
    capability({ kind: "image" as MediaKind, status: "available", reason: null }),
    capability({ providerId: "xai", status: "unavailable", reason: "missing_auth" })
  ];

  expect(availableMediaCapabilities(capabilities, "video")).toEqual([
    capability({ status: "available", reason: null })
  ]);
});

test("mediaCapabilityConnectStateMessage deduplicates provider display names", () => {
  const capabilities = [
    capability(),
    capability({ modelId: "another-model" }),
    capability({ providerId: "xai", providerDisplayName: "", reason: "missing_auth" })
  ];

  expect(mediaCapabilityConnectStateMessage(capabilities, "video")).toBe(
    "Connect Relaydance or xai to enable video generation."
  );
});

test("mediaCapabilityConnectStateMessage appears only for unavailable video providers", () => {
  expect(mediaCapabilityConnectStateMessage([capability()], "video")).toBe(
    "Connect Relaydance to enable video generation."
  );
  expect(
    mediaCapabilityConnectStateMessage(
      [capability(), capability({ providerId: "xai", providerDisplayName: "xAI" })],
      "video"
    )
  ).toBe("Connect Relaydance or xAI to enable video generation.");
  expect(
    mediaCapabilityConnectStateMessage([capability({ status: "available", reason: null })], "video")
  ).toBeNull();
  expect(mediaCapabilityConnectStateMessage([capability({ kind: "image" })], "image")).toBeNull();
});

test("mediaCapabilityConnectStateMessage ignores non-auth unavailable states", () => {
  expect(
    mediaCapabilityConnectStateMessage(
      [
        capability({ reason: "adapter_unavailable" }),
        capability({ status: "unknown", reason: "missing_auth" })
      ],
      "video"
    )
  ).toBeNull();
});
