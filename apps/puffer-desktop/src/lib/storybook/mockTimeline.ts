import type { DiffTimelineItem, PermissionTimelineItem, ToolTimelineItem } from "../types";
import { mockSessionDetail } from "../mockData";

export const storyToolItem = mockSessionDetail.timeline.find(
  (item): item is ToolTimelineItem => item.kind === "tool"
)!;

export const storyDiffItem = mockSessionDetail.timeline.find(
  (item): item is DiffTimelineItem => item.kind === "diff"
)!;

export const storyPermissionItem = mockSessionDetail.timeline.find(
  (item): item is PermissionTimelineItem => item.kind === "permission"
)!;

export const storyMarkdown = [
  "# Release checklist",
  "",
  "Puffer renders **markdown**, inline `code`, links like [settings](/Users/yuna/.puffer/config.toml:2), and task lists.",
  "",
  "- [x] Add Storybook shell",
  "- [x] Cover core components",
  "- [ ] Wire visual regression",
  "",
  "| Area | Status |",
  "| --- | --- |",
  "| Auth | Ready |",
  "| Sessions | In review |",
  "",
  "> Keep stories focused on one state at a time.",
  "",
  "```ts",
  "const provider = 'worldrouter';",
  "```"
].join("\n");
