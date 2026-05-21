<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import HighlightedLine from "../../components/HighlightedLine.svelte";
  import Icon from "../../design/Icon.svelte";
  import PdfDocumentPreview from "./PdfDocumentPreview.svelte";
  import {
    buildFilePreview,
    hasRichFilePreview,
    hasRichFilePreviewPath,
    type FilePreview
  } from "./filePreview";
  import {
    fsUnwatch,
    fsWatch,
    isDaemonReachable,
    listDir,
    loadFileTabs,
    lspInspect,
    readFile,
    saveFileTabs,
    writeFile,
    type DirEntry,
    type FileTabsState,
    type FsChangedEvent,
    type LspInspectResult,
    type ReadFileResult
  } from "../../api/desktop";
  import { ensureLocalDaemonClient } from "../../api/daemonClient";

  type Props = {
    cwd: string;
    sessionId?: string;
    openPath?: string | null;
    openLine?: number | null;
    openRequestId?: number | null;
  };
  let {
    cwd,
    sessionId = "preview",
    openPath = null,
    openLine = null,
    openRequestId = null
  }: Props = $props();

  type OpenFileTab = {
    path: string;
    name: string;
    size: number;
    pinned: boolean;
  };

  type OpenFileOptions = {
    pinned?: boolean;
    line?: number | null;
    character?: number | null;
  };

  const RICH_PREVIEW_MAX_BYTES = 24 * 1024 * 1024;

  // Directory cache: absolute path → its (already-loaded) entries. Keeps
  // the tree interactions snappy across expand/collapse cycles and lets
  // us distinguish "still loading" (no key) from "empty dir" (key with
  // zero entries).
  let cache = $state<Map<string, DirEntry[]>>(new Map());
  let expanded = $state<Set<string>>(new Set());
  let loading = $state<Set<string>>(new Set());
  let errors = $state<Map<string, string>>(new Map());

  // Active right-pane state. Open tabs mirror VS Code's preview behavior:
  // an unpinned preview tab is replaceable until edited, saved, or pinned.
  let openTabs = $state<OpenFileTab[]>([]);
  let fileCache = $state<Map<string, ReadFileResult>>(new Map());
  let draftCache = $state<Map<string, string>>(new Map());
  let activePath = $state<string | null>(null);
  let activeSize = $state<number>(0);
  let activeFile = $state<ReadFileResult | null>(null);
  let activeLoading = $state(false);
  let activeError = $state<string | null>(null);
  let draftContent = $state("");
  let activePreview = $state<FilePreview | null>(null);
  let activePreviewLoading = $state(false);
  let activePreviewError = $state<string | null>(null);
  let viewerMode = $state<"preview" | "raw">("preview");
  let saving = $state(false);
  let saveError = $state<string | null>(null);
  let selectedSymbol = $state<string | null>(null);
  let lspLoading = $state(false);
  let lspError = $state<string | null>(null);
  let lspResult = $state<LspInspectResult | null>(null);
  let lspAnchor = $state<{ line: number; character: number } | null>(null);
  let fileReadGeneration = 0;
  let filePreviewGeneration = 0;
  let lspInspectGeneration = 0;
  let editorEl = $state<HTMLTextAreaElement | null>(null);
  let editorGutterEl = $state<HTMLDivElement | null>(null);
  let editorHighlightEl = $state<HTMLPreElement | null>(null);
  let fileTabsReady = $state(false);
  let fileTabsSaveTimer: ReturnType<typeof setTimeout> | null = null;
  let lastOpenRequestKey: string | null = null;

  // Root is derived from cwd — switching sessions resets everything.
  let root = $derived(cwd);

  // Web preview has no daemon, so file listing / reading RPCs can't
  // succeed. Render a preview-mode notice instead of a red error.
  const previewMode = !isDaemonReachable();

  $effect(() => {
    // Reset whenever cwd changes. We mutate state inside `untrack` so
    // Svelte doesn't treat our own writes as reactive dependencies of
    // this effect — otherwise setting `loading` / `cache` from
    // loadDir's synchronous prelude loops back into the effect.
    const next = root;
    const nextSessionId = sessionId;
    if (!next || previewMode) return;
    untrack(() => {
      fileTabsReady = false;
      cache = new Map();
      expanded = new Set([next]);
      loading = new Set();
      errors = new Map();
      openTabs = [];
      fileCache = new Map();
      draftCache = new Map();
      activePath = null;
      activeFile = null;
      activeError = null;
      activeSize = 0;
      clearActivePreview();
      viewerMode = "preview";
      draftContent = "";
      saving = false;
      saveError = null;
      fileReadGeneration += 1;
      clearLspState();
      void loadDir(next);
      void restoreFileTabs(next, nextSessionId);
    });
  });

  $effect(() => {
    openTabs;
    activePath;
    if (!fileTabsReady || previewMode || sessionId === "preview") return;
    scheduleFileTabsSave();
  });

  $effect(() => {
    const path = openPath;
    const requestKey = openRequestId == null ? `${path ?? ""}:${openLine ?? ""}` : String(openRequestId);
    if (!path || requestKey === lastOpenRequestKey || !root || previewMode) return;
    lastOpenRequestKey = requestKey;
    void revealAndOpenFile(path, openLine);
  });

  // Filesystem-watcher lifecycle. When `cwd` is set, ask the daemon to watch
  // it recursively and subscribe to `workspace:fs:changed`. When cwd changes
  // or the component unmounts, unhook and tear down the watch. The daemon's
  // replay machinery will re-fire historical events on reconnect flagged with
  // `replay: true`; we ignore those because a freshly-mounted pane hasn't
  // cached anything yet, so there's nothing stale to refresh.
  //
  // The watch is owned by this effect so it automatically tears down + re-
  // creates when `root` changes (session switch). onDestroy handles the final
  // unmount.
  let currentWatchId: string | null = null;
  let fsEventUnsubscribe: (() => void) | null = null;
  let destroyed = false;

  function createFsWatchId(): string {
    const random =
      globalThis.crypto?.randomUUID?.() ??
      `${Date.now()}-${Math.random().toString(36).slice(2)}`;
    return `files-${random}`;
  }

  async function rebuildWatch(target: string) {
    // Tear down whatever's left from the previous watch root first.
    teardownWatch();
    if (destroyed || !target) return;

    const expectedWatchId = createFsWatchId();
    currentWatchId = expectedWatchId;
    try {
      const client = await ensureLocalDaemonClient();
      const listener = (payload: FsChangedEvent) => {
        if (!payload) return;
        // Replay events describe mutations that happened before this pane
        // existed — nothing to invalidate. The cache is already fresh from
        // the latest listDir on mount.
        if (payload.replay) return;
        if (payload.watchId !== currentWatchId) return;
        handleFsChanged(payload.paths ?? []);
      };
      fsEventUnsubscribe = client.on<FsChangedEvent>("workspace:fs:changed", listener);

      // Start the actual watch. If the pane was unmounted / rerooted while
      // this await was pending, unwatch immediately to avoid leaking.
      const { watchId } = await fsWatch([target], true, expectedWatchId);
      if (destroyed || target !== root || currentWatchId !== expectedWatchId) {
        await fsUnwatch(watchId).catch(() => {
          /* best-effort */
        });
        return;
      }
      currentWatchId = watchId;
    } catch (_err) {
      if (currentWatchId === expectedWatchId) currentWatchId = null;
      fsEventUnsubscribe?.();
      fsEventUnsubscribe = null;
      // Failing to install the watcher isn't fatal — the pane still works,
      // just without auto-refresh. Don't spam the user with a toast; the
      // cache fallback (expand/collapse) still works.
      console.warn("fsWatch failed; FilesPane will not auto-refresh", _err);
    }
  }

  function teardownWatch() {
    fsEventUnsubscribe?.();
    fsEventUnsubscribe = null;
    const id = currentWatchId;
    currentWatchId = null;
    if (id) {
      void fsUnwatch(id).catch(() => {
        /* ignore — the daemon might be gone already */
      });
    }
  }

  $effect(() => {
    const target = root;
    if (!target || previewMode) return;
    void rebuildWatch(target);
  });

  onDestroy(() => {
    destroyed = true;
    if (fileTabsSaveTimer) clearTimeout(fileTabsSaveTimer);
    void teardownWatch();
  });

  /** Invalidate cached directories that contain any changed path + kick off
   *  a re-fetch for the ones currently expanded, so the tree reflects reality
   *  without collapsing. Also reloads the active right-pane file if it was
   *  in the changed set. */
  function handleFsChanged(changed: string[]) {
    if (!changed || changed.length === 0) return;

    // Collect the set of cached directory keys that need invalidation. For
    // each changed path, walk up its parents: any parent that's currently
    // cached lists this path as one of its entries. We have to be careful
    // to handle both direct parents (for creates/deletes) AND the changed
    // path itself if it IS a directory (for descendant changes — some
    // backends coalesce an intermediate directory mtime bump).
    const toInvalidate = new Set<string>();
    for (const p of changed) {
      if (!p) continue;
      // Normalise trailing slashes.
      const norm = p.endsWith("/") && p.length > 1 ? p.slice(0, -1) : p;
      // Walk up through parents until we leave the root. Each ancestor that
      // we have cached needs to be refreshed — the change might be a new
      // file in that ancestor or (for recursive backends like FSEvents) an
      // mtime bump on that ancestor's own entry list.
      let current = norm;
      while (current && current.length >= root.length) {
        if (cache.has(current)) toInvalidate.add(current);
        const parent = parentPath(current);
        if (parent === current) break;
        current = parent;
      }
    }

    if (toInvalidate.size === 0 && activePath && changed.includes(activePath)) {
      // Active file changed but we don't have its containing dir cached —
      // just reload the file contents.
      void reloadActiveFile();
      return;
    }
    if (toInvalidate.size === 0) return;

    // Refresh each invalidated directory. If the directory is currently
    // expanded, re-fetch and overwrite; otherwise just evict from cache
    // so the next expand picks up fresh data.
    for (const dir of toInvalidate) {
      if (expanded.has(dir)) {
        void refreshDir(dir);
      } else {
        const next = new Map(cache);
        next.delete(dir);
        cache = next;
      }
    }

    if (activePath && changed.includes(activePath)) {
      void reloadActiveFile();
    }
  }

  async function refreshDir(path: string) {
    // Refresh in place: re-listDir and merge into the cache without
    // touching the `loading` set (we don't want a spinner on every
    // passive update — that would flicker during an agent edit burst).
    try {
      const entries = await listDir(path);
      const nextCache = new Map(cache);
      nextCache.set(path, entries);
      cache = nextCache;
      // If the directory used to error out but now loads, clear the error.
      if (errors.has(path)) {
        const nextErrors = new Map(errors);
        nextErrors.delete(path);
        errors = nextErrors;
      }
    } catch (err) {
      // On refresh error (dir removed), evict the cache entry so the tree
      // stops rendering stale children. Don't surface the error loudly —
      // it'll resurface naturally if the user tries to expand again.
      const nextCache = new Map(cache);
      nextCache.delete(path);
      cache = nextCache;
      void err;
    }
  }

  async function reloadActiveFile() {
    const target = activePath;
    if (!target) return;
    const wasDirty = isTabDirty(target);
    const expectedRoot = root;
    const expectedSessionId = sessionId;
    const generation = fileReadGeneration;
    try {
      const result = await readPreviewFile(target);
      if (
        generation === fileReadGeneration &&
        activePath === target &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) {
        cacheFileResult(result, !wasDirty, target);
      }
    } catch (err) {
      if (
        generation === fileReadGeneration &&
        activePath === target &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) {
        activeError = err instanceof Error ? err.message : String(err);
      }
    }
  }

  async function restoreFileTabs(expectedRoot: string, expectedSessionId: string) {
    if (!expectedSessionId || expectedSessionId === "preview") {
      fileTabsReady = true;
      return;
    }
    try {
      const state = await loadFileTabs(expectedSessionId);
      if (destroyed || root !== expectedRoot || sessionId !== expectedSessionId) return;
      const restoredTabs = state.tabs.map((tab) => tabFor(tab.path, 0, tab.pinned));
      openTabs = restoredTabs;
      const restoredActive =
        state.activePath && restoredTabs.some((tab) => tab.path === state.activePath)
          ? state.activePath
          : restoredTabs[0]?.path;
      if (restoredActive) {
        await activateFile(restoredActive, restoredTabs.find((tab) => tab.path === restoredActive)?.size);
      }
    } catch (err) {
      console.warn("failed to restore Files tabs", err);
    } finally {
      if (!destroyed && root === expectedRoot && sessionId === expectedSessionId) {
        fileTabsReady = true;
      }
    }
  }

  function fileTabsState(): FileTabsState {
    return {
      tabs: openTabs.map((tab) => ({
        path: tab.path,
        pinned: tab.pinned
      })),
      activePath
    };
  }

  function scheduleFileTabsSave() {
    if (fileTabsSaveTimer) clearTimeout(fileTabsSaveTimer);
    const targetSessionId = sessionId;
    const snapshot = fileTabsState();
    fileTabsSaveTimer = setTimeout(() => {
      fileTabsSaveTimer = null;
      void persistFileTabs(targetSessionId, snapshot);
    }, 200);
  }

  async function persistFileTabs(targetSessionId: string, state: FileTabsState) {
    if (previewMode || targetSessionId === "preview") return;
    try {
      await saveFileTabs(targetSessionId, state);
    } catch (err) {
      console.warn("failed to persist Files tabs", err);
    }
  }

  function parentPath(p: string): string {
    if (!p) return p;
    const idx = p.lastIndexOf("/");
    if (idx <= 0) return "/";
    return p.slice(0, idx);
  }

  function fileName(path: string): string {
    return path.split("/").pop() || path;
  }

  function fileTabPathLabel(path: string): string {
    if (root && path.startsWith(`${root}/`)) return path.slice(root.length + 1);
    return path || "file";
  }

  function closeFileTabLabel(path: string): string {
    return `Close ${fileTabPathLabel(path)}`;
  }

  function readPreviewFile(path: string): Promise<ReadFileResult> {
    return readFile(path, hasRichFilePreviewPath(path) ? RICH_PREVIEW_MAX_BYTES : undefined);
  }

  function tabFor(path: string, size: number, pinned: boolean): OpenFileTab {
    return {
      path,
      name: fileName(path),
      size,
      pinned
    };
  }

  function isTabDirty(path: string): boolean {
    const file = fileCache.get(path);
    if (!file || file.encoding !== "utf8") return false;
    const draft = draftCache.get(path);
    return draft != null && draft !== file.content;
  }

  function pinTab(path: string) {
    const next = openTabs.map((tab) => (tab.path === path ? { ...tab, pinned: true } : tab));
    openTabs = next;
  }

  function cacheFileResult(
    result: ReadFileResult,
    resetDraft: boolean,
    logicalPath = result.path
  ) {
    const cachedResult = result.path === logicalPath ? result : { ...result, path: logicalPath };
    const nextFiles = new Map(fileCache);
    nextFiles.set(logicalPath, cachedResult);
    fileCache = nextFiles;

    if (resetDraft || !draftCache.has(logicalPath)) {
      const nextDrafts = new Map(draftCache);
      nextDrafts.set(logicalPath, cachedResult.encoding === "utf8" ? cachedResult.content : "");
      draftCache = nextDrafts;
    }

    openTabs = openTabs.map((tab) =>
      tab.path === logicalPath ? { ...tab, size: cachedResult.size } : tab
    );

    if (activePath === logicalPath) {
      activeFile = cachedResult;
      activeSize = cachedResult.size;
      activeError = null;
      draftContent = draftCache.get(logicalPath) ?? (cachedResult.encoding === "utf8" ? cachedResult.content : "");
      void loadActivePreview(cachedResult);
    }
  }

  function clearActivePreview() {
    filePreviewGeneration += 1;
    activePreview = null;
    activePreviewLoading = false;
    activePreviewError = null;
  }

  async function loadActivePreview(result: ReadFileResult) {
    if (!hasRichFilePreview(result)) {
      clearActivePreview();
      return;
    }
    const expectedPath = result.path;
    const expectedRoot = root;
    const expectedSessionId = sessionId;
    const generation = ++filePreviewGeneration;
    activePreview = null;
    activePreviewError = null;
    activePreviewLoading = true;
    try {
      const preview = await buildFilePreview(result);
      if (
        generation !== filePreviewGeneration ||
        activePath !== expectedPath ||
        root !== expectedRoot ||
        sessionId !== expectedSessionId
      ) return;
      activePreview = preview;
    } catch (err) {
      if (
        generation !== filePreviewGeneration ||
        activePath !== expectedPath ||
        root !== expectedRoot ||
        sessionId !== expectedSessionId
      ) return;
      activePreviewError = err instanceof Error ? err.message : String(err);
    } finally {
      if (
        generation === filePreviewGeneration &&
        activePath === expectedPath &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) {
        activePreviewLoading = false;
      }
    }
  }

  function rememberDraft(path: string, content: string) {
    const next = new Map(draftCache);
    next.set(path, content);
    draftCache = next;
  }

  function setDraft(content: string) {
    draftContent = content;
    if (!activePath) return;
    rememberDraft(activePath, content);
    if (isTabDirty(activePath)) pinTab(activePath);
  }

  async function loadDir(path: string) {
    if (cache.has(path) || loading.has(path)) return;
    const nextLoading = new Set(loading);
    nextLoading.add(path);
    loading = nextLoading;
    try {
      const entries = await listDir(path);
      const nextCache = new Map(cache);
      nextCache.set(path, entries);
      cache = nextCache;
      const nextErrors = new Map(errors);
      nextErrors.delete(path);
      errors = nextErrors;
    } catch (err) {
      const nextErrors = new Map(errors);
      nextErrors.set(path, err instanceof Error ? err.message : String(err));
      errors = nextErrors;
    } finally {
      const next = new Set(loading);
      next.delete(path);
      loading = next;
    }
  }

  function joinPath(parent: string, name: string): string {
    if (parent.endsWith("/")) return `${parent}${name}`;
    return `${parent}/${name}`;
  }

  function toggleDir(path: string) {
    const next = new Set(expanded);
    if (next.has(path)) {
      next.delete(path);
    } else {
      next.add(path);
      if (!cache.has(path)) void loadDir(path);
    }
    expanded = next;
  }

  async function revealAndOpenFile(path: string, line: number | null = null) {
    const nextExpanded = new Set(expanded);
    const relative = path.startsWith(`${root}/`) ? path.slice(root.length + 1) : "";
    if (relative) {
      let current = root;
      for (const segment of relative.split("/").slice(0, -1)) {
        current = joinPath(current, segment);
        nextExpanded.add(current);
        if (!cache.has(current)) void loadDir(current);
      }
      expanded = nextExpanded;
    }
    await openFile(path, 0, { pinned: true, line });
  }

  async function openFile(path: string, size: number, options: OpenFileOptions = {}) {
    const pinned = options.pinned ?? false;
    const existingIndex = openTabs.findIndex((tab) => tab.path === path);
    const previewIndex = openTabs.findIndex((tab) => !tab.pinned && !isTabDirty(tab.path));
    const nextTab = tabFor(path, size, pinned);

    if (existingIndex >= 0) {
      openTabs = openTabs.map((tab) =>
        tab.path === path ? { ...tab, pinned: tab.pinned || pinned, size } : tab
      );
    } else if (!pinned && previewIndex >= 0) {
      const replaced = openTabs[previewIndex];
      const nextTabs = [...openTabs];
      nextTabs[previewIndex] = nextTab;
      openTabs = nextTabs;
      if (replaced) {
        const nextFiles = new Map(fileCache);
        nextFiles.delete(replaced.path);
        fileCache = nextFiles;
        const nextDrafts = new Map(draftCache);
        nextDrafts.delete(replaced.path);
        draftCache = nextDrafts;
      }
    } else {
      openTabs = [...openTabs, nextTab];
    }

    await activateFile(path, size, options.line, options.character);
  }

  async function activateFile(
    path: string,
    size?: number,
    line: number | null = null,
    character: number | null = null
  ) {
    const expectedRoot = root;
    const expectedSessionId = sessionId;
    const generation = fileReadGeneration;
    activePath = path;
    activeSize = size ?? openTabs.find((tab) => tab.path === path)?.size ?? 0;
    activeError = null;
    saveError = null;
    clearActivePreview();
    viewerMode = "preview";
    clearLspState();

    const cached = fileCache.get(path);
    if (cached) {
      activeFile = cached;
      activeSize = cached.size;
      draftContent = draftCache.get(path) ?? (cached.encoding === "utf8" ? cached.content : "");
      activeLoading = false;
      void loadActivePreview(cached);
      await focusEditorLine(path, line, character);
      return;
    }

    activeFile = null;
    draftContent = "";
    activeLoading = true;
    let loaded = false;
    try {
      const result = await readPreviewFile(path);
      if (
        generation === fileReadGeneration &&
        activePath === path &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) {
        cacheFileResult(result, false, path);
        loaded = true;
      }
    } catch (err) {
      if (
        generation === fileReadGeneration &&
        activePath === path &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) {
        activeError = err instanceof Error ? err.message : String(err);
      }
    } finally {
      if (
        generation === fileReadGeneration &&
        activePath === path &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) activeLoading = false;
    }
    if (loaded) await focusEditorLine(path, line, character);
  }

  async function closeTab(event: Event, path: string) {
    event.stopPropagation();
    if (isTabDirty(path) && !window.confirm(`Discard unsaved changes to ${fileName(path)}?`)) {
      return;
    }

    const closingIndex = openTabs.findIndex((tab) => tab.path === path);
    const nextTabs = openTabs.filter((tab) => tab.path !== path);
    openTabs = nextTabs;

    const nextFiles = new Map(fileCache);
    nextFiles.delete(path);
    fileCache = nextFiles;
    const nextDrafts = new Map(draftCache);
    nextDrafts.delete(path);
    draftCache = nextDrafts;

    if (activePath !== path) return;
    const nextActive = nextTabs[Math.min(closingIndex, nextTabs.length - 1)] ?? nextTabs[nextTabs.length - 1];
    if (nextActive) {
      await activateFile(nextActive.path, nextActive.size);
    } else {
      activePath = null;
      activeSize = 0;
      activeFile = null;
      activeLoading = false;
      activeError = null;
      clearActivePreview();
      viewerMode = "preview";
      draftContent = "";
      saveError = null;
      clearLspState();
    }
  }

  function handleFileTabKeydown(event: KeyboardEvent, path: string, size: number) {
    if (event.target !== event.currentTarget) return;
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    void activateFile(path, size);
  }

  function fmtSize(bytes: number): string {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
  }

  function splitLines(content: string): string[] {
    // No trailing empty line for files that end with a newline; the
    // editor view already renders the final "\n" as the row terminator.
    const trimmed = content.endsWith("\n") ? content.slice(0, -1) : content;
    return trimmed.split("\n");
  }

  let viewerLines = $derived(
    activeFile && activeFile.encoding === "utf8" ? splitLines(activeFile.content) : []
  );
  let draftLineNumbers = $derived(
    Array.from({ length: Math.max(1, draftContent.split("\n").length) }, (_, index) => index + 1)
  );
  let draftLines = $derived(splitLines(draftContent));
  let referenceSections = $derived(lspResult ? buildReferenceSections(lspResult) : []);
  let definitionLocations = $derived(lspResult ? parseDefinitionLocations(lspResult) : []);
  let hoverText = $derived(lspResult?.operations.hover?.result ?? "");
  let lspFallbackText = $derived(
    lspResult
      ? Object.values(lspResult.operations)
          .map((op) => op.result)
          .filter(Boolean)
          .join("\n\n")
      : ""
  );
  let canEdit = $derived(
    !!activeFile &&
      activeFile.encoding === "utf8" &&
      !activeFile.truncated &&
      !activeLoading
  );
  let hasPreview = $derived(!!activeFile && hasRichFilePreview(activeFile));
  let canToggleRaw = $derived(canEdit && hasPreview);
  let showRawEditor = $derived(canEdit && (!hasPreview || viewerMode === "raw"));
  let dirty = $derived(activePath ? isTabDirty(activePath) : false);

  function cancelEditing() {
    draftContent = activeFile?.encoding === "utf8" ? activeFile.content : "";
    if (activePath) rememberDraft(activePath, draftContent);
    saveError = null;
  }

  async function saveEditing() {
    const target = activePath;
    if (!target || !dirty || saving) return;
    const expectedRoot = root;
    const expectedSessionId = sessionId;
    const generation = fileReadGeneration;
    const content = draftContent;
    saving = true;
    saveError = null;
    try {
      const result = await writeFile(target, content);
      if (
        generation !== fileReadGeneration ||
        root !== expectedRoot ||
        sessionId !== expectedSessionId
      ) {
        return;
      }
      cacheFileResult(result, true, target);
      pinTab(target);
      if (activePath === target) {
        clearLspState();
      }
      void refreshDir(parentPath(target));
    } catch (err) {
      if (
        generation === fileReadGeneration &&
        root === expectedRoot &&
        sessionId === expectedSessionId &&
        activePath === target
      ) {
        saveError = err instanceof Error ? err.message : String(err);
      }
    } finally {
      if (
        generation === fileReadGeneration &&
        root === expectedRoot &&
        sessionId === expectedSessionId
      ) {
        saving = false;
      }
    }
  }

  function handleEditorKeydown(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
      event.preventDefault();
      void saveEditing();
    }
    if (event.key === "Escape" && !dirty) {
      event.preventDefault();
      clearLspState();
    }
  }

  function syncEditorOverlays(target: HTMLTextAreaElement) {
    if (editorGutterEl) editorGutterEl.scrollTop = target.scrollTop;
    if (editorHighlightEl) {
      editorHighlightEl.scrollTop = target.scrollTop;
      editorHighlightEl.scrollLeft = target.scrollLeft;
    }
  }

  function syncEditorScroll(event: Event) {
    const target = event.currentTarget as HTMLTextAreaElement;
    syncEditorOverlays(target);
  }

  function offsetForLine(content: string, line: number): number {
    const targetLine = Math.max(1, Math.floor(line));
    let offset = 0;
    for (let current = 1; current < targetLine; current += 1) {
      const next = content.indexOf("\n", offset);
      if (next === -1) return content.length;
      offset = next + 1;
    }
    return Math.min(offset, content.length);
  }

  function offsetForLocation(content: string, line: number, character: number | null | undefined): number {
    const lineOffset = offsetForLine(content, line);
    if (!character || !Number.isFinite(character) || character < 1) return lineOffset;
    const lineEnd = content.indexOf("\n", lineOffset);
    const maxOffset = lineEnd === -1 ? content.length : lineEnd;
    return Math.min(lineOffset + Math.floor(character) - 1, maxOffset);
  }

  async function focusEditorLine(
    path: string,
    line: number | null | undefined,
    character: number | null | undefined = null
  ) {
    if (!line || !Number.isFinite(line) || line < 1) return;
    await tick();
    if (activePath !== path || !canEdit || !editorEl) return;
    const offset = offsetForLocation(draftContent, line, character);
    editorEl.focus();
    editorEl.setSelectionRange(offset, offset);
    const style = getComputedStyle(editorEl);
    const lineHeight = Number.parseFloat(style.lineHeight) || 18;
    editorEl.scrollTop = Math.max(0, (Math.floor(line) - 1) * lineHeight - editorEl.clientHeight / 3);
    syncEditorOverlays(editorEl);
  }

  function clearLspState() {
    lspInspectGeneration += 1;
    selectedSymbol = null;
    lspLoading = false;
    lspError = null;
    lspResult = null;
    lspAnchor = null;
  }

  function identifierAt(line: string, character: number): string | null {
    if (line.length === 0) return null;
    let index = Math.max(0, Math.min(character, line.length - 1));
    if (!isIdentifierChar(line[index] ?? "") && character > 0) {
      index = Math.max(0, Math.min(character - 1, line.length - 1));
    }
    if (!isIdentifierChar(line[index] ?? "")) return null;
    let start = index;
    let end = index + 1;
    while (start > 0 && isIdentifierChar(line[start - 1])) start -= 1;
    while (end < line.length && isIdentifierChar(line[end])) end += 1;
    const value = line.slice(start, end);
    return value.length > 0 ? value : null;
  }

  function isIdentifierChar(ch: string): boolean {
    return /[A-Za-z0-9_$]/.test(ch);
  }

  function lineHasSymbol(line: string, symbol: string | null): boolean {
    if (!symbol) return false;
    return line.includes(symbol);
  }

  function symbolCharacterAt(line: string, character: number): number {
    if (line.length === 0) return 0;
    const current = Math.max(0, Math.min(character, line.length - 1));
    if (isIdentifierChar(line[current] ?? "")) return current;
    if (character > 0) {
      const previous = Math.max(0, Math.min(character - 1, line.length - 1));
      if (isIdentifierChar(line[previous] ?? "")) return previous;
    }
    return current;
  }

  function clickCharacter(event: MouseEvent, line: string): number {
    const target = event.currentTarget as HTMLElement;
    const rect = target.getBoundingClientRect();
    const style = getComputedStyle(target);
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d");
    if (!ctx) return 0;
    ctx.font = `${style.fontWeight} ${style.fontSize} ${style.fontFamily}`;
    const charWidth = ctx.measureText("M").width || 7;
    const raw = Math.floor((event.clientX - rect.left) / charWidth);
    return Math.max(0, Math.min(raw, Math.max(0, line.length - 1)));
  }

  function lineCharacterFromOffset(content: string, offset: number): { line: number; character: number } {
    const beforeCursor = content.slice(0, Math.max(0, Math.min(offset, content.length)));
    const lines = beforeCursor.split("\n");
    return {
      line: lines.length - 1,
      character: lines[lines.length - 1]?.length ?? 0
    };
  }

  async function handleCodeClick(event: MouseEvent, lineIndex: number, line: string) {
    if (!activePath || !activeFile || activeFile.encoding !== "utf8") return;
    const character = clickCharacter(event, line);
    const symbolCharacter = symbolCharacterAt(line, character);
    const symbol = identifierAt(line, symbolCharacter);
    if (!symbol) {
      clearLspState();
      return;
    }
    await inspectSymbol(lineIndex, symbolCharacter, symbol);
  }

  function handleCodeKeydown(event: KeyboardEvent, lineIndex: number, line: string) {
    if (event.key !== "Enter" && event.key !== " ") return;
    const match = line.match(/[A-Za-z_$][A-Za-z0-9_$]*/);
    if (!match || match.index === undefined) return;
    event.preventDefault();
    void inspectSymbol(lineIndex, match.index, match[0]);
  }

  function handleEditorCursorInspect(event: MouseEvent | KeyboardEvent) {
    if (!activePath || !activeFile || activeFile.encoding !== "utf8") return;
    const target = event.currentTarget as HTMLTextAreaElement;
    const { line, character } = lineCharacterFromOffset(draftContent, target.selectionStart);
    const sourceLine = draftLines[line] ?? "";
    const symbolCharacter = symbolCharacterAt(sourceLine, character);
    const symbol = identifierAt(sourceLine, symbolCharacter);
    if (!symbol) {
      clearLspState();
      return;
    }
    void inspectSymbol(line, symbolCharacter, symbol);
  }

  function handleEditorKeyup(event: KeyboardEvent) {
    if (event.key === "Enter" || event.key === " " || event.key.startsWith("Arrow")) {
      handleEditorCursorInspect(event);
    }
  }

  async function inspectSymbol(lineIndex: number, character: number, symbol: string) {
    if (!activePath || !activeFile || activeFile.encoding !== "utf8") return;
    const inspectPath = activePath;
    const inspectRoot = root;
    const inspectSessionId = sessionId;
    const inspectGeneration = ++lspInspectGeneration;
    selectedSymbol = symbol;
    lspAnchor = { line: lineIndex, character };
    lspLoading = true;
    lspError = null;
    lspResult = null;
    try {
      const result = await lspInspect(inspectPath, inspectRoot, lineIndex, character);
      if (
        destroyed ||
        inspectGeneration !== lspInspectGeneration ||
        activePath !== inspectPath ||
        root !== inspectRoot ||
        sessionId !== inspectSessionId
      ) return;
      lspResult = result;
    } catch (err) {
      if (
        destroyed ||
        inspectGeneration !== lspInspectGeneration ||
        activePath !== inspectPath ||
        root !== inspectRoot ||
        sessionId !== inspectSessionId
      ) return;
      lspError = err instanceof Error ? err.message : String(err);
    } finally {
      if (
        !destroyed &&
        inspectGeneration === lspInspectGeneration &&
        activePath === inspectPath &&
        root === inspectRoot &&
        sessionId === inspectSessionId
      ) {
        lspLoading = false;
      }
    }
  }

  type LspLocation = {
    label: string;
    file: string;
    line: number;
    character: number;
    kind: "definition" | "reference" | "incoming" | "outgoing";
  };

  type LspReferenceSection = {
    title: string;
    locations: LspLocation[];
  };

  function parseDefinitionLocations(result: LspInspectResult): LspLocation[] {
    const text = result.operations.goToDefinition?.result ?? "";
    return text
      .split("\n")
      .filter((line) => line.trim().startsWith("- "))
      .map((line) => parsePathLocation(line.replace(/^\s*-\s*/, ""), "definition"))
      .filter((value): value is LspLocation => value !== null);
  }

  function buildReferenceSections(result: LspInspectResult): LspReferenceSection[] {
    const sections: LspReferenceSection[] = [];
    const refs = parseGroupedReferences(result.operations.findReferences?.result ?? "");
    if (refs.length > 0) sections.push({ title: "References", locations: refs });
    const incoming = parseCallLocations(result.operations.incomingCalls?.result ?? "", "incoming");
    if (incoming.length > 0) sections.push({ title: "Called by", locations: incoming });
    const outgoing = parseCallLocations(result.operations.outgoingCalls?.result ?? "", "outgoing");
    if (outgoing.length > 0) sections.push({ title: "Calls", locations: outgoing });
    return sections;
  }

  function parseGroupedReferences(text: string): LspLocation[] {
    const locations: LspLocation[] = [];
    let file = "";
    for (const raw of text.split("\n")) {
      const line = raw.trimEnd();
      if (!line.trim()) continue;
      if (!line.startsWith(" ") && line.endsWith(":")) {
        file = line.slice(0, -1);
        continue;
      }
      const match = line.match(/-\s*line\s+(\d+):(\d+)/);
      if (match && file) {
        locations.push({
          label: `${file}:${match[1]}:${match[2]}`,
          file,
          line: Number(match[1]),
          character: Number(match[2]),
          kind: "reference"
        });
      }
    }
    return locations;
  }

  function parseCallLocations(text: string, kind: "incoming" | "outgoing"): LspLocation[] {
    return text
      .split("\n")
      .filter((line) => line.trim().startsWith("- "))
      .map((line) => {
        const body = line.replace(/^\s*-\s*/, "");
        const [name, rest] = body.split(/\s+-\s+/, 2);
        const loc = parsePathLocation(rest ?? body, kind);
        return loc ? { ...loc, label: name && rest ? `${name} - ${loc.label}` : loc.label } : null;
      })
      .filter((value): value is LspLocation => value !== null);
  }

  function parsePathLocation(text: string, kind: LspLocation["kind"]): LspLocation | null {
    const match = text.match(/^(.*):(\d+):(\d+)$/);
    if (!match) return null;
    return {
      label: text,
      file: match[1],
      line: Number(match[2]),
      character: Number(match[3]),
      kind
    };
  }

  function resolvedLocationPath(file: string): string {
    if (file.startsWith("/")) return file;
    return joinPath(root, file);
  }

  async function openLocation(location: LspLocation) {
    const path = resolvedLocationPath(location.file);
    await openFile(path, 0, { line: location.line, character: location.character });
  }

  type TreeRow = {
    path: string;
    name: string;
    depth: number;
    kind: "file" | "directory" | "symlink";
    size: number;
  };

  // Flatten the tree into a single row list so we can render with one
  // {#each}. Recursion is iterative (stack) to keep Svelte happy — each
  // child only appears when its parent is in `expanded`.
  function buildRows(current: string, depth: number, acc: TreeRow[]) {
    const entries = cache.get(current);
    if (!entries) return;
    for (const e of entries) {
      const childPath = joinPath(current, e.name);
      acc.push({
        path: childPath,
        name: e.name,
        depth,
        kind: e.kind,
        size: e.size
      });
      if (
        (e.kind === "directory" || e.kind === "symlink") &&
        expanded.has(childPath)
      ) {
        buildRows(childPath, depth + 1, acc);
      }
    }
  }

  let rows = $derived.by<TreeRow[]>(() => {
    const acc: TreeRow[] = [];
    // Touch these so Svelte knows to re-derive when they change — the
    // cache / expanded sets are replaced by reference on update, but
    // derived.by only tracks values it reads inside the closure.
    cache;
    expanded;
    buildRows(root, 0, acc);
    return acc;
  });
