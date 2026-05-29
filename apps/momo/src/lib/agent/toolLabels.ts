import type { IconName } from "./components/Icon.svelte";

export interface ToolLabel {
  icon: IconName;
  label: string;
}

const TOOL_LABELS: Record<string, ToolLabel> = {
  read_file: { icon: "file", label: "正在读取文件…" },
  read: { icon: "file", label: "正在读取文件…" },
  write_file: { icon: "edit", label: "正在写入文件…" },
  write: { icon: "edit", label: "正在写入文件…" },
  edit: { icon: "edit", label: "正在编辑文件…" },
  edit_file: { icon: "edit", label: "正在编辑文件…" },
  apply_patch: { icon: "edit", label: "正在修改文件…" },
  apply_diff: { icon: "edit", label: "正在修改文件…" },
  bash: { icon: "terminal", label: "正在执行命令…" },
  shell: { icon: "terminal", label: "正在执行命令…" },
  grep: { icon: "search", label: "正在搜索…" },
  glob: { icon: "search", label: "正在查找文件…" },
  skill: { icon: "sparkles", label: "正在调用技能…" },
  websearch: { icon: "globe", label: "正在联网搜索…" },
  web_search: { icon: "globe", label: "正在联网搜索…" },
  webfetch: { icon: "globe", label: "正在抓取网页…" }
};

/** 嗅探 bash 命令前缀,把通用 shell 伪装成高层动作(沿用旧 momo)。 */
export function lookupToolLabel(toolId: string, input?: unknown): ToolLabel | null {
  const id = toolId.toLowerCase();
  if ((id === "bash" || id === "shell") && typeof input === "object" && input) {
    const cmd = String((input as Record<string, unknown>).command ?? "");
    if (cmd.startsWith("telegram ")) return { icon: "message-circle", label: "正在使用 Telegram…" };
    if (cmd.startsWith("email ")) return { icon: "mail", label: "正在处理邮件…" };
  }
  return TOOL_LABELS[id] ?? null;
}
