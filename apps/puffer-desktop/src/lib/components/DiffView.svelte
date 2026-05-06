<script lang="ts">
  import type { DiffSnapshot } from "../types";
  import HighlightedLine from "./HighlightedLine.svelte";

  type PatchLine = {
    kind: "context" | "added" | "removed";
    text: string;
    oldNumber: number | null;
    newNumber: number | null;
  };

  type Hunk = {
    header: string;
    oldStart: number;
    newStart: number;
    lines: PatchLine[];
  };

  type DiffFile = {
    path: string;
    oldPath: string;
    status: "added" | "removed" | "modified";
    added: number;
    removed: number;
    meta: string[];
    hunks: Hunk[];
  };

  export let diff: DiffSnapshot;
  export let compact = false;

  let activePath = "";
  let closedFiles: Record<string, boolean> = {};

  function cleanPath(value: string): string {
    return value.replace(/^a\//, "").replace(/^b\//, "").trim();
  }

  function fileId(path: string, index: number): string {
    return `diff-file-${index}-${path.replace(/[^a-zA-Z0-9_-]/g, "-")}`;
  }

  function parseHeaderPath(line: string): string | null {
    const value = line.slice(4).trim().split(/\s+/)[0];
    if (!value || value === "/dev/null") return value;
    return cleanPath(value);
  }

  function parseDiff(text: string): DiffFile[] {
    const files: DiffFile[] = [];
    let current: DiffFile | null = null;
    let hunk: Hunk | null = null;
    let oldNumber = 0;
    let newNumber = 0;

    const ensureFile = () => {
      if (!current) {
        current = {
          path: diff.title || "session.diff",
          oldPath: diff.title || "session.diff",
          status: "modified",
          added: 0,
          removed: 0,
          meta: [],
          hunks: []
        };
        files.push(current);
      }
      return current;
    };

    const startFile = (path: string, oldPath = path): DiffFile => {
      const file: DiffFile = {
        path,
        oldPath,
        status: "modified",
        added: 0,
        removed: 0,
        meta: [],
        hunks: []
      };
      current = file;
      files.push(file);
      hunk = null;
      return file;
    };

    for (const raw of text.split("\n")) {
      if (raw.startsWith("diff --git ")) {
        const match = /^diff --git a\/(.+?) b\/(.+)$/.exec(raw);
        const file = startFile(match ? cleanPath(match[2]) : raw.replace("diff --git ", ""));
        file.meta.push(raw);
        continue;
      }

      if (raw.startsWith("--- ")) {
        const file = ensureFile();
        const oldPath = parseHeaderPath(raw);
        file.oldPath = oldPath && oldPath !== "/dev/null" ? oldPath : file.oldPath;
        if (oldPath === "/dev/null") file.status = "added";
        file.meta.push(raw);
        continue;
      }

      if (raw.startsWith("+++ ")) {
        const file = ensureFile();
        const newPath = parseHeaderPath(raw);
        if (newPath && newPath !== "/dev/null") file.path = newPath;
        if (newPath === "/dev/null") file.status = "removed";
        file.meta.push(raw);
        continue;
      }

      if (raw.startsWith("@@")) {
        const file = ensureFile();
        const match = /@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)/.exec(raw);
        oldNumber = match ? Number(match[1]) : oldNumber;
        newNumber = match ? Number(match[2]) : newNumber;
        hunk = {
          header: raw,
          oldStart: oldNumber,
          newStart: newNumber,
          lines: []
        };
        file.hunks.push(hunk);
        continue;
      }

      if (!hunk) {
        ensureFile().meta.push(raw);
        continue;
      }

      const file = ensureFile();
      if (raw.startsWith("+")) {
        hunk.lines.push({ kind: "added", text: raw.slice(1) || " ", oldNumber: null, newNumber });
        file.added += 1;
        newNumber += 1;
      } else if (raw.startsWith("-")) {
        hunk.lines.push({ kind: "removed", text: raw.slice(1) || " ", oldNumber, newNumber: null });
        file.removed += 1;
        oldNumber += 1;
      } else {
        hunk.lines.push({
          kind: "context",
          text: raw.startsWith(" ") ? raw.slice(1) || " " : raw || " ",
          oldNumber,
          newNumber
        });
        oldNumber += 1;
        newNumber += 1;
      }
    }

    return files.filter((file) => file.hunks.length > 0 || file.added || file.removed || file.meta.length > 0);
  }

  function totals(files: DiffFile[]) {
    return files.reduce(
      (sum, file) => ({
        added: sum.added + file.added,
        removed: sum.removed + file.removed
      }),
      { added: 0, removed: 0 }
    );
  }

  function dirname(path: string): string {
    const parts = path.split("/");
    parts.pop();
    return parts.join("/") || ".";
  }

  function basename(path: string): string {
    return path.split("/").pop() || path;
  }

  function barStyle(file: DiffFile): string {
    const total = Math.max(1, file.added + file.removed);
    return `--add:${(file.added / total) * 100}%; --del:${(file.removed / total) * 100}%`;
  }

  function scrollToFile(event: MouseEvent, path: string, index: number) {
    event.preventDefault();
    activePath = path;

    const targetId = fileId(path, index);
    history.replaceState(null, "", `#${targetId}`);

    requestAnimationFrame(() => {
      const target = document.getElementById(targetId);
      const scroller = target?.closest<HTMLElement>(".diff-wrap");
      if (!target || !scroller) return;

      const delta = target.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
      scroller.scrollTop += delta - 8;
    });
  }

  function toggleFile(path: string) {
    closedFiles = { ...closedFiles, [path]: closedFiles[path] !== true };
  }

  $: files = parseDiff(diff.patch);
  $: stat = totals(files);
  $: if ((!activePath || !files.some((file) => file.path === activePath)) && files.length > 0) {
    activePath = files[0].path;
  }
  $: visibleFiles = compact ? files.slice(0, 3) : files;
  $: openFiles = Object.fromEntries(files.map((file) => [file.path, closedFiles[file.path] !== true]));
