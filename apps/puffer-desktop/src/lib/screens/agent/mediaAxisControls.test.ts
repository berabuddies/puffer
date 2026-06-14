import { expect, test } from "vitest";
import {
  axisControlKind,
  axisDefaultValue,
  axisOptions,
  capabilityAxesError,
  normalizeAxisSelections,
  selectionIsValid
} from "./mediaAxisControls";

const enumAxis = {
  id: "aspect_ratio",
  label: "Aspect ratio",
  role: "param",
  control: { enum: { values: ["16:9", "9:16"], default: "16:9" } }
};

const rangeAxis = {
  id: "duration_seconds",
  label: "Duration",
  role: "param",
  control: { range: { min: 4, max: 12, step: 2, default: 6 } }
};

const boolAxis = {
  id: "audio",
  label: "Native audio",
  role: "selector",
  control: { bool: { default: true } }
};

const canonicalImageAxes = [
  {
    id: "mode",
    label: "Mode",
    role: "param",
    control: { enum: { values: ["1K SD", "2K HD"], default: "1K SD" } }
  },
  {
    id: "ratio",
    label: "Ratio",
    role: "param",
    control: { enum: { values: ["Auto", "1:1", "16:9"], default: "Auto" } }
  },
  {
    id: "output",
    label: "Output",
    role: "param",
    control: { range: { min: 1, max: 9, step: 1, default: 1 } }
  }
];

const canonicalVideoAxes = [
  {
    id: "resolution",
    label: "Mode",
    role: "param",
    control: { enum: { values: ["720p", "1080p"], default: "1080p" } }
  },
  {
    id: "ratio",
    label: "Ratio",
    role: "param",
    control: { enum: { values: ["Auto", "16:9", "9:16"], default: "Auto" } }
  },
  {
    id: "duration",
    label: "Duration",
    role: "param",
    control: { range: { min: 4, max: 12, step: 1, default: 5 } }
  },
  boolAxis
];

test("axis helpers expose enum metadata", () => {
  expect(axisControlKind(enumAxis)).toBe("enum");
  expect(axisOptions(enumAxis)).toEqual(["16:9", "9:16"]);
  expect(axisDefaultValue(enumAxis)).toBe("16:9");
  expect(selectionIsValid(enumAxis, "9:16")).toBe(true);
  expect(selectionIsValid(enumAxis, "1:1")).toBe(false);
});

test("axis helpers normalize bool and range values to strings", () => {
  expect(axisControlKind(rangeAxis)).toBe("range");
  expect(axisDefaultValue(rangeAxis)).toBe("6");
  expect(selectionIsValid(rangeAxis, "8")).toBe(true);
  expect(selectionIsValid(rangeAxis, "9")).toBe(false);
  expect(axisControlKind(boolAxis)).toBe("bool");
  expect(axisDefaultValue(boolAxis)).toBe("true");
  expect(selectionIsValid(boolAxis, "false")).toBe(true);
});

test("normalizeAxisSelections keeps valid saved values and drops stale keys", () => {
  expect(
    normalizeAxisSelections([enumAxis, rangeAxis, boolAxis], {
      aspect_ratio: "9:16",
      duration_seconds: "20",
      audio: "false",
      stale_video_option: "remove-me"
    })
  ).toEqual({
    aspect_ratio: "9:16",
    duration_seconds: "6",
    audio: "false"
  });
});

test("canonical image axes expose mode ratio and output only", () => {
  expect(canonicalImageAxes.map((axis) => axis.label)).toEqual(["Mode", "Ratio", "Output"]);
  expect(canonicalImageAxes.map((axis) => axis.id)).not.toContain("size");
  expect(canonicalImageAxes.map((axis) => axis.id)).not.toContain("quality");
  expect(canonicalImageAxes.map((axis) => axis.id)).not.toContain("output_format");

  expect(
    normalizeAxisSelections(canonicalImageAxes, {
      mode: "2K HD",
      ratio: "16:9",
      output: "4",
      size: "1024x1024"
    })
  ).toEqual({
    mode: "2K HD",
    ratio: "16:9",
    output: "4"
  });
});

test("switching image models refreshes stale output max", () => {
  const lowerOutputAxes = canonicalImageAxes.map((axis) =>
    axis.id === "output"
      ? {
          ...axis,
          control: { range: { min: 1, max: 4, step: 1, default: 1 } }
        }
      : axis
  );

  expect(
    normalizeAxisSelections(lowerOutputAxes, {
      mode: "1K SD",
      ratio: "1:1",
      output: "9"
    })
  ).toEqual({
    mode: "1K SD",
    ratio: "1:1",
    output: "1"
  });
});

test("canonical video axes use Mode Ratio and Duration labels", () => {
  expect(canonicalVideoAxes.map((axis) => axis.label)).toEqual([
    "Mode",
    "Ratio",
    "Duration",
    "Native audio"
  ]);
  expect(canonicalVideoAxes.map((axis) => axis.label)).not.toContain("Video ratio");
  expect(canonicalVideoAxes.map((axis) => axis.label)).not.toContain("Length");
});

test("malformed controls are invalid so the modal can block saving", () => {
  const malformedAxis = {
    id: "resolution",
    label: "Resolution",
    role: "param",
    control: { enum: { values: [], default: "" } }
  };

  expect(axisControlKind(malformedAxis)).toBe("invalid");
  expect(axisDefaultValue(malformedAxis)).toBeNull();
  expect(axisOptions(malformedAxis)).toEqual([]);
  expect(selectionIsValid(malformedAxis, "720p")).toBe(false);
});

test("malformed axis collections do not crash normalization", () => {
  const badDefaultRangeAxis = {
    id: "duration_seconds",
    label: "Duration",
    role: "param",
    control: { range: { min: 4, max: 12, step: 2, default: 5 } }
  };

  expect(capabilityAxesError(null)).toBe("Capability axes are malformed.");
  expect(normalizeAxisSelections(null, { aspect_ratio: "16:9" })).toEqual({});
  expect(capabilityAxesError([null])).toBe("Capability axis (missing id) is malformed.");
  expect(capabilityAxesError([badDefaultRangeAxis])).toBe(
    "Capability axis duration_seconds is malformed."
  );
  expect(normalizeAxisSelections([badDefaultRangeAxis], { duration_seconds: "8" })).toEqual({});
});
