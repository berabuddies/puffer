export function buildReplayRegistryMetadata(testCase, supportedActionIds) {
  const supported = new Set(supportedActionIds);
  const actions = [];
  const invariants = new Map();
  const unsupported = [];

  for (const step of testCase.steps ?? []) {
    if (step.phase === "assert") {
      collectInvariant(invariants, step.target ?? step.action, step);
      continue;
    }
    const descriptor = describeAction(step, supported);
    actions.push(descriptor);
    if (!descriptor.supported) unsupported.push(descriptor.id);
    for (const assertion of step.assertions ?? []) collectInvariant(invariants, assertion, step);
    for (const tag of step.coverage ?? []) {
      if (String(tag).startsWith("invariant:")) collectInvariant(invariants, tag.slice("invariant:".length), step);
    }
  }

  return {
    actionRegistryVersion: 1,
    invariantRegistryVersion: 1,
    actions,
    invariants: [...invariants.values()].sort((left, right) => left.id.localeCompare(right.id)),
    unsupportedActions: [...new Set(unsupported)].sort()
  };
}

export function assertReplayRegistrySupported(registryMetadata) {
  const unsupported = registryMetadata.unsupportedActions ?? [];
  if (unsupported.length > 0) {
    throw new Error(`Unsupported replay action(s): ${unsupported.join(", ")}`);
  }
}

function describeAction(step, supported) {
  const id = step.action ?? step.id ?? "";
  return {
    id,
    kind: step.kind ?? "",
    target: step.target ?? "",
    phase: step.phase ?? "",
    supported: supported.has(id),
    paramsSchema: inferParamsSchema(step.params ?? {}),
    guard: {
      requires: [...(step.requires ?? [])],
      blocks: [...(step.blocks ?? [])]
    },
    resources: {
      produces: [...(step.produces ?? [])],
      consumes: [...(step.consumes ?? [])],
      invalidates: [...(step.invalidates ?? [])]
    },
    coverage: [...(step.coverage ?? [])],
    shrinkHints: shrinkHintsForStep(step)
  };
}

function collectInvariant(invariants, id, step) {
  if (!id) return;
  const key = String(id).replace(/^invariant:/, "");
  const previous = invariants.get(key) ?? {
    id: key,
    preconditions: [],
    coverage: [],
    failureSignature: key,
    severityHint: "P2"
  };
  invariants.set(key, {
    ...previous,
    preconditions: [...new Set([...previous.preconditions, ...(step.requires ?? [])])].sort(),
    coverage: [...new Set([...previous.coverage, ...(step.coverage ?? [])])].sort(),
    severityHint: severityHintForInvariant(key)
  });
}

function inferParamsSchema(params) {
  return Object.fromEntries(
    Object.entries(params ?? {}).map(([key, value]) => [key, {
      type: Array.isArray(value) ? "array" : value === null ? "null" : typeof value,
      required: true
    }])
  );
}

function shrinkHintsForStep(step) {
  const hints = [];
  if ((step.params && Object.keys(step.params).length > 0)) hints.push("simplify-params");
  if ((step.coverage ?? []).some((tag) => String(tag).startsWith("async:"))) hints.push("minimize-async-schedule");
  if (step.phase === "fuzz") hints.push("try-delete-step");
  if ((step.produces ?? []).length > 0) hints.push("preserve-dependency-closure");
  return hints;
}

function severityHintForInvariant(id) {
  if (["one-request-per-intent", "active-session-stable", "no-cross-provider-model"].includes(id)) return "P1";
  if (["draft-preserved-on-failure", "stale-error-scoped", "no-permanent-loading"].includes(id)) return "P1";
  return "P2";
}
