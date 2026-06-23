<script lang="ts">
  import { onDestroy } from "svelte";
  import { ensureLocalDaemonClient } from "../../api/daemonClient";
  import { browserRecording, type BrowserRecordedFrame } from "../../api/desktop";
  import Icon, { type IconName } from "../../design/Icon.svelte";
  import HighlightedLine from "../../components/HighlightedLine.svelte";
  import { chatFileTarget, fileOpenIntent, type ChatOpenIntent } from "../../chatOpenIntent";
  import type { ToolTimelineItem } from "../../types";
  import InlineCanvas from "./InlineCanvas.svelte";

  type Props = {
    item: ToolTimelineItem;
    sessionId?: string | null;
    defaultCollapsed?: boolean;
    onOpenChatIntent?: (intent: ChatOpenIntent) => void;
    onSubmitCanvasState?: (message: string) => boolean | void | Promise<boolean | void>;
  };
  let {
    item,
    sessionId = null,
    defaultCollapsed = true,
    onOpenChatIntent,
    onSubmitCanvasState
  }: Props = $props();
  type RenderRow = { kind: "ctx" | "add" | "del" | "omit"; line: number | null; text: string };
  type FileRender = { mode: "read" | "diff"; path: string; rows: RenderRow[] };
  type BashRender = { mode: "bash"; command: string; output: string; errOnly: boolean; meta: string[] };
  type ListRender = { mode: "list"; title: string; meta: string[]; rows: string[]; body?: string | null; hint?: string | null };
  type WebRender = { mode: "web"; title: string; meta: string[]; body: string };
  type McpDetail = { label: string; value: string };
  type McpSection = { title: string; rows: string[]; body?: string | null; empty?: string | null };
  type McpRender = { mode: "mcp"; title: string; meta: string[]; details: McpDetail[]; sections: McpSection[]; error?: string | null };
  type ToolRender = FileRender | BashRender | ListRender | WebRender | McpRender;
  type RecordingFrame = BrowserRecordedFrame & { src: string };

  function iconFor(name: string | null | undefined): IconName {
    if (!name) return "bolt";
    const t = name.toLowerCase();
    if (t.includes("edit") || t.includes("write")) return "edit";
    if (t.includes("read") || t.includes("view") || t.includes("image") || t.includes("doc")) return "file";
    if (t.includes("grep") || t.includes("search")) return "search";
    if (t.includes("bash") || t.includes("shell") || t.includes("exec")) return "terminal";
    if (t.includes("browser") || t.includes("fetch") || t.includes("web")) return "globe";
    if (t.includes("git") || t.includes("diff")) return "git";
    if (t.includes("plan") || t.includes("thinking")) return "cpu";
    if (isSubagentToolName(name) || t.includes("mcp__")) return "plug";
    if (t.includes("list") || t.includes("ls")) return "folder";
    return "bolt";
  }

  function argLine(input: string): string {
    const first = input.split("\n").find((l) => l.trim().length > 0) ?? "";
    const trimmed = first.trim();
    if (trimmed.startsWith("{")) {
      try {
        const obj = JSON.parse(input) as Record<string, unknown>;
        const single =
          obj.path ?? obj.file_path ?? obj.pattern ?? obj.command ??
          obj.query ?? obj.url ?? obj.cwd ?? obj.regex ?? obj.prompt ?? null;
        if (typeof single === "string" && single) return single;
        const arr =
          (obj.paths as unknown) ?? (obj.files as unknown) ??
          (obj.globs as unknown) ?? (obj.urls as unknown) ?? null;
        if (Array.isArray(arr) && arr.every((x) => typeof x === "string")) {
          if (arr.length === 0) return "—";
          if (arr.length === 1) return arr[0] as string;
          return `${arr[0]}  +${arr.length - 1}`;
        }
      } catch {
        /* fall through */
      }
    }
    return trimmed;
  }

  function titleCaseAction(value: string | null): string {
    if (!value) return "Action";
    return value
      .split(/[_-]+/)
      .filter(Boolean)
      .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
      .join(" ");
  }

  function smartTitle(value: string | null): string {
    return titleCaseAction(value)
      .replace(/\bMcp\b/g, "MCP")
      .replace(/\bCdp\b/g, "CDP")
      .replace(/\bJson\b/g, "JSON")
      .replace(/\bUrl\b/g, "URL")
      .replace(/\bUri\b/g, "URI")
      .replace(/\bId\b/g, "ID");
  }

  function shortValue(value: string | null, max = 80): string | null {
    if (!value) return null;
    const compact = value.replace(/\s+/g, " ").trim();
    if (!compact) return null;
    return compact.length > max ? `${compact.slice(0, max - 1)}...` : compact;
  }

  function shortUnknown(value: unknown, max = 80): string | null {
    return shortValue(valueText(value), max);
  }

  function openFilePath(path: string) {
    const target = chatFileTarget(path);
    if (!target) return;
    onOpenChatIntent?.(fileOpenIntent(target.path, target.line));
  }

  function browserArgLine(input: Record<string, unknown> | null): string | null {
    const action = stringField(input, ["action"]);
    if (!action) return null;
    const label = titleCaseAction(action);
    const url = shortValue(stringField(input, ["url"]));
    const tabId = shortValue(stringField(input, ["tabId"]));
    const ref = shortValue(stringField(input, ["ref"]));
    const text = shortValue(stringField(input, ["text"]), 48);
    const key = shortValue(stringField(input, ["key"]));
    const script = shortValue(stringField(input, ["script"]), 48);

    switch (action) {
      case "list":
        return "List";
      case "open":
        return url ? `Open ${url}` : "Open";
      case "focus":
        return tabId ? `Focus ${tabId}` : "Focus";
      case "close":
        return tabId ? `Close ${tabId}` : "Close";
      case "navigate":
        return url ? `Navigate ${url}` : "Navigate";
      case "reload":
        return "Reload";
      case "back":
        return "Back";
      case "forward":
        return "Forward";
      case "snapshot":
        return "Snapshot";
      case "click":
        return ref ? `Click ${ref}` : "Click";
      case "type":
        return text ? `Type "${text}"` : "Type";
      case "fill":
        return text ? `Fill "${text}"` : "Fill";
      case "press":
        return key ? `Press ${key}` : "Press";
      case "evaluate":
        return script ? `Evaluate ${script}` : "Evaluate";
      default:
        return label;
    }
  }

  function subAgentArgLine(input: Record<string, unknown> | null): string {
    const tool = stringField(input, ["agent_type", "agentType", "tool", "role"]) ?? "spawn";
    const model = stringField(input, ["model"]);
    const effort = stringField(input, ["reasoningEffort", "reasoning_effort"]);
    const prompt = shortValue(stringField(input, ["message", "prompt"]), 72);
    return [tool, model, effort, prompt].filter(Boolean).join(" · ");
  }

  function statusLabel(s: string): string {
    const n = s.toLowerCase();
    if (n.includes("run") || n === "pending") return "running";
    if (n.includes("err") || n.includes("fail")) return "failed";
    return "done";
  }

  function asRecord(value: unknown): Record<string, unknown> | null {
    return typeof value === "object" && value !== null ? (value as Record<string, unknown>) : null;
  }

  function parseJsonObject(text: string): Record<string, unknown> | null {
    try {
      return asRecord(JSON.parse(text));
    } catch {
      return null;
    }
  }

  function stringField(obj: Record<string, unknown> | null, names: string[]): string | null {
    if (!obj) return null;
    for (const name of names) {
      const value = obj[name];
      if (typeof value === "string" && value.length > 0) return value;
    }
    return null;
  }

  function numberField(obj: Record<string, unknown> | null, names: string[]): number | null {
    if (!obj) return null;
    for (const name of names) {
      const value = obj[name];
      if (typeof value === "number" && Number.isFinite(value)) return value;
    }
    return null;
  }

  function boolField(obj: Record<string, unknown> | null, names: string[]): boolean | null {
    if (!obj) return null;
    for (const name of names) {
      const value = obj[name];
      if (typeof value === "boolean") return value;
    }
    return null;
  }

  function stringArrayField(obj: Record<string, unknown> | null, names: string[]): string[] {
    if (!obj) return [];
    for (const name of names) {
      const value = obj[name];
      if (Array.isArray(value)) return value.filter((item): item is string => typeof item === "string");
    }
    return [];
  }

  function recordField(obj: Record<string, unknown> | null, name: string): Record<string, unknown> | null {
    return asRecord(obj?.[name]);
  }

  function recordArrayField(obj: Record<string, unknown> | null, name: string): Record<string, unknown>[] {
    const value = obj?.[name];
    if (!Array.isArray(value)) return [];
    return value.map(asRecord).filter((item): item is Record<string, unknown> => item !== null);
  }

  function valueText(value: unknown): string {
    if (value === null || value === undefined) return "";
    if (typeof value === "string") return value;
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }

  function parseJsonValue(text: string): unknown | null {
    try {
      return JSON.parse(text) as unknown;
    } catch {
      return null;
    }
  }

  function mcpParts(
    name: string,
    input: Record<string, unknown> | null
  ): { server: string; tool: string } | null {
    const match = /^mcp__(.*?)__(.*)$/.exec(name);
    if (match) return { server: match[1] || "mcp", tool: match[2] || "tool" };
    const server = stringField(input, ["server"]);
    const tool = stringField(input, ["tool"]);
    if (server) return { server, tool: tool ?? "tool" };
    const compactName = name.toLowerCase().replace(/[^a-z0-9]/g, "");
    if (tool && (compactName === "mcp" || compactName === "mcptoolcall")) {
      return { server: "mcp", tool };
    }
    return null;
  }

  function compactToolName(name: string | null | undefined): string {
    return (name ?? "").toLowerCase().replace(/[^a-z0-9]/g, "");
  }

  function isSubagentToolName(name: string | null | undefined): boolean {
    const compact = compactToolName(name);
    return (
      compact === "subagent" ||
      compact === "spawnagent" ||
      compact === "waitagent" ||
      compact === "sendinput" ||
      compact === "closeagent" ||
      compact === "resumeagent" ||
      compact.includes("collab")
    );
  }

  function subagentDisplayToolName(name: string): string {
    const compact = compactToolName(name);
    if (compact === "spawnagent" || compact === "subagent") return "Spawn sub-agent";
    if (compact === "waitagent") return "Wait for sub-agent";
    if (compact === "sendinput") return "Message sub-agent";
    if (compact === "closeagent") return "Close sub-agent";
    if (compact === "resumeagent") return "Resume sub-agent";
    return "Sub-agent";
  }

  function displayToolName(name: string, input: Record<string, unknown> | null): string {
    const mcp = mcpParts(name, input);
    if (mcp) return `${smartTitle(mcp.server)} · ${smartTitle(mcp.tool)}`;
    if (isSubagentToolName(name)) return subagentDisplayToolName(name);
    return name && name !== "undefined" ? name : "Tool";
  }

  function mcpArgLine(name: string, input: Record<string, unknown> | null): string | null {
    const mcp = mcpParts(name, input);
    if (!mcp) return null;
    const resourceUri = shortValue(stringField(input, ["resourceUri", "resource_uri"]));
    if (resourceUri) return resourceUri;
    const args = recordField(input, "arguments");
    if (!args || Object.keys(args).length === 0) return smartTitle(mcp.tool);
    const preferred = stringField(args, [
      "url",
      "uri",
      "path",
      "file",
      "filePath",
      "query",
      "q",
      "pattern",
      "target",
      "ref",
      "action",
      "key"
    ]);
    if (preferred) return preferred;
    const entries = Object.entries(args)
      .filter(([, value]) => value !== null && value !== undefined && value !== "")
      .slice(0, 3)
      .map(([key, value]) => {
        const rendered = shortUnknown(value, 42);
        return rendered ? `${key}: ${rendered}` : key;
      });
    return entries.length ? entries.join(" · ") : smartTitle(mcp.tool);
  }

  function isBrowserToolCall(name: string, input: Record<string, unknown> | null): boolean {
    const lowerName = name.toLowerCase();
    if (lowerName === "browser") return true;
    const mcp = mcpParts(name, input);
    return mcp?.server.toLowerCase() === "browser";
  }

  function browserArgs(input: Record<string, unknown> | null): Record<string, unknown> | null {
    return recordField(input, "arguments") ?? input;
  }

  function outputStatusMeta(output: Record<string, unknown> | null): string[] {
    return [
      stringField(output, ["status"]),
      numberField(output, ["durationMs", "duration_ms"]) !== null
        ? `${numberField(output, ["durationMs", "duration_ms"])}ms`
        : null
    ].filter((value): value is string => Boolean(value));
  }

  function mcpArgumentDetails(input: Record<string, unknown> | null): McpDetail[] {
    const args = recordField(input, "arguments");
    if (!args) return [];
    return Object.entries(args)
      .filter(([, value]) => value !== null && value !== undefined && value !== "")
      .slice(0, 8)
      .map(([label, value]) => ({ label, value: shortUnknown(value, 140) ?? "" }));
  }

  function itemRow(item: unknown, fallbackLabel: string): string {
    const record = asRecord(item);
    if (!record) return valueText(item).replace(/\s+/g, " ").trim();
    const name = stringField(record, ["name", "title", "id", "label"]);
    const uri = stringField(record, ["uri", "url", "uriTemplate", "uri_template", "path"]);
    const description = stringField(record, ["description", "mimeType", "mime_type"]);
    const primary = name ?? uri ?? fallbackLabel;
    return [primary, uri && uri !== primary ? uri : null, description]
      .filter(Boolean)
      .join(" · ");
  }

  function rowsFromArray(value: unknown, fallbackLabel: string): string[] {
    if (!Array.isArray(value)) return [];
    return value.map((item) => itemRow(item, fallbackLabel)).filter(Boolean);
  }

  function mcpSectionsForStructuredValue(tool: string, value: unknown): McpSection[] {
    const record = asRecord(value);
    if (record) {
      const knownSections: Array<{ key: string; title: string; empty: string; fallback: string }> = [
        { key: "resources", title: "Resources", empty: "No resources returned.", fallback: "resource" },
        { key: "resourceTemplates", title: "Resource templates", empty: "No resource templates returned.", fallback: "template" },
        { key: "resource_templates", title: "Resource templates", empty: "No resource templates returned.", fallback: "template" },
        { key: "tools", title: "Tools", empty: "No tools returned.", fallback: "tool" },
        { key: "servers", title: "Servers", empty: "No servers returned.", fallback: "server" },
        { key: "tabs", title: "Tabs", empty: "No tabs returned.", fallback: "tab" },
        { key: "frames", title: "Frames", empty: "No frames returned.", fallback: "frame" }
      ];
      const sections = knownSections
        .filter(({ key }) => Array.isArray(record[key]))
        .map(({ key, title, empty, fallback }) => ({
          title,
          rows: rowsFromArray(record[key], fallback),
          empty
        }));
      if (sections.length > 0) return sections;

      const entries = Object.entries(record)
        .filter(([, item]) => item !== null && item !== undefined)
        .map(([key, item]) => `${key}: ${shortUnknown(item, 180) ?? ""}`);
      if (entries.length > 0 && entries.length <= 12) {
        return [{ title: smartTitle(tool), rows: entries, empty: "No fields returned." }];
      }
    }
    if (Array.isArray(value)) {
      return [{
        title: smartTitle(tool),
        rows: rowsFromArray(value, "item"),
        empty: "No items returned."
      }];
    }
    return [{
      title: smartTitle(tool),
      rows: [],
      body: valueText(value),
      empty: "No result returned."
    }];
  }

  function mcpSectionsForText(tool: string, text: string): McpSection[] {
    const parsed = parseJsonValue(text);
    if (parsed !== null) return mcpSectionsForStructuredValue(tool, parsed);
    return [{
      title: smartTitle(tool),
      rows: [],
      body: text,
      empty: text.trim() ? null : "No text returned."
    }];
  }

  function mcpErrorText(output: Record<string, unknown> | null): string | null {
    const error = recordField(output, "error");
    if (!error) return null;
    return stringField(error, ["message", "error"])
      ?? shortUnknown(error, 500)
      ?? "Tool call failed.";
  }

  function mcpRenderFor(
    name: string,
    input: Record<string, unknown> | null,
    output: Record<string, unknown> | null
  ): McpRender | null {
    const mcp = mcpParts(name, input);
    if (!mcp) return null;
    const title = `${smartTitle(mcp.server)} · ${smartTitle(mcp.tool)}`;
    const meta = [mcp.server, ...outputStatusMeta(output)];
    const details = mcpArgumentDetails(input);
    const error = mcpErrorText(output);
    if (error) {
      return {
        mode: "mcp",
        title,
        meta,
        details,
        error,
        sections: [{ title: "Error", rows: [], body: error }]
      };
    }

    const result = recordField(output, "result");
    const sections: McpSection[] = [];
    const structured = result?.structuredContent ?? result?.structured_content;
    if (structured !== null && structured !== undefined) {
      sections.push(...mcpSectionsForStructuredValue(mcp.tool, structured));
    }
    const content = result?.content;
    if (Array.isArray(content)) {
      for (const item of content) {
        const record = asRecord(item);
        const type = stringField(record, ["type"]) ?? "content";
        const text = stringField(record, ["text"]);
        if (text !== null) {
          sections.push(...mcpSectionsForText(mcp.tool, text));
        } else {
          sections.push({
            title: smartTitle(type),
            rows: [],
            body: valueText(item),
            empty: "No content returned."
          });
        }
      }
    }
    if (sections.length === 0 && result) {
      sections.push(...mcpSectionsForStructuredValue(mcp.tool, result));
    }
    if (sections.length === 0) {
      sections.push({ title: smartTitle(mcp.tool), rows: [], empty: "No result returned." });
    }
    return { mode: "mcp", title, meta, details, sections };
  }

  function compactRows(rows: RenderRow[], limit = 140): RenderRow[] {
    if (rows.length <= limit) return rows;
    const head = Math.floor(limit * 0.65);
    const tail = limit - head;
    return [
      ...rows.slice(0, head),
      { kind: "omit", line: null, text: `${rows.length - limit} unchanged lines omitted` },
      ...rows.slice(rows.length - tail)
    ];
  }

  function readRows(content: string, startLine = 1): RenderRow[] {
    return compactRows(
      content.split("\n").map((line, index) => ({
        kind: "ctx",
        line: startLine + index,
        text: line
      })),
      120
    );
  }

  function diffRows(oldText: string | null, newText: string): RenderRow[] {
    const oldLines = oldText === null ? [] : oldText.split("\n");
    const newLines = newText.split("\n");
    if (oldText === null || oldLines.length === 0) {
      return compactRows(
        newLines.map((line, index) => ({ kind: "add", line: index + 1, text: line })),
        160
      );
    }

    let prefix = 0;
    while (
      prefix < oldLines.length &&
      prefix < newLines.length &&
      oldLines[prefix] === newLines[prefix]
    ) {
      prefix += 1;
    }
    let suffix = 0;
    while (
      suffix < oldLines.length - prefix &&
      suffix < newLines.length - prefix &&
      oldLines[oldLines.length - 1 - suffix] === newLines[newLines.length - 1 - suffix]
    ) {
      suffix += 1;
    }

    const context = 3;
    const start = Math.max(0, prefix - context);
    const oldEnd = Math.min(oldLines.length, oldLines.length - suffix + context);
    const newEnd = Math.min(newLines.length, newLines.length - suffix + context);
    const rows: RenderRow[] = [];
    if (start > 0) {
      rows.push({ kind: "omit", line: null, text: `${start} unchanged lines omitted` });
    }
    for (let i = start; i < prefix; i += 1) {
      rows.push({ kind: "ctx", line: i + 1, text: oldLines[i] });
    }
    for (let i = prefix; i < oldLines.length - suffix; i += 1) {
      rows.push({ kind: "del", line: null, text: oldLines[i] });
    }
    for (let i = prefix; i < newLines.length - suffix; i += 1) {
      rows.push({ kind: "add", line: i + 1, text: newLines[i] });
    }
    for (let i = newLines.length - suffix; i < newEnd; i += 1) {
      rows.push({ kind: "ctx", line: i + 1, text: newLines[i] });
    }
    const omittedTail = newLines.length - newEnd;
    if (omittedTail > 0) {
      rows.push({ kind: "omit", line: null, text: `${omittedTail} unchanged lines omitted` });
    }
    return compactRows(rows);
  }

  function patchRows(diff: string): RenderRow[] {
    return compactRows(
      diff.split("\n").map((line) => {
        if (line.startsWith("+") && !line.startsWith("+++")) return { kind: "add", line: null, text: line.slice(1) };
        if (line.startsWith("-") && !line.startsWith("---")) return { kind: "del", line: null, text: line.slice(1) };
        if (line.startsWith("@@")) return { kind: "omit", line: null, text: line };
        return { kind: "ctx", line: null, text: line.startsWith(" ") ? line.slice(1) : line };
      }),
      180
    );
  }

  function fileRenderFor(
    name: string,
    input: Record<string, unknown> | null,
    output: Record<string, unknown> | null
  ): FileRender | null {
    const normalized = name.toLowerCase();
    const file = recordField(output, "file");
    const path = stringField(input, ["file_path", "path"]) ?? stringField(output, ["filePath", "path"]) ?? stringField(file, ["filePath", "path"]) ?? "";
    if (normalized === "read" || normalized === "read_file") {
      const content = stringField(output, ["content"]) ?? stringField(file, ["content"]) ?? "";
      if (content) {
        return { mode: "read", path, rows: readRows(content, numberField(file, ["startLine"]) ?? 1) };
      }
      if (stringField(output, ["type"]) === "file_unchanged" || file) {
        const message = stringField(output, ["type"]) === "file_unchanged"
          ? "File unchanged since previous read."
          : "Binary or structured file read. Preview is not available in the transcript.";
        return { mode: "read", path, rows: [{ kind: "ctx", line: null, text: message }] };
      }
      return null;
    }
    if (normalized === "write" || normalized === "write_file") {
      const content = stringField(input, ["content", "contents"]) ?? stringField(output, ["content"]) ?? "";
      if (!content) return null;
      const original = stringField(output, ["originalFile", "original_file"]);
      return { mode: "diff", path, rows: diffRows(original, content) };
    }
    if (normalized === "edit" || normalized === "edit_file" || normalized === "replace_in_file") {
      const changes = recordArrayField(output, "changes").length ? recordArrayField(output, "changes") : recordArrayField(input, "changes");
      if (changes.length === 1) {
        const change = changes[0];
        const diff = stringField(change, ["diff", "patch"]);
        if (diff) {
          return {
            mode: "diff",
            path: stringField(change, ["path", "filePath", "file_path"]) ?? path,
            rows: patchRows(diff)
          };
        }
      }
      const oldText = stringField(input, ["old", "old_string", "oldText"]);
      const newText = stringField(input, ["new", "new_string", "newText"]);
      if (oldText === null || newText === null) return null;
      return { mode: "diff", path, rows: diffRows(oldText, newText) };
    }
    if (normalized === "apply_patch" || normalized === "apply_diff") {
      const changes = recordArrayField(output, "changes").length ? recordArrayField(output, "changes") : recordArrayField(input, "changes");
      if (changes.length !== 1) return null;
      const change = changes[0];
      const diff = stringField(change, ["diff", "patch"]) ?? "";
      if (!diff) return null;
      return {
        mode: "diff",
        path: stringField(change, ["path", "filePath", "file_path"]) ?? path,
        rows: patchRows(diff)
      };
    }
    return null;
  }

  function bashRenderFor(
    name: string,
    input: Record<string, unknown> | null,
    output: Record<string, unknown> | null
  ): BashRender | null {
    const normalized = name.toLowerCase();
    if (normalized !== "bash" && normalized !== "powershell" && normalized !== "shell") return null;
    const command = stringField(input, ["command"]) ?? "";
    if (!output) return command ? { mode: "bash", command, output: "", errOnly: false, meta: [] } : null;
    const combined =
      stringField(output, ["aggregatedOutput", "combinedOutput", "combined", "output", "text"]);
    const stdout = stringField(output, ["stdout"]) ?? "";
    const stderr = stringField(output, ["stderr"]) ?? "";
    const meta = [
      boolField(output, ["interrupted"]) ? "interrupted" : null,
      stringField(output, ["backgroundTaskId", "background_task_id"]),
      stringField(output, ["outputFile", "output_file"]),
      boolField(output, ["noOutputExpected", "no_output_expected"]) ? "no output" : null
    ].filter((value): value is string => Boolean(value));
    const text = combined ?? (stdout && stderr ? `${stdout}${stdout.endsWith("\n") ? "" : "\n"}${stderr}` : stdout || stderr);
    if (!command && !text && meta.length === 0) return null;
    return { mode: "bash", command, output: text, errOnly: !stdout && Boolean(stderr), meta };
  }

  function listRenderFor(name: string, input: Record<string, unknown> | null, output: Record<string, unknown> | null): ListRender | null {
    const normalized = name.toLowerCase();
    if (normalized === "toolsearch") {
      return { mode: "list", title: stringField(input, ["query"]) ?? "Tool search", meta: [], rows: [], body: toolOutput };
    }
    if (normalized === "skill") {
      return { mode: "list", title: stringField(input, ["skill"]) ?? "Skill", meta: [], rows: [], body: toolOutput };
    }
    if (!output) return null;
    if (normalized === "glob") {
      const rows = stringArrayField(output, ["filenames"]);
      return {
        mode: "list",
        title: stringField(input, ["pattern"]) ?? "Glob results",
        meta: [`${numberField(output, ["numFiles"]) ?? rows.length} files`, `${numberField(output, ["durationMs"]) ?? 0}ms`],
        rows,
        hint: stringField(output, ["hint"])
      };
    }
    if (normalized === "grep") {
      const mode = stringField(output, ["mode"]) ?? "results";
      const content = stringField(output, ["content"]);
      const rows = content ? content.split("\n").filter(Boolean) : stringArrayField(output, ["filenames"]);
      const meta = [
        mode,
        `${numberField(output, ["numFiles"]) ?? stringArrayField(output, ["filenames"]).length} files`,
        numberField(output, ["numLines"]) !== null ? `${numberField(output, ["numLines"])} lines` : null,
        numberField(output, ["numMatches"]) !== null ? `${numberField(output, ["numMatches"])} matches` : null
      ].filter((value): value is string => Boolean(value));
      return { mode: "list", title: stringField(input, ["pattern"]) ?? "Grep results", meta, rows };
    }
    if (normalized === "apply_patch" || normalized === "apply_diff") {
      const changes = recordArrayField(output, "changes").length ? recordArrayField(output, "changes") : recordArrayField(input, "changes");
      const rows = changes.map((change) => {
        const path = stringField(change, ["path", "filePath", "file_path"]) ?? "unknown file";
        const kind = valueText(change.kind || "update").replace(/\s+/g, " ");
        return `${path} · ${kind}`;
      });
      return {
        mode: "list",
        title: "Patch",
        meta: [`${changes.length} ${changes.length === 1 ? "file" : "files"}`, stringField(output, ["status"]) ?? ""].filter(Boolean),
        rows,
        body: changes.length === 1 ? stringField(changes[0], ["diff", "patch"]) : null
      };
    }
    if (normalized === "plan") {
      const plan = recordArrayField(output, "plan");
      return {
        mode: "list",
        title: "Plan",
        meta: [],
        rows: plan.map((step) => `${stringField(step, ["status"]) ?? "pending"} · ${stringField(step, ["step"]) ?? ""}`),
        body: stringField(input, ["explanation"]) ?? stringField(output, ["explanation"])
      };
    }
    if (normalized === "thinking") {
      const summary = stringArrayField(output, ["summary"]);
      const content = stringArrayField(output, ["content"]);
      return {
        mode: "list",
        title: "Thinking",
        meta: [],
        rows: summary.length ? summary : content,
        body: summary.length || content.length ? null : toolOutput
      };
    }
    if (isSubagentToolName(name)) {
      const states = asRecord(output?.agentsStates);
      const rows = states
        ? Object.entries(states).map(([id, state]) => {
            const stateRecord = asRecord(state);
            const status = stringField(stateRecord, ["status"]) ?? "unknown";
            const message = stringField(stateRecord, ["message"]);
            return message ? `${id} · ${status} — ${message}` : `${id} · ${status}`;
          })
        : stringArrayField(output, ["receiverThreadIds"]);
      return {
        mode: "list",
        title: subagentDisplayToolName(name),
        meta: [
          stringField(input, ["agent_type", "agentType", "tool", "role"]) ?? "",
          stringField(input, ["model"]) ?? "",
          stringField(input, ["reasoningEffort", "reasoning_effort"]) ?? "",
          stringField(output, ["status"]) ?? ""
        ].filter(Boolean),
        rows,
        body: stringField(input, ["message", "prompt"])
      };
    }
    if (normalized === "view_image" || normalized === "image_generation") {
      return {
        mode: "list",
        title: normalized === "view_image" ? "View image" : "Image generation",
        meta: [stringField(output, ["status"]) ?? ""].filter(Boolean),
        rows: [stringField(input, ["path"]) ?? stringField(output, ["savedPath"]) ?? stringField(output, ["result"]) ?? ""].filter(Boolean),
        body: stringField(output, ["revisedPrompt"]) ?? stringField(input, ["prompt"])
      };
    }
    if (output && (output.contentItems || output.result || output.error)) {
      return {
        mode: "list",
        title: name,
        meta: [stringField(output, ["status"]) ?? ""].filter(Boolean),
        rows: [],
        body: valueText(output.contentItems ?? output.result ?? output.error ?? output)
      };
    }
    return null;
  }

  function webRenderFor(name: string, input: Record<string, unknown> | null, output: Record<string, unknown> | null): WebRender | null {
    if (!output) return null;
    const normalized = name.toLowerCase();
    if (normalized !== "webfetch" && normalized !== "websearch" && normalized !== "web_search") return null;
    const body = stringField(output, ["result", "content", "text"]) ?? toolOutput;
    const action = recordField(output, "action") ?? recordField(input, "action");
    const url = stringField(output, ["url"]) ?? stringField(action, ["url", "query", "pattern"]) ?? stringField(input, ["url", "query"]) ?? name;
    const meta = [
      numberField(output, ["code"]) !== null ? `${numberField(output, ["code"])} ${stringField(output, ["codeText"]) ?? ""}`.trim() : null,
      numberField(output, ["bytes"]) !== null ? `${numberField(output, ["bytes"])} bytes` : null,
      numberField(output, ["durationMs"]) !== null ? `${numberField(output, ["durationMs"])}ms` : null
    ].filter((value): value is string => Boolean(value));
    return { mode: "web", title: url, meta, body };
  }

  function renderFor(
    name: string,
    input: Record<string, unknown> | null,
    output: Record<string, unknown> | null
  ): ToolRender | null {
    return fileRenderFor(name, input, output)
      ?? bashRenderFor(name, input, output)
      ?? mcpRenderFor(name, input, output)
      ?? listRenderFor(name, input, output)
      ?? webRenderFor(name, input, output);
  }

  function isCanvasToolCall(name: string, input: Record<string, unknown> | null): boolean {
    const compact = compactToolName(name);
    return compact === "canvas" && Array.isArray(input?.body);
  }

  let toolName = $derived(
    item.toolName && item.toolName !== "undefined" ? item.toolName : "Tool"
  );
  let toolInput = $derived(item.input ?? item.body ?? "");
  let toolOutput = $derived(item.output ?? "");
  let inputJson = $derived(item.inputJson ?? parseJsonObject(toolInput));
  let outputJson = $derived(parseJsonObject(toolOutput));
  let toolDisplayName = $derived(displayToolName(toolName, inputJson));
  let toolRender = $derived(renderFor(toolName, inputJson, outputJson));
  let toolStatus = $derived(item.status ?? "");
  let allLines = $derived(toolOutput.split("\n"));
  let nonEmptyLines = $derived(allLines.filter((l) => l.length > 0));
  let totalLines = $derived(nonEmptyLines.length);
  let hasOutput = $derived(totalLines > 0 || toolRender !== null);
  let isPending = $derived(
    toolStatus.toLowerCase().startsWith("run") || toolStatus === "pending"
  );
  let isTerminalTool = $derived(["bash", "shell", "powershell"].includes(toolName.toLowerCase()));
  let isBrowserTool = $derived(isBrowserToolCall(toolName, inputJson));
  let isCanvasTool = $derived(isCanvasToolCall(toolName, inputJson));
  let canvasId = $derived(stringField(outputJson, ["canvasId", "canvas_id"]));
  let recordingFrames = $state<RecordingFrame[]>([]);
  let selectedFrameId = $state<string | null>(null);
  let recordingDisposer: (() => void) | null = null;
  let recordingKey = "";
  let recordingGeneration = 0;

  function toRecordingFrame(frame: BrowserRecordedFrame): RecordingFrame {
    return {
      ...frame,
      src: `data:${frame.mimeType || "image/jpeg"};base64,${frame.data}`
    };
  }

  function browserFrameStructurallyMatchesArgs(
    args: Record<string, unknown> | null,
    frame: BrowserRecordedFrame
  ): boolean {
    const backendSessionId = stringField(args, ["backendSessionId", "backend_session_id"]);
    const tabId = stringField(args, ["tabId", "tab_id"]);
    if (backendSessionId && frame.backendSessionId !== backendSessionId) return false;
    if (tabId && frame.tabId !== tabId) return false;
    return true;
  }

  function parseBrowserUrl(value: string): URL | null {
    const trimmed = value.trim();
    if (!trimmed) return null;
    try {
      return new URL(trimmed);
    } catch {
      try {
        return new URL(`https://${trimmed}`);
      } catch {
        return null;
      }
    }
  }

  function comparableBrowserHost(url: URL): string {
    return url.hostname.toLowerCase().replace(/^www\./, "");
  }

  function browserHostsCompatible(actionUrl: URL, frameUrl: URL): boolean {
    const actionHost = comparableBrowserHost(actionUrl);
    const frameHost = comparableBrowserHost(frameUrl);
    return (
      actionHost === frameHost ||
      frameHost.endsWith(`.${actionHost}`) ||
      actionHost.endsWith(`.${frameHost}`)
    );
  }

  function browserPathsCompatible(actionUrl: URL, frameUrl: URL): boolean {
    const actionPath = actionUrl.pathname.replace(/\/+$/, "") || "/";
    const framePath = frameUrl.pathname.replace(/\/+$/, "") || "/";
    if (actionPath === "/") return true;
    return framePath === actionPath || framePath.startsWith(`${actionPath}/`);
  }

  function browserSearchAndHashCompatible(actionUrl: URL, frameUrl: URL): boolean {
    if (actionUrl.search && frameUrl.search !== actionUrl.search) return false;
    if (actionUrl.hash && frameUrl.hash !== actionUrl.hash) return false;
    return true;
  }

  function browserUrlsCompatible(actionUrlValue: string, frameUrlValue: string): boolean {
    if (frameUrlValue === actionUrlValue || frameUrlValue.startsWith(`${actionUrlValue}#`)) {
      return true;
    }
    const actionUrl = parseBrowserUrl(actionUrlValue);
    const frameUrl = parseBrowserUrl(frameUrlValue);
    if (!actionUrl || !frameUrl) return false;
    return (
      browserHostsCompatible(actionUrl, frameUrl) &&
      browserPathsCompatible(actionUrl, frameUrl) &&
      browserSearchAndHashCompatible(actionUrl, frameUrl)
    );
  }

  function browserFrameUrlMatchesArgs(
    args: Record<string, unknown> | null,
    frame: BrowserRecordedFrame
  ): boolean {
    const url = stringField(args, ["url"]);
    if (!url) return true;
    return browserUrlsCompatible(url, frame.url);
  }

  function shouldPreferBrowserUrl(args: Record<string, unknown> | null): boolean {
    return Boolean(
      stringField(args, ["url"]) &&
        !stringField(args, ["backendSessionId", "backend_session_id"]) &&
        !stringField(args, ["tabId", "tab_id"])
    );
  }

  function preferBrowserFramesForArgs(
    args: Record<string, unknown> | null,
    frames: BrowserRecordedFrame[]
  ): BrowserRecordedFrame[] {
    const structural = frames.filter((frame) => browserFrameStructurallyMatchesArgs(args, frame));
    if (!shouldPreferBrowserUrl(args)) return structural;
    const urlMatches = structural.filter((frame) => browserFrameUrlMatchesArgs(args, frame));
    if (urlMatches.length > 0) return urlMatches;
    return structural.length <= 1 ? structural : [];
  }

  function browserRecordingKey(): string {
    if (!sessionId || !isBrowserTool) return "";
    const args = browserArgs(inputJson);
    return [
      sessionId,
      item.id,
      toolName,
      stringField(args, ["backendSessionId", "backend_session_id"]) ?? "",
      stringField(args, ["tabId", "tab_id"]) ?? "",
      stringField(args, ["action"]) ?? "",
      stringField(args, ["url"]) ?? ""
    ].join("\u0000");
  }

  function resetBrowserRecording(): void {
    recordingDisposer?.();
    recordingDisposer = null;
    recordingFrames = [];
    selectedFrameId = null;
  }

  function mergeRecordingFrameForArgs(
    args: Record<string, unknown> | null,
    frame: BrowserRecordedFrame,
    expectedKey: string,
    generation: number
  ) {
    if (generation !== recordingGeneration || expectedKey !== recordingKey) return;
    if (!browserFrameStructurallyMatchesArgs(args, frame)) return;
    if (shouldPreferBrowserUrl(args) && !browserFrameUrlMatchesArgs(args, frame)) return;
    const next = toRecordingFrame(frame);
    if (recordingFrames.some((item) => item.frameId === next.frameId)) return;
    recordingFrames = [...recordingFrames, next].slice(-80);
  }

  async function loadBrowserRecordingForAction(
    targetSessionId: string,
    args: Record<string, unknown> | null,
    expectedKey: string,
    generation: number
  ) {
    try {
      const snapshot = await browserRecording(targetSessionId);
      if (generation !== recordingGeneration || expectedKey !== recordingKey) return;
      recordingFrames = preferBrowserFramesForArgs(args, snapshot.frames)
        .map(toRecordingFrame)
        .slice(-80);
    } catch {
      if (generation !== recordingGeneration || expectedKey !== recordingKey) return;
      recordingFrames = [];
    }
  }

  async function subscribeBrowserRecordingForAction(
    targetSessionId: string,
    args: Record<string, unknown> | null,
    expectedKey: string,
    generation: number
  ) {
    const client = await ensureLocalDaemonClient();
    if (generation !== recordingGeneration || expectedKey !== recordingKey) return;
    recordingDisposer?.();
    recordingDisposer = client.on<BrowserRecordedFrame>(
      `browser:${targetSessionId}:recording`,
      (frame) => mergeRecordingFrameForArgs(args, frame, expectedKey, generation)
    );
  }

  $effect(() => {
    const nextKey = browserRecordingKey();
    if (nextKey === recordingKey) return;
    recordingKey = nextKey;
    recordingGeneration += 1;
    resetBrowserRecording();
    if (!nextKey || !sessionId || !isBrowserTool) return;
    const args = browserArgs(inputJson);
    const generation = recordingGeneration;
    void loadBrowserRecordingForAction(sessionId, args, nextKey, generation);
    void subscribeBrowserRecordingForAction(sessionId, args, nextKey, generation);
  });

  onDestroy(() => {
    recordingDisposer?.();
    recordingDisposer = null;
  });

  let selectedFrame = $derived(
    recordingFrames.find((frame) => frame.frameId === selectedFrameId) ?? recordingFrames.at(-1) ?? null
  );

  $effect(() => {
    if (!isBrowserTool) return;
    if (!selectedFrameId && recordingFrames.length > 0) {
      selectedFrameId = recordingFrames.at(-1)?.frameId ?? null;
    }
    if (selectedFrameId && !recordingFrames.some((frame) => frame.frameId === selectedFrameId)) {
      selectedFrameId = recordingFrames.at(-1)?.frameId ?? null;
    }
  });

  // Actions default closed; Canvas is a user-facing widget and opens inline.
  function initialCollapsed(): boolean {
    if (isCanvasTool) return false;
    return defaultCollapsed;
  }

  let collapsed = $state(initialCollapsed());
  let canvasAutoExpanded = $state(false);

  $effect(() => {
    if (isCanvasTool && !canvasAutoExpanded) {
      collapsed = false;
      canvasAutoExpanded = true;
    }
  });

  let visibleLines = $derived(nonEmptyLines);
  let toggleable = $derived(hasOutput || isPending || isBrowserTool || isCanvasTool);

  let arg = $derived(
    isSubagentToolName(toolName)
      ? subAgentArgLine(inputJson)
      : mcpArgLine(toolName, inputJson)
        ?? (isBrowserTool
        ? (browserArgLine(inputJson) ?? "Action")
        : argLine(toolInput))
  );
  let status = $derived(statusLabel(toolStatus));

  function handleHeadClick() {
    if (toggleable) collapsed = !collapsed;
  }