</script>

<article class:compact class="diff-view">
  <div class="diff-layout">
    <aside class="file-tree" aria-label="Changed files">
      <div class="tree-head">
        <span>Changed files</span>
        <span>{files.length}</span>
      </div>
      <div class="tree-total" style={`--add:${Math.max(1, stat.added + stat.removed) ? (stat.added / Math.max(1, stat.added + stat.removed)) * 100 : 0}%; --del:${Math.max(1, stat.added + stat.removed) ? (stat.removed / Math.max(1, stat.added + stat.removed)) * 100 : 0}%`}>
        <span class="add">+{stat.added}</span>
        <span class="del">-{stat.removed}</span>
        <span class="mini-bar"></span>
      </div>
      <div class="tree-scroll">
        {#each files as file, index (file.path)}
          <a
            href={`#${fileId(file.path, index)}`}
            class="tree-row"
            class:active={activePath === file.path}
            onclick={(event) => scrollToFile(event, file.path, index)}
          >
            <span class="file-dot" data-status={file.status}></span>
            <span class="tree-path">
              <span class="dir">{dirname(file.path)}/</span>
              <strong>{basename(file.path)}</strong>
            </span>
            <span class="tree-stats">
              <span class="add">+{file.added}</span>
              <span class="del">-{file.removed}</span>
            </span>
            <span class="mini-bar" style={barStyle(file)}></span>
          </a>
        {/each}
      </div>
    </aside>

    <div class="diff-files">
      {#each visibleFiles as file, index (file.path)}
        <section class="file-card" id={fileId(file.path, index)}>
          <button type="button" class="file-head" onclick={() => toggleFile(file.path)}>
            <span class="chevron" class:open={openFiles[file.path]} aria-hidden="true"></span>
            <span class="file-dot" data-status={file.status}></span>
            <span class="file-name">
              <span>{dirname(file.path)}/</span>{basename(file.path)}
            </span>
            {#if file.status !== "modified"}
              <span class="status-chip">{file.status}</span>
            {/if}
            <span class="chip add">+{file.added}</span>
            <span class="chip del">-{file.removed}</span>
            <span class="mini-bar" style={barStyle(file)}></span>
          </button>

          {#if openFiles[file.path]}
            {#each file.hunks as hunk, hunkIndex (hunk.header + hunkIndex)}
              <div class="hunk">
                <div class="hunk-head">
                  <span class="mono">{hunk.header}</span>
                </div>
                <div class="patch-lines">
                  {#each hunk.lines as line, lineIndex (lineIndex)}
                    <div class={"patch-line " + line.kind}>
                      <span class="gutter">{line.oldNumber ?? ""}</span>
                      <span class="gutter">{line.newNumber ?? ""}</span>
                      <span class="marker">{line.kind === "added" ? "+" : line.kind === "removed" ? "-" : ""}</span>
                      <code><HighlightedLine text={line.text} path={file.path} /></code>
                    </div>
                  {/each}
                </div>
              </div>
            {/each}
            {#if file.hunks.length === 0}
              <div class="file-empty">No renderable hunks for this file.</div>
            {/if}
          {/if}
        </section>
      {/each}
      {#if compact && files.length > visibleFiles.length}
        <p class="truncation-note">Showing {visibleFiles.length} of {files.length} files.</p>
      {/if}
    </div>
  </div>
</article>

<style>
  .diff-view {
    --diff-add-bg: oklch(0.96 0.05 145);
    --diff-add-bg-strong: oklch(0.92 0.1 145);
    --diff-add-fg: oklch(0.42 0.13 145);
    --diff-add-marker: oklch(0.55 0.16 145);
    --diff-del-bg: oklch(0.96 0.04 25);
    --diff-del-bg-strong: oklch(0.92 0.1 25);
    --diff-del-fg: oklch(0.48 0.18 25);
    --diff-del-marker: oklch(0.62 0.2 25);
    --diff-hunk-bg: color-mix(in oklab, oklch(0.97 0.02 254) 78%, var(--background));
    min-height: 100%;
    background: var(--background);
    color: var(--foreground);
  }

  .eyebrow {
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0;
  }

  p {
    margin: 2px 0 0;
    color: var(--muted-foreground);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .mono {
    font-family: var(--font-mono);
  }

  .chip,
  .status-chip {
    height: 22px;
    display: inline-flex;
    align-items: center;
    border-radius: 999px;
    padding: 0 8px;
    background: var(--muted);
    color: var(--muted-foreground);
    white-space: nowrap;
  }

  .add { color: var(--diff-add-fg) !important; }
  .del { color: var(--diff-del-fg) !important; }
  .chip.add { background: color-mix(in oklab, var(--diff-add-marker) 13%, transparent); }
  .chip.del { background: color-mix(in oklab, var(--diff-del-marker) 13%, transparent); }

  .diff-layout {
    display: grid;
    grid-template-columns: minmax(240px, 300px) minmax(0, 1fr);
    align-items: start;
  }

  .file-tree {
    border-right: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 95%, var(--muted));
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr);
    position: sticky;
    top: 0;
    align-self: start;
    max-height: 100vh;
  }

  .tree-head,
  .tree-total {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 9px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--muted-foreground);
    font-size: 11px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0;
  }

  .tree-total {
    justify-content: flex-start;
    text-transform: none;
    font-family: var(--font-mono);
  }

  .tree-scroll {
    padding: 8px 6px;
    overflow: auto;
  }

  .tree-row {
    width: 100%;
    min-width: 0;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto auto;
    align-items: center;
    gap: 7px;
    padding: 6px 7px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    text-align: left;
    text-decoration: none;
    cursor: pointer;
    font: inherit;
  }

  .tree-row:hover,
  .tree-row.active {
    background: var(--accent);
  }

  .file-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--muted-foreground);
    flex: 0 0 auto;
  }

  .file-dot[data-status="added"] { background: var(--diff-add-marker); }
  .file-dot[data-status="removed"] { background: var(--diff-del-marker); }

  .tree-path,
  .file-name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-size: 12px;
  }

  .tree-path .dir,
  .file-name span {
    color: var(--muted-foreground);
  }

  .tree-stats {
    display: inline-flex;
    gap: 5px;
    font-family: var(--font-mono);
    font-size: 10.5px;
  }

  .mini-bar {
    width: 42px;
    height: 6px;
    border-radius: 2px;
    overflow: hidden;
    background:
      linear-gradient(
        90deg,
        var(--diff-add-marker) 0 var(--add),
        var(--diff-del-marker) var(--add) calc(var(--add) + var(--del)),
        var(--border) calc(var(--add) + var(--del)) 100%
      );
    flex: 0 0 auto;
  }

  .diff-files {
    padding: 14px 16px 18px;
    display: flex;
    flex-direction: column;
    gap: 14px;
    background: var(--background);
    min-width: 0;
    overflow: visible;
  }

  .file-card {
    border: 1px solid var(--border);
    border-radius: 8px;
    overflow: hidden;
    background: var(--background);
    scroll-margin-top: 12px;
  }

  .file-head {
    width: 100%;
    min-width: 0;
    display: grid;
    grid-template-columns: auto auto minmax(0, 1fr) auto auto auto auto;
    align-items: center;
    gap: 9px;
    padding: 9px 12px;
    border: 0;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    text-align: left;
  }

  .chevron {
    width: 0;
    height: 0;
    border-top: 4px solid transparent;
    border-bottom: 4px solid transparent;
    border-left: 5px solid currentColor;
    color: var(--muted-foreground);
    transition: transform 120ms;
  }

  .chevron.open {
    transform: rotate(90deg);
  }

  .status-chip {
    border: 1px solid var(--border);
    background: transparent;
    font-size: 10.5px;
    text-transform: uppercase;
  }

  .hunk + .hunk {
    border-top: 1px solid var(--border);
  }

  .hunk-head {
    width: 100%;
    display: block;
    align-items: center;
    padding: 5px 12px;
    color: color-mix(in oklab, oklch(0.42 0.1 254) 80%, var(--foreground));
    background: var(--diff-hunk-bg);
    text-align: left;
    font-size: 11px;
  }

  .patch-lines {
    overflow-x: auto;
  }

  .patch-line {
    display: grid;
    grid-template-columns: 42px 42px 18px minmax(max-content, 1fr);
    min-width: max-content;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 20px;
    white-space: pre;
  }

  .patch-line.context {
    background: var(--background);
  }

  .patch-line.added {
    background: var(--diff-add-bg);
  }

  .patch-line.removed {
    background: var(--diff-del-bg);
  }

  .gutter {
    padding: 0 8px;
    text-align: right;
    color: var(--muted-foreground);
    user-select: none;
    border-right: 1px solid color-mix(in oklab, var(--border) 70%, transparent);
  }

  .marker {
    text-align: center;
    color: var(--muted-foreground);
    font-weight: 600;
    user-select: none;
  }

  .patch-line.added .marker { color: var(--diff-add-marker); }
  .patch-line.removed .marker { color: var(--diff-del-marker); }

  code {
    padding-right: 14px;
  }

  .file-empty,
  .truncation-note {
    padding: 18px;
    text-align: center;
    color: var(--muted-foreground);
    font-size: 12px;
  }

  .diff-view.compact .file-tree {
    display: none;
  }

  .diff-view.compact .diff-layout {
    grid-template-columns: 1fr;
  }

  .diff-view.compact .diff-files {
    padding: 8px;
  }

  @media (max-width: 900px) {
    .diff-layout {
      grid-template-columns: 1fr;
    }

    .file-tree {
      display: none;
    }
  }
</style>
