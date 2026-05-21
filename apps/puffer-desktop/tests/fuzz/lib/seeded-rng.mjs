export function hashString(input) {
  let hash = 2166136261;
  for (let index = 0; index < input.length; index += 1) {
    hash ^= input.charCodeAt(index);
    hash = Math.imul(hash, 16777619);
  }
  return hash >>> 0;
}

export function createRng(seed) {
  let state = hashString(String(seed));
  return function next() {
    state += 0x6d2b79f5;
    let value = state;
    value = Math.imul(value ^ (value >>> 15), value | 1);
    value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
    return ((value ^ (value >>> 14)) >>> 0) / 4294967296;
  };
}

export function randomInt(rng, min, max) {
  return Math.floor(rng() * (max - min + 1)) + min;
}

export function pick(rng, values) {
  if (!Array.isArray(values) || values.length === 0) {
    throw new Error("Cannot pick from an empty list");
  }
  return values[randomInt(rng, 0, values.length - 1)];
}

export function weightedPick(rng, values) {
  const total = values.reduce((sum, value) => sum + Math.max(0, Number(value.weight ?? 1)), 0);
  if (total <= 0) {
    return pick(rng, values);
  }
  let cursor = rng() * total;
  for (const value of values) {
    cursor -= Math.max(0, Number(value.weight ?? 1));
    if (cursor <= 0) return value;
  }
  return values[values.length - 1];
}