</script>

<div class="pf-files-pane">
  <aside class="tree">
    <div class="tree-head">
      <Icon name="folder" size={12} />
      <span class="tree-root" title={root}>
        {root ? (root.split("/").pop() || root) : "workspace"}
      </span>
    </div>
    <div class="tree-body">
      {#if previewMode}
        <div class="tree-empty">
          <div class="msg">Files view is live in the desktop app</div>
          <div class="sub">Launch Puffer locally to browse this session's working directory.</div>
        </div>
      {:else if errors.has(root) && !cache.has(root)}
        <div class="tree-empty">
          <div class="msg">Failed to load directory</div>
          <div class="sub mono">{errors.get(root)}</div>
        </div>
      {:else if loading.has(root) && !cache.has(root)}
        <div class="tree-empty sub">Loading...</div>
      {:else if rows.length === 0 && cache.has(root)}
        <div class="tree-empty sub">Empty directory</div>
      {:else}
        {#each rows as row (row.path)}
          <button
            type="button"
            class="row"
            class:active={activePath === row.path}
            style="padding-left: {8 + row.depth * 14}px"
            onclick={() =>
              row.kind === "directory" || (row.kind === "symlink" && !errors.has(row.path))
                ? toggleDir(row.path)
                : openFile(row.path, row.size)}
            ondblclick={(event) => {
              if (row.kind !== "file") return;
              event.preventDefault();
              void openFile(row.path, row.size, { pinned: true });
            }}
            aria-expanded={row.kind === "directory" || row.kind === "symlink" ? expanded.has(row.path) : undefined}
            title={row.path}
          >
            {#if row.kind === "directory"}
              <span class="chev" class:on={expanded.has(row.path)}>
                <Icon name="chevR" size={10} />
              </span>
              <Icon
                name={expanded.has(row.path) ? "folderOpen" : "folder"}
                size={12}
                color="var(--muted-foreground)"
              />
            {:else if row.kind === "symlink"}
              <span class="chev" class:on={expanded.has(row.path)}>
                <Icon name="chevR" size={10} />
              </span>
              <Icon name="link" size={12} color="var(--muted-foreground)" />
            {:else}
              <span class="chev-spacer"></span>
              <Icon name="file" size={12} color="var(--muted-foreground)" />
            {/if}
            <span class="row-name">{row.name}</span>
          </button>
          {#if (row.kind === "directory" || row.kind === "symlink") && expanded.has(row.path) && errors.has(row.path)}
            <div class="row-error mono" style="padding-left: {8 + (row.depth + 1) * 14}px">
              {errors.get(row.path)}
            </div>
          {:else if (row.kind === "directory" || row.kind === "symlink") && expanded.has(row.path) && loading.has(row.path) && !cache.has(row.path)}
            <div class="row-sub" style="padding-left: {8 + (row.depth + 1) * 14}px">
              Loading...
            </div>
          {:else if (row.kind === "directory" || row.kind === "symlink") && expanded.has(row.path) && cache.has(row.path) && cache.get(row.path)!.length === 0}
            <div class="row-sub" style="padding-left: {8 + (row.depth + 1) * 14}px">
              (empty)
            </div>
          {/if}
        {/each}
      {/if}
    </div>
  </aside>

  <section class="viewer">
    {#if previewMode}
      <div class="viewer-empty">
        <Icon name="file" size={20} color="var(--muted-foreground)" />
        <div class="title">File preview is live in the desktop app</div>
        <div class="sub">Open Puffer locally to preview files from this session.</div>
      </div>
    {:else if !activePath}
      <div class="viewer-empty">
        <Icon name="file" size={20} color="var(--muted-foreground)" />
        <div class="title">No file selected</div>
        <div class="sub">Pick a file in the tree on the left to preview it here.</div>
      </div>
    {:else}
      <div class="file-tabs" role="tablist" aria-label="Open files">
        {#each openTabs as tab (tab.path)}
          {@const closeLabel = closeFileTabLabel(tab.path)}
          <div
            role="tab"
            tabindex="0"
            aria-selected={activePath === tab.path}
            class="file-tab"
            class:active={activePath === tab.path}
            class:preview={!tab.pinned}
            class:dirty={isTabDirty(tab.path)}
            title={tab.path}
            onclick={() => void activateFile(tab.path, tab.size)}
            ondblclick={() => pinTab(tab.path)}
            onkeydown={(event) => handleFileTabKeydown(event, tab.path, tab.size)}
          >
            <Icon name="file" size={11} color="var(--muted-foreground)" />
            <span class="tab-title">{tab.name}</span>
            {#if isTabDirty(tab.path)}
              <span class="dirty-dot" aria-label="Unsaved changes"></span>
            {/if}
            <button
              type="button"
              class="tab-close"
              aria-label={closeLabel}
              title={closeLabel}
              onclick={(event) => void closeTab(event, tab.path)}
            >
              <Icon name="x" size={11} />
            </button>
          </div>
        {/each}
      </div>
      <header class="viewer-head">
        <Icon name="file" size={12} color="var(--muted-foreground)" />
        <span class="path mono" title={activePath}>{activePath}</span>
        <span class="size">{fmtSize(activeSize)}</span>
        {#if activeFile?.truncated}
          <span class="badge">truncated</span>
        {/if}
        {#if saveError}
          <span class="save-error mono">{saveError}</span>
        {/if}
        <div class="viewer-actions">
          {#if canToggleRaw}
            <div class="viewer-mode" aria-label="File view mode">
              <button
                type="button"
                class:active={viewerMode === "preview"}
                aria-pressed={viewerMode === "preview"}
                onclick={() => (viewerMode = "preview")}
              >
                Preview
              </button>
              <button
                type="button"
                class:active={viewerMode === "raw"}
                aria-pressed={viewerMode === "raw"}
                onclick={() => (viewerMode = "raw")}
              >
                Raw
              </button>
            </div>
          {/if}
          {#if canEdit && dirty}
            <button
              type="button"
              class="file-action"
              onclick={cancelEditing}
              disabled={saving}
            >
              Cancel
            </button>
            <button
              type="button"
              class="file-action primary"
              onclick={() => void saveEditing()}
              disabled={saving || !dirty}
            >
              {saving ? "Saving..." : "Save"}
            </button>
          {/if}
        </div>
      </header>
      <div class="viewer-body">
        {#if activeLoading && !activeFile}
          <div class="viewer-msg sub">Loading...</div>
        {:else if activeError}
          <div class="viewer-msg err mono">{activeError}</div>
        {:else if activePreviewLoading}
          <div class="viewer-msg sub">Preparing preview...</div>
        {:else if activePreviewError}
          <div class="viewer-msg err mono">{activePreviewError}</div>
        {:else if showRawEditor}
          <div class="editor-shell">
            <div class="editor-gutter" bind:this={editorGutterEl} aria-hidden="true">
              {#each draftLineNumbers as lineNumber}
                <span>{lineNumber}</span>
              {/each}
            </div>
            <div class="editor-stack">
              <pre class="editor-highlight" bind:this={editorHighlightEl} aria-hidden="true">{#each draftLines as line}<span class:symbol-line={lineHasSymbol(line, selectedSymbol)}><HighlightedLine text={line || " "} path={activePath} highlight={selectedSymbol} /></span>{/each}</pre>
              <textarea
                class="editor"
                bind:this={editorEl}
                value={draftContent}
                spellcheck="false"
                wrap="off"
                oninput={(event) => setDraft((event.currentTarget as HTMLTextAreaElement).value)}
                onkeydown={handleEditorKeydown}
                onkeyup={handleEditorKeyup}
                onmouseup={handleEditorCursorInspect}
                onscroll={syncEditorScroll}
                aria-label="Edit file contents"
              ></textarea>
            </div>
          </div>
        {:else if activePreview && activePreview.kind === "markdown"}
          <article class="file-preview markdown-preview" aria-label="Markdown preview">
            {@html activePreview.html}
          </article>
        {:else if activePreview && activePreview.kind === "csv"}
          <div class="file-preview table-preview" aria-label="CSV preview">
            <table>
              <tbody>
                {#each activePreview.rows as row, rowIndex}
                  <tr>
                    {#each row as cell}
                      {#if rowIndex === 0}
                        <th>{cell}</th>
                      {:else}
                        <td>{cell}</td>
                      {/if}
                    {/each}
                  </tr>
                {/each}
              </tbody>
            </table>
          </div>
        {:else if activePreview && activePreview.kind === "pdf"}
          <div class="file-preview pdf-shell" aria-label="PDF preview">
            <PdfDocumentPreview base64={activePreview.base64} textLines={activePreview.lines} />
          </div>
        {:else if activePreview && activePreview.kind === "docx"}
          <article class="file-preview office-preview" aria-label="DOCX preview">
            {#each activePreview.paragraphs as paragraph}
              <p>{paragraph}</p>
            {/each}
          </article>
        {:else if activePreview && activePreview.kind === "pptx"}
          <div class="file-preview office-preview" aria-label="PowerPoint preview">
            {#each activePreview.slides as slide}
              <section>
                <h2>{slide.title}</h2>
                {#each slide.lines as line}
                  <p>{line}</p>
                {/each}
              </section>
            {/each}
          </div>
        {:else if activePreview && activePreview.kind === "xlsx"}
          <div class="file-preview spreadsheet-preview" aria-label="Excel preview">
            {#each activePreview.sheets as sheet}
              <section>
                <h2>{sheet.name}</h2>
                <table>
                  <tbody>
                    {#each sheet.rows as row}
                      <tr>
                        {#each row as cell}
                          <td>{cell}</td>
                        {/each}
                      </tr>
                    {/each}
                  </tbody>
                </table>
              </section>
            {/each}
          </div>
        {:else if activePreview && activePreview.kind === "office-binary"}
          <div class="file-preview office-preview" aria-label={activePreview.title}>
            <section>
              <h2>{activePreview.title}</h2>
              {#if activePreview.html}
                <div class="legacy-office-html">{@html activePreview.html}</div>
              {:else}
                {#each activePreview.lines as line}
                  <p>{line}</p>
                {/each}
              {/if}
            </section>
          </div>
        {:else if activeFile && activeFile.encoding === "utf8"}
          <pre class="code"><!--
            --><div class="gutter">{#each viewerLines as _line, i}<span class="gl">{i + 1}</span>{/each}</div><!--
            --><div class="lines">{#each viewerLines as line, i}<span class="ln" class:symbol-line={lineHasSymbol(line, selectedSymbol)} data-line={i + 1} role="button" tabindex="0" onclick={(event) => void handleCodeClick(event, i, line)} onkeydown={(event) => handleCodeKeydown(event, i, line)}><HighlightedLine text={line || " "} path={activePath} highlight={selectedSymbol} /></span>{/each}</div><!--
          --></pre>
        {:else if activeFile && activeFile.encoding === "base64"}
          <div class="viewer-msg">
            Binary file ({fmtSize(activeFile.size)}). Download is not supported yet.
          </div>
        {/if}
        {#if selectedSymbol}
          <aside class="lsp-popup" aria-label="Symbol references">
            <header>
              <div>
                <div class="eyebrow">Symbol</div>
                <div class="symbol">{selectedSymbol}</div>
              </div>
              <button type="button" aria-label="Close symbol popup" onclick={clearLspState}>
                <Icon name="x" size={12} />
              </button>
            </header>
            {#if lspAnchor}
              <div class="lsp-origin mono">line {lspAnchor.line + 1}:{lspAnchor.character + 1}</div>
            {/if}
            {#if lspLoading}
              <div class="lsp-state">Loading language server results...</div>
            {:else if lspError}
              <div class="lsp-state danger">{lspError}</div>
            {:else if lspResult}
              {#if hoverText && !hoverText.startsWith("No hover")}
                <section>
                  <h3>Hover</h3>
                  <pre>{hoverText}</pre>
                </section>
              {/if}
              {#if definitionLocations.length > 0}
                <section>
                  <h3>Definition</h3>
                  {#each definitionLocations as location, i (i)}
                    <button type="button" class="lsp-location" onclick={() => void openLocation(location)}>
                      <span class="kind">def</span>
                      <span class="label">{location.label}</span>
                    </button>
                  {/each}
                </section>
              {/if}
              {#each referenceSections as section (section.title)}
                <section>
                  <h3>{section.title}</h3>
                  {#each section.locations as location, i (i)}
                    <button type="button" class="lsp-location" onclick={() => void openLocation(location)}>
                      <span class="kind">{location.kind === "reference" ? "ref" : location.kind === "incoming" ? "in" : "out"}</span>
                      <span class="label">{location.label}</span>
                    </button>
                  {/each}
                </section>
              {/each}
              {#if definitionLocations.length === 0 && referenceSections.length === 0 && (!hoverText || hoverText.startsWith("No hover"))}
                <div class="lsp-state">{lspFallbackText}</div>
              {/if}
            {/if}
          </aside>
        {/if}
      </div>
    {/if}
  </section>
</div>

<style>
  .pf-files-pane {
    flex: 1;
    display: flex;
    min-height: 0;
    overflow: hidden;
  }

  .tree {
    width: 240px;
    flex-shrink: 0;
    border-right: 1px solid var(--border);
    display: flex;
    flex-direction: column;
    background: color-mix(in oklab, var(--background) 97%, var(--muted));
  }
  .tree-head {
    flex-shrink: 0;
    padding: 8px 10px;
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 11px;
    font-weight: 600;
    color: var(--muted-foreground);
    text-transform: uppercase;
    letter-spacing: 0.05em;
    border-bottom: 1px solid var(--border);
  }
  .tree-root {
    font-family: var(--font-mono);
    text-transform: none;
    letter-spacing: 0;
    color: var(--foreground);
    font-weight: 500;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .tree-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 4px 0;
    font-size: 12px;
  }
  .tree-empty {
    padding: 20px 12px;
    color: var(--muted-foreground);
    font-size: 12px;
    text-align: center;
  }
  .tree-empty .msg {
    color: var(--foreground);
    font-weight: 500;
    margin-bottom: 4px;
  }
  .tree-empty .sub { font-size: 11px; }
  .tree-empty .mono { font-family: var(--font-mono); }
  .tree-empty.sub {
    text-align: left;
    font-style: italic;
  }

  .row {
    width: 100%;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 3px 8px 3px 8px;
    background: transparent;
    border: 0;
    border-radius: 4px;
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    text-align: left;
    transition: background 100ms;
  }
  .row:hover { background: var(--accent); }
  .row.active { background: var(--muted); color: var(--foreground); }
  .row .chev {
    width: 12px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    color: var(--muted-foreground);
    transition: transform 120ms;
  }
  .row .chev.on { transform: rotate(90deg); }
  .row .chev-spacer { width: 12px; display: inline-block; }
  .row-name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-size: 12px;
  }
  .row-sub,
  .row-error {
    font-size: 11px;
    color: var(--muted-foreground);
    padding: 2px 8px;
    font-style: italic;
  }
  .row-error {
    color: oklch(0.55 0.2 30);
    font-style: normal;
    white-space: pre-wrap;
    word-break: break-word;
  }

  .viewer {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    background: var(--background);
  }
  .file-tabs {
    flex-shrink: 0;
    display: flex;
    min-height: 31px;
    overflow-x: auto;
    overflow-y: hidden;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 95%, var(--muted));
  }
  .file-tab {
    height: 31px;
    min-width: 120px;
    max-width: 220px;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 0 6px 0 10px;
    border: 0;
    border-right: 1px solid var(--border);
    border-bottom: 2px solid transparent;
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 12px;
    cursor: pointer;
  }
  .file-tab:hover {
    background: color-mix(in oklab, var(--accent) 55%, transparent);
    color: var(--foreground);
  }
  .file-tab.active {
    background: var(--background);
    border-bottom-color: var(--puffer-accent, var(--foreground));
    color: var(--foreground);
  }
  .file-tab.preview .tab-title {
    font-style: italic;
  }
  .file-tab.dirty .tab-title {
    color: var(--foreground);
  }
  .file-tab .tab-title {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
  }
  .dirty-dot {
    width: 6px;
    height: 6px;
    flex-shrink: 0;
    border-radius: 999px;
    background: var(--muted-foreground);
  }
  .tab-close {
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    background: transparent;
    flex-shrink: 0;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 4px;
    color: var(--muted-foreground);
    cursor: pointer;
  }
  .tab-close:hover,
  .tab-close:focus-visible {
    background: var(--muted);
    color: var(--foreground);
    outline: none;
  }
  .viewer-head {
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    font-size: 12px;
    background: color-mix(in oklab, var(--background) 97%, var(--muted));
  }
  .viewer-head .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--foreground);
  }
  .viewer-head .size {
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--muted-foreground);
  }
  .viewer-head .badge {
    font-size: 9px;
    font-weight: 700;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    padding: 1px 5px;
    border-radius: 3px;
    background: oklch(0.7 0.16 40);
    color: white;
  }
  .viewer-head .save-error {
    min-width: 0;
    color: oklch(0.55 0.2 30);
    font-size: 11px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .viewer-actions {
    margin-left: auto;
    display: inline-flex;
    align-items: center;
    gap: 6px;
    flex-shrink: 0;
  }
  .viewer-mode {
    height: 24px;
    display: inline-flex;
    align-items: center;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: var(--background);
  }
  .viewer-mode button {
    height: 22px;
    min-width: 54px;
    padding: 0 8px;
    border: 0;
    border-right: 1px solid var(--border);
    background: transparent;
    color: var(--muted-foreground);
    font: inherit;
    font-size: 11px;
    cursor: pointer;
  }
  .viewer-mode button:last-child {
    border-right: 0;
  }
  .viewer-mode button:hover {
    background: var(--accent);
    color: var(--foreground);
  }
  .viewer-mode button.active {
    background: var(--foreground);
    color: var(--background);
  }
  .file-action {
    height: 24px;
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0 9px;
    background: var(--background);
    color: var(--foreground);
    font-size: 11.5px;
    font-weight: 500;
    cursor: pointer;
  }
  .file-action:hover:not(:disabled) {
    background: var(--accent);
  }
  .file-action.primary {
    background: var(--foreground);
    border-color: var(--foreground);
    color: var(--background);
  }
  .file-action:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }

  .viewer-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    position: relative;
  }
  .viewer-empty {
    height: 100%;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 8px;
    padding: 40px;
    color: var(--muted-foreground);
    text-align: center;
  }
  .viewer-empty .title { font-size: 14px; font-weight: 600; color: var(--foreground); }
  .viewer-empty .sub { font-size: 12.5px; max-width: 360px; line-height: 1.55; }

  .viewer-msg {
    padding: 20px 24px;
    color: var(--muted-foreground);
    font-size: 12.5px;
  }
  .viewer-msg.sub { font-style: italic; }
  .viewer-msg.err {
    color: oklch(0.55 0.2 30);
    font-family: var(--font-mono);
    white-space: pre-wrap;
    word-break: break-word;
  }
  .file-preview {
    min-height: 100%;
    padding: 18px 24px;
    color: var(--foreground);
    font-size: 13px;
    line-height: 1.55;
  }
  .markdown-preview {
    max-width: 860px;
  }
  .markdown-preview :global(h1),
  .markdown-preview :global(h2),
  .markdown-preview :global(h3),
  .office-preview h2,
  .spreadsheet-preview h2 {
    margin: 0 0 10px;
    color: var(--foreground);
    font-weight: 650;
    line-height: 1.25;
  }
  .markdown-preview :global(h1) { font-size: 24px; }
  .markdown-preview :global(h2),
  .office-preview h2,
  .spreadsheet-preview h2 { font-size: 16px; }
  .markdown-preview :global(h3) { font-size: 14px; }
  .markdown-preview :global(p),
  .office-preview p {
    margin: 0 0 10px;
  }
  .markdown-preview :global(ul) {
    margin: 0 0 12px;
    padding-left: 20px;
  }
  .markdown-preview :global(blockquote) {
    margin: 0 0 12px;
    padding: 8px 12px;
    border-left: 3px solid var(--border);
    color: var(--muted-foreground);
    background: color-mix(in oklab, var(--background) 96%, var(--muted));
  }
  .markdown-preview :global(code) {
    font-family: var(--font-mono);
    font-size: 12px;
    background: var(--muted);
    padding: 1px 4px;
    border-radius: 4px;
  }
  .markdown-preview :global(pre) {
    margin: 0 0 12px;
    padding: 10px 12px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: color-mix(in oklab, var(--background) 95%, var(--muted));
  }
  .markdown-preview :global(pre code) {
    padding: 0;
    background: transparent;
  }
  .table-preview,
  .spreadsheet-preview {
    overflow: auto;
  }
  .table-preview table,
  .spreadsheet-preview table {
    border-collapse: collapse;
    min-width: min(680px, 100%);
    font-size: 12.5px;
  }
  .table-preview th,
  .table-preview td,
  .spreadsheet-preview td {
    border: 1px solid var(--border);
    padding: 6px 8px;
    text-align: left;
    vertical-align: top;
    white-space: pre-wrap;
  }
  .table-preview th {
    background: color-mix(in oklab, var(--background) 92%, var(--muted));
    font-weight: 650;
  }
  .pdf-shell {
    position: relative;
    padding: clamp(10px, 2vw, 20px);
    height: 100%;
    overflow: hidden;
    background: color-mix(in oklab, var(--background) 94%, var(--muted));
  }
  .office-preview section,
  .spreadsheet-preview section {
    margin: 0 0 18px;
    padding-bottom: 14px;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 70%, transparent);
  }
  .office-preview section:last-child,
  .spreadsheet-preview section:last-child {
    border-bottom: 0;
  }
  .legacy-office-html {
    color: var(--ink);
  }
  .legacy-office-html :global(p),
  .legacy-office-html :global(div) {
    margin: 0 0 8px;
  }
  .legacy-office-html :global(table) {
    border-collapse: collapse;
    width: 100%;
  }
  .legacy-office-html :global(td),
  .legacy-office-html :global(th) {
    border: 1px solid var(--border);
    padding: 6px 8px;
    vertical-align: top;
  }
  .editor-shell {
    height: 100%;
    min-height: 100%;
    display: grid;
    grid-template-columns: 48px minmax(0, 1fr);
    background: var(--background);
  }
  .editor-gutter {
    overflow: hidden;
    padding: 10px 8px 10px 12px;
    border-right: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 97%, var(--muted));
    color: var(--muted-foreground);
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    text-align: right;
    user-select: none;
  }
  .editor-gutter span {
    display: block;
    font-variant-numeric: tabular-nums;
  }
  .editor-stack {
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    background: var(--background);
  }
  .editor-highlight,
  .editor {
    width: 100%;
    height: 100%;
    min-height: 100%;
    padding: 10px 12px;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    tab-size: 2;
    white-space: pre;
    overflow: auto;
  }
  .editor-highlight {
    position: absolute;
    inset: 0;
    margin: 0;
    border: 0;
    pointer-events: none;
    color: var(--foreground);
  }
  .editor-highlight span {
    display: block;
    min-height: 1.5em;
  }
  .editor-highlight span.symbol-line {
    background: color-mix(in oklab, var(--puffer-accent) 6%, transparent);
  }
  .editor {
    position: relative;
    z-index: 1;
    border: 0;
    resize: none;
    outline: none;
    background: transparent;
    color: transparent;
    caret-color: var(--foreground);
    -webkit-text-fill-color: transparent;
  }
  .editor::selection {
    background: color-mix(in oklab, var(--puffer-accent, #2563eb) 24%, transparent);
  }

  .code {
    margin: 0;
    font-family: var(--font-mono);
    font-size: 12px;
    line-height: 1.5;
    display: flex;
    min-height: 100%;
  }
  .code .gutter {
    flex-shrink: 0;
    display: flex;
    flex-direction: column;
    padding: 10px 8px 10px 12px;
    color: var(--muted-foreground);
    border-right: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 97%, var(--muted));
    user-select: none;
    text-align: right;
    min-width: 38px;
  }
  .code .gutter .gl {
    display: block;
    font-variant-numeric: tabular-nums;
  }
  .code .lines {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    padding: 10px 12px;
    color: var(--foreground);
  }
  .code .lines .ln {
    display: block;
    white-space: pre;
    cursor: text;
    border-radius: 3px;
  }
  .code .lines .ln:hover {
    background: color-mix(in oklab, var(--accent) 60%, transparent);
  }
  .code .lines .ln.symbol-line {
    background: color-mix(in oklab, var(--puffer-accent) 6%, transparent);
  }
  .lsp-popup {
    position: absolute;
    top: 12px;
    right: 16px;
    width: min(440px, calc(100% - 32px));
    max-height: min(620px, calc(100% - 24px));
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
    box-shadow: 0 18px 50px -28px rgba(0, 0, 0, 0.45);
    z-index: 10;
    font-size: 12px;
  }
  .lsp-popup header {
    position: sticky;
    top: 0;
    z-index: 1;
    display: flex;
    align-items: center;
    gap: 10px;
    justify-content: space-between;
    padding: 10px 12px;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--background) 98%, var(--muted));
  }
  .lsp-popup header button {
    all: unset;
    width: 22px;
    height: 22px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    border-radius: 5px;
    color: var(--muted-foreground);
    cursor: pointer;
  }
  .lsp-popup header button:hover {
    background: var(--muted);
    color: var(--foreground);
  }
  .lsp-popup .eyebrow {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted-foreground);
    font-weight: 700;
  }
  .lsp-popup .symbol {
    margin-top: 2px;
    font-family: var(--font-mono);
    font-weight: 700;
    color: var(--foreground);
  }
  .lsp-origin {
    padding: 7px 12px;
    border-bottom: 1px solid var(--border);
    color: var(--muted-foreground);
    font-size: 11px;
  }
  .lsp-popup section {
    padding: 10px 12px;
    border-bottom: 1px solid color-mix(in oklab, var(--border) 70%, transparent);
  }
  .lsp-popup section:last-child {
    border-bottom: 0;
  }
  .lsp-popup h3 {
    margin: 0 0 7px;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.06em;
    color: var(--muted-foreground);
  }
  .lsp-popup pre,
  .lsp-state {
    margin: 0;
    white-space: pre-wrap;
    word-break: break-word;
    font-family: var(--font-mono);
    font-size: 11.5px;
    line-height: 1.45;
    color: var(--foreground);
  }
  .lsp-state {
    padding: 12px;
    color: var(--muted-foreground);
  }
  .lsp-state.danger {
    color: oklch(0.55 0.2 30);
  }
  .lsp-location {
    width: 100%;
    display: grid;
    grid-template-columns: 34px minmax(0, 1fr);
    gap: 7px;
    align-items: baseline;
    padding: 5px 6px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    text-align: left;
    cursor: pointer;
    font: inherit;
  }
  .lsp-location:hover {
    background: var(--muted);
  }
  .lsp-location .kind {
    font-family: var(--font-mono);
    font-size: 10px;
    color: var(--muted-foreground);
    text-transform: uppercase;
  }
  .lsp-location .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-family: var(--font-mono);
    font-size: 11.5px;
  }
</style>