</script>

<div
  class="pf-tool"
  data-collapsed={collapsed}
  data-pending={isPending}
>
  <button
    type="button"
    class="pf-tool-head"
    onclick={handleHeadClick}
    aria-expanded={toggleable ? !collapsed : undefined}
    aria-label={toggleable ? (collapsed ? "Expand tool output" : "Collapse tool output") : undefined}
    disabled={!toggleable}
  >
    <span class="pf-tool-icon"><Icon name={iconFor(toolName)} size={13} /></span>
    <span class="pf-tool-name" title={toolName}>{toolDisplayName}</span>
    <span class="pf-tool-arg" title={arg}>{arg}</span>
    <span class="pf-tool-status" data-state={status}>
      <span class="dot"></span>{status}
    </span>
    {#if toggleable}
      <span class="pf-tool-chevron" aria-hidden="true">
        <Icon name={collapsed ? "chevR" : "chevD"} size={11} />
      </span>
    {/if}
  </button>
  {#if hasOutput && !collapsed}
    <div class:pf-tool-canvas-body={isCanvasTool} class="pf-tool-body">
      {#if isCanvasTool && inputJson}
        <InlineCanvas spec={inputJson} {canvasId} {sessionId} {onSubmitCanvasState} />
      {:else if isBrowserTool}
        <div class="pf-browser-recording-render">
          {#if selectedFrame}
            <figure class="pf-browser-screen">
              <img src={selectedFrame.src} alt={selectedFrame.title || selectedFrame.url || "Browser recording frame"} />
              <figcaption>
                <span>{selectedFrame.title || "Browser"}</span>
                <span>{selectedFrame.url}</span>
              </figcaption>
            </figure>
            <div class="pf-browser-strip" aria-label="Browser screen recording">
              {#each recordingFrames as frame (frame.frameId)}
                <button
                  type="button"
                  class="pf-browser-thumb"
                  class:selected={frame.frameId === selectedFrame.frameId}
                  onclick={() => (selectedFrameId = frame.frameId)}
                  title={frame.title || frame.url}
                >
                  <img src={frame.src} alt="" />
                </button>
              {/each}
            </div>
          {:else}
            <div class="pf-browser-empty">No browser frames recorded for this action yet.</div>
          {/if}
        </div>
      {:else if toolRender?.mode === "read" || toolRender?.mode === "diff"}
        <div class="pf-file-render" data-mode={toolRender.mode}>
          {#if toolRender.path}
            <button
              type="button"
              class="pf-file-path pf-file-path-button"
              title={toolRender.path}
              onclick={() => openFilePath(toolRender.path)}
            >
              {toolRender.path}
            </button>
          {/if}
          <div class="pf-file-lines">
            {#each toolRender.rows as row, i (i)}
              <div class="pf-file-row {row.kind}">
                <span class="gutter">{row.line ?? ""}</span>
                <span class="mark">{row.kind === "add" ? "+" : row.kind === "del" ? "-" : row.kind === "omit" ? "…" : ""}</span>
                <span class="code"><HighlightedLine text={row.text || " "} path={toolRender.path} /></span>
              </div>
            {/each}
          </div>
        </div>
      {:else if toolRender?.mode === "bash"}
        <div class="pf-fake-pty" class:danger={toolRender.errOnly}>
          {#if toolRender.meta.length}
            <div class="pf-pty-meta">{toolRender.meta.join(" · ")}</div>
          {/if}
          <div class="pf-pty-command-line">
            <span class="pf-pty-prompt">$</span>
            <pre>{toolRender.command || arg}</pre>
          </div>
          {#if toolRender.output}
            <pre class="pf-pty-output">{toolRender.output}</pre>
          {:else}
            <div class="pf-pty-empty">(no output)</div>
          {/if}
        </div>
      {:else if toolRender?.mode === "list"}
        <div class="pf-structured-render">
          <div class="pf-render-title">{toolRender.title}</div>
          {#if toolRender.meta.length}
            <div class="pf-render-meta">{toolRender.meta.join(" · ")}</div>
          {/if}
          {#if toolRender.body}
            <pre>{toolRender.body}</pre>
          {/if}
          {#if toolRender.rows.length}
            <div class="pf-result-list">
              {#each toolRender.rows as row, i (i)}
                {@const target = chatFileTarget(row)}
                {#if target}
                  <button
                    type="button"
                    class="pf-result-row pf-result-link"
                    title={row}
                    onclick={() => openFilePath(row)}
                  >
                    {row}
                  </button>
                {:else}
                  <div class="pf-result-row">{row}</div>
                {/if}
              {/each}
            </div>
          {:else if !toolRender.body}
            <div class="pf-render-empty">No results</div>
          {/if}
          {#if toolRender.hint}
            <div class="pf-render-hint">{toolRender.hint}</div>
          {/if}
        </div>
      {:else if toolRender?.mode === "mcp"}
        <div class="pf-structured-render" data-mode="mcp" data-error={Boolean(toolRender.error)}>
          <div class="pf-render-title">{toolRender.title}</div>
          {#if toolRender.meta.length}
            <div class="pf-render-meta">{toolRender.meta.join(" · ")}</div>
          {/if}
          {#if toolRender.details.length}
            <div class="pf-mcp-details">
              {#each toolRender.details as detail (`${detail.label}-${detail.value}`)}
                <div class="pf-mcp-detail">
                  <span>{detail.label}</span>
                  <strong>{detail.value}</strong>
                </div>
              {/each}
            </div>
          {/if}
          {#each toolRender.sections as section, sectionIndex (`${section.title}-${sectionIndex}`)}
            <section class="pf-mcp-section">
              <div class="pf-render-label">{section.title}</div>
              {#if section.body}
                <pre>{section.body}</pre>
              {/if}
              {#if section.rows.length}
                <div class="pf-result-list">
                  {#each section.rows as row, i (i)}
                    {@const target = chatFileTarget(row)}
                    {#if target}
                      <button
                        type="button"
                        class="pf-result-row pf-result-link"
                        title={row}
                        onclick={() => openFilePath(row)}
                      >
                        {row}
                      </button>
                    {:else}
                      <div class="pf-result-row">{row}</div>
                    {/if}
                  {/each}
                </div>
              {:else if !section.body}
                <div class="pf-render-empty">{section.empty ?? "No results"}</div>
              {/if}
            </section>
          {/each}
        </div>
      {:else if toolRender?.mode === "web"}
        <div class="pf-structured-render">
          <div class="pf-render-title">{toolRender.title}</div>
          {#if toolRender.meta.length}
            <div class="pf-render-meta">{toolRender.meta.join(" · ")}</div>
          {/if}
          <pre>{toolRender.body}</pre>
        </div>
      {:else}
        <div class="terminal">
          {#each visibleLines as line, i (i)}
            <div class:dim={line.trim().startsWith("//") || line.trim().startsWith("#")}>{line}</div>
          {/each}
        </div>
      {/if}
    </div>
  {:else if !collapsed && isPending}
    <div class="pf-tool-body pf-tool-pending-body">
      <div class="pf-tool-pending">
        <div class="pf-tool-pending-bar"></div>
        <div class="pf-tool-pending-text">awaiting result…</div>
      </div>
    </div>
  {:else if !collapsed && isBrowserTool}
    <div class="pf-tool-body">
      <div class="pf-browser-recording-render">
        <div class="pf-browser-empty">No browser frames recorded for this action yet.</div>
      </div>
    </div>
  {/if}
</div>

<style>
  .pf-tool-head {
    width: 100%;
    text-align: left;
    background: color-mix(in oklab, var(--muted) 50%, var(--background));
    border: 0;
    font: inherit;
    cursor: pointer;
  }
  .pf-tool-head:disabled { cursor: default; }
  .pf-tool-chevron {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    color: var(--muted-foreground);
    flex-shrink: 0;
    margin-left: 4px;
    transition: transform 120ms;
  }
  .pf-tool-head:hover .pf-tool-chevron {
    color: var(--foreground);
  }
  .pf-tool-more {
    all: unset;
    display: inline-flex;
    margin-top: 4px;
    padding: 2px 8px;
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
    color: oklch(0.7 0.1 145);
    background: transparent;
    cursor: pointer;
    border-radius: 4px;
  }
  .pf-tool-more:hover {
    background: color-mix(in oklab, oklch(0.7 0.1 145) 12%, transparent);
  }
  .pf-file-render {
    background: var(--background);
    border-top: 1px solid var(--border);
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
  }
  .pf-file-path {
    padding: 7px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--muted-foreground);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pf-file-path-button {
    width: 100%;
    border: 0;
    background: transparent;
    font: inherit;
    text-align: left;
    cursor: pointer;
  }
  .pf-file-path-button:hover {
    color: var(--foreground);
    text-decoration: underline;
  }
  .pf-file-lines {
    padding: 6px 0;
    overflow: auto;
  }
  .pf-file-row {
    display: grid;
    grid-template-columns: 48px 18px minmax(0, 1fr);
    min-height: 20px;
    line-height: 20px;
  }
  .pf-file-row .gutter {
    padding-right: 8px;
    text-align: right;
    color: var(--muted-foreground);
    user-select: none;
  }
  .pf-file-row .mark {
    color: var(--muted-foreground);
    user-select: none;
  }
  .pf-file-row .code {
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pf-file-row.add {
    background: color-mix(in oklab, oklch(0.72 0.18 145) 12%, transparent);
  }
  .pf-file-row.add .mark {
    color: oklch(0.48 0.16 145);
  }
  .pf-file-row.del {
    background: color-mix(in oklab, var(--destructive) 10%, transparent);
  }
  .pf-file-row.del .mark {
    color: var(--destructive);
  }
  .pf-file-row.omit {
    color: var(--muted-foreground);
    background: color-mix(in oklab, var(--muted) 45%, transparent);
    font-style: italic;
  }
  .pf-tool-body .terminal {
    border-top: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    padding: 10px 12px 12px;
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
    line-height: 1.45;
    display: flex;
    flex-direction: column;
    gap: 3px;
  }
  .pf-tool-body .terminal :global(.prompt) {
    color: var(--puffer-accent);
  }
  .pf-tool-body .terminal :global(.err) {
    color: var(--destructive);
  }
  .pf-tool-body .terminal .dim {
    color: var(--muted-foreground);
  }
  .pf-structured-render {
    background: var(--background);
    border-top: 1px solid var(--border);
    padding: 10px 12px;
    font-family: var(--font-sans);
    font-size: var(--pf-chat-detail-size);
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .pf-fake-pty {
    border-top: 1px solid var(--border);
    background: var(--background);
    color: var(--foreground);
    padding: 10px 12px 12px;
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
    line-height: 1.45;
  }
  .pf-fake-pty.danger {
    box-shadow: inset 3px 0 0 color-mix(in oklab, var(--destructive) 70%, transparent);
  }
  .pf-pty-meta {
    margin-bottom: 7px;
    color: var(--muted-foreground);
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
  }
  .pf-pty-command-line {
    display: grid;
    grid-template-columns: 18px minmax(0, 1fr);
    gap: 8px;
    align-items: start;
    margin-bottom: 8px;
  }
  .pf-pty-prompt {
    color: var(--puffer-accent);
    user-select: none;
  }
  .pf-pty-command-line pre,
  .pf-pty-output {
    margin: 0;
    white-space: pre-wrap;
    overflow: auto;
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
  }
  .pf-pty-command-line pre {
    color: var(--foreground);
  }
  .pf-pty-output {
    padding-left: 26px;
    color: color-mix(in oklab, var(--foreground) 84%, var(--muted-foreground));
  }
  .pf-pty-empty {
    padding-left: 26px;
    color: var(--muted-foreground);
    font-style: italic;
  }
  .pf-render-title {
    font-family: var(--font-sans);
    font-size: var(--pf-chat-detail-size);
    font-weight: 600;
    color: var(--foreground);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .pf-render-meta,
  .pf-render-label,
  .pf-render-empty,
  .pf-render-hint {
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
    color: var(--muted-foreground);
  }
  .pf-render-section {
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .pf-render-section.danger {
    border-color: color-mix(in oklab, var(--destructive) 28%, var(--border));
  }
  .pf-structured-render[data-error="true"] {
    border-top-color: color-mix(in oklab, var(--destructive) 35%, var(--border));
  }
  .pf-structured-render[data-error="true"] .pf-render-title {
    color: var(--destructive);
  }
  .pf-render-label {
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--muted) 40%, transparent);
  }
  .pf-structured-render pre {
    margin: 0;
    padding: 8px;
    white-space: pre-wrap;
    overflow: auto;
    line-height: 1.45;
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
  }
  .pf-result-list {
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .pf-result-row {
    display: block;
    width: 100%;
    padding: 5px 8px;
    white-space: pre;
    overflow: hidden;
    text-overflow: ellipsis;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 70%, transparent);
    font-family: var(--font-mono);
    font-size: var(--pf-chat-code-size);
    color: inherit;
    text-align: left;
    background: transparent;
    border-left: 0;
    border-right: 0;
    border-top: 0;
  }
  .pf-result-row:last-child {
    border-bottom: 0;
  }
  .pf-result-link {
    cursor: pointer;
    color: var(--accent);
    text-decoration: underline;
    text-underline-offset: 0.16em;
  }
  .pf-result-link:hover {
    color: var(--foreground);
  }
  .pf-mcp-details {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(150px, 1fr));
    gap: 6px;
  }
  .pf-mcp-detail {
    min-width: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 6px 8px;
    background: color-mix(in oklab, var(--muted) 30%, transparent);
  }
  .pf-mcp-detail span {
    display: block;
    margin-bottom: 3px;
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
    color: var(--muted-foreground);
    text-transform: uppercase;
    letter-spacing: 0.05em;
  }
  .pf-mcp-detail strong {
    display: block;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--foreground);
    font-family: var(--font-sans);
    font-size: var(--pf-chat-detail-size);
    font-weight: 600;
  }
  .pf-mcp-section {
    display: grid;
    gap: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
  }
  .pf-mcp-section .pf-result-list {
    border: 0;
    border-radius: 0;
  }
  .pf-mcp-section .pf-render-empty {
    padding: 8px;
  }
  .pf-browser-recording-render {
    background: var(--background);
    border-top: 1px solid var(--border);
    padding: 10px;
    display: grid;
    gap: 8px;
  }
  .pf-browser-screen {
    margin: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    overflow: hidden;
    background: var(--background);
  }
  .pf-browser-screen img {
    width: 100%;
    max-height: 260px;
    display: block;
    object-fit: contain;
    background: #fff;
  }
  .pf-browser-screen figcaption {
    display: grid;
    gap: 3px;
    padding: 7px 9px;
    border-top: 1px solid var(--border);
    font-family: var(--font-sans);
    font-size: var(--pf-chat-meta-size);
  }
  .pf-browser-screen figcaption span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pf-browser-screen figcaption span:first-child {
    font-weight: 600;
    color: var(--foreground);
  }
  .pf-browser-screen figcaption span:last-child {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
  }
  .pf-browser-strip {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    padding-bottom: 2px;
  }
  .pf-browser-thumb {
    width: 88px;
    height: 54px;
    flex: 0 0 auto;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 2px;
    background: var(--background);
    cursor: pointer;
    overflow: hidden;
  }
  .pf-browser-thumb img {
    width: 100%;
    height: 100%;
    display: block;
    object-fit: cover;
  }
  .pf-browser-thumb.selected {
    border-color: var(--puffer-accent);
    box-shadow: 0 0 0 2px color-mix(in oklab, var(--puffer-accent) 20%, transparent);
  }
  .pf-browser-empty {
    min-height: 120px;
    display: grid;
    place-items: center;
    color: var(--muted-foreground);
    font-family: var(--font-sans);
    font-size: var(--pf-chat-detail-size);
    text-align: center;
  }
  .pf-tool-pending-body {
    background: var(--background);
    padding: 0;
  }
  .pf-tool-pending {
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 10px 12px;
    font-family: var(--font-sans);
  }
  .pf-tool-pending-bar {
    width: 68px;
    height: 6px;
    border-radius: 999px;
    flex-shrink: 0;
    background: linear-gradient(
      90deg,
      color-mix(in oklab, var(--muted-foreground) 12%, var(--muted)) 0%,
      color-mix(in oklab, var(--puffer-accent) 28%, var(--muted)) 50%,
      color-mix(in oklab, var(--muted-foreground) 12%, var(--muted)) 100%
    );
    background-size: 200% 100%;
    animation: pf-shimmer 1.4s linear infinite;
  }
  .pf-tool-pending-text {
    color: var(--muted-foreground);
    font-size: var(--pf-chat-meta-size);
    font-style: normal;
    opacity: 1;
  }
  @keyframes pf-shimmer {
    0% { background-position: 200% 0; }
    100% { background-position: -200% 0; }
  }
</style>
