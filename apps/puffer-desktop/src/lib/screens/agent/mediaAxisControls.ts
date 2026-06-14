export type AxisControlKind = "enum" | "range" | "bool" | "invalid";

type AxisControlInput = { control?: unknown } | null | undefined;

type EnumControl = { enum: { values: string[]; default: string } };
type RangeControl = { range: { min: number; max: number; step: number; default: number } };
type BoolControl = { bool: { default: boolean } };

export function axisControlKind(axis: AxisControlInput): AxisControlKind {
  if (enumControl(axis)) return "enum";
  if (rangeControl(axis)) return "range";
  if (boolControl(axis)) return "bool";
  return "invalid";
}

export function axisOptions(axis: AxisControlInput): string[] {
  const control = enumControl(axis);
  return control ? [...control.enum.values] : [];
}

export function axisDefaultValue(axis: AxisControlInput): string | null {
  const enumValue = enumControl(axis);
  if (enumValue) return enumValue.enum.default;
  const rangeValue = rangeControl(axis);
  if (rangeValue) return String(rangeValue.range.default);
  const boolValue = boolControl(axis);
  if (boolValue) return boolValue.bool.default ? "true" : "false";
  return null;
}

export function selectionIsValid(
  axis: AxisControlInput,
  value: string | undefined
): value is string {
  if (value === undefined) return false;
  const enumValue = enumControl(axis);
  if (enumValue) return enumValue.enum.values.includes(value);
  const boolValue = boolControl(axis);
  if (boolValue) return value === "true" || value === "false";
  const rangeValue = rangeControl(axis);
  if (!rangeValue) return false;
  const numeric = Number(value);
  const { min, max, step } = rangeValue.range;
  if (!Number.isFinite(numeric) || numeric < min || numeric > max) return false;
  return rangeStepContains(min, step, numeric);
}

export function normalizeAxisSelections(
  axes: unknown,
  saved: Record<string, string>
): Record<string, string> {
  if (!Array.isArray(axes)) return {};
  const next: Record<string, string> = {};
  for (const axis of axes) {
    const id = axisId(axis);
    if (!id) continue;
    if (selectionIsValid(axis, saved[id])) {
      next[id] = saved[id];
      continue;
    }
    const defaultValue = axisDefaultValue(axis);
    if (defaultValue !== null && selectionIsValid(axis, defaultValue)) {
      next[id] = defaultValue;
    }
  }
  return next;
}

export function capabilityAxesError(axes: unknown): string | null {
  if (!Array.isArray(axes)) return "Capability axes are malformed.";
  for (const axis of axes) {
    const id = axisId(axis);
    if (!id || axisControlKind(axis) === "invalid") {
      return `Capability axis ${id || "(missing id)"} is malformed.`;
    }
  }
  return null;
}

function enumControl(axis: AxisControlInput): EnumControl | null {
  const control = controlRecord(axis);
  const enumValue = control?.enum;
  if (!isRecord(enumValue)) return null;
  const values = enumValue.values;
  const defaultValue = enumValue.default;
  if (!Array.isArray(values) || values.length === 0) return null;
  if (!values.every((value) => typeof value === "string" && value.length > 0)) {
    return null;
  }
  if (typeof defaultValue !== "string" || !values.includes(defaultValue)) {
    return null;
  }
  return { enum: { values, default: defaultValue } };
}

function rangeControl(axis: AxisControlInput): RangeControl | null {
  const control = controlRecord(axis);
  const rangeValue = control?.range;
  if (!isRecord(rangeValue)) return null;
  const { min, max, step, default: defaultValue } = rangeValue;
  if (
    !isFiniteNumber(min) ||
    !isFiniteNumber(max) ||
    !isFiniteNumber(step) ||
    !isFiniteNumber(defaultValue)
  ) {
    return null;
  }
  if (max < min || step <= 0 || defaultValue < min || defaultValue > max) return null;
  if (!rangeStepContains(min, step, defaultValue)) return null;
  return { range: { min, max, step, default: defaultValue } };
}

function boolControl(axis: AxisControlInput): BoolControl | null {
  const control = controlRecord(axis);
  const boolValue = control?.bool;
  if (!isRecord(boolValue) || typeof boolValue.default !== "boolean") return null;
  return { bool: { default: boolValue.default } };
}

function axisId(axis: unknown): string | null {
  if (!isRecord(axis)) return null;
  return typeof axis.id === "string" && axis.id.length > 0 ? axis.id : null;
}

function controlRecord(axis: AxisControlInput): Record<string, unknown> | null {
  if (!isRecord(axis)) return null;
  return isRecord(axis.control) ? axis.control : null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function isFiniteNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value);
}

function rangeStepContains(min: number, step: number, value: number): boolean {
  const offset = (value - min) / step;
  return Math.abs(offset - Math.round(offset)) < 1e-9;
}
