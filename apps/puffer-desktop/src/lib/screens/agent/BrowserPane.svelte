<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import Icon from "../../design/Icon.svelte";
  import { ensureLocalDaemonClient } from "../../api/daemonClient";
  import {
    browserClose,
    browserCopySelection,
    browserCursor,
    browserHistory,
    browserInput,
    browserNavigate,
    browserOpen,
    browserReload,
    browserResize,
    browserTabClose,
    browserTabFocus,
    browserTabOpen,
    browserTabsList,
    isDaemonReachable,
    type BrowserDevtoolsEvent,
    type BrowserFrameEvent,
    type BrowserState,
    type BrowserTabInfo,
    type BrowserTabsState
  } from "../../api/desktop";

  type Props = {
    sessionId: string;
  };

  type BrowserTab = {
    id: string;
    label: string;
    url: string;
    title: string;
    loading: boolean;
    error: string | null;
    status: string;
    connected: boolean;
    favicon: string;
    frame: BrowserFrameEvent | null;
    devtools: BrowserDevtoolsEvent[];
  };

  type ApplyTabsOptions = {
    allowEmpty?: boolean;
  };

  let { sessionId }: Props = $props();

  let viewport: HTMLDivElement | null = $state(null);
  let canvas: HTMLCanvasElement | null = $state(null);
  let tabs = $state<BrowserTab[]>([]);
  let activeTabId = $state("");
  let nextTabNumber = 2;
  let urlDraft = $state("about:blank");
  let currentUrl = $state("about:blank");
  let title = $state("");
  let status = $state("Starting Chrome...");
  let error = $state<string | null>(null);
  let loading = $state(false);
  let connected = $state(false);
  let frameWidth = $state(1);
  let frameHeight = $state(1);
  let browserCursorStyle = $state("default");
  let showDevtools = $state(false);
  let devtoolsView = $state<"console" | "network">("console");

  let disposers: Array<() => void> = [];
  let activeDisposers: Array<() => void> = [];
  let resizeObserver: ResizeObserver | null = null;
  let disposed = false;
  let mounted = false;
  let activeRootSessionId = "";
  let activeEventSessionId = "";
  let sessionGeneration = 0;
  let lastResize = { width: 960, height: 720 };
  let activePointerId: number | null = null;
  let activeButton: "left" | "middle" | "right" | "none" = "none";
  let activeButtons = 0;
  let activeClickCount = 0;
  let lastClick = {
    at: 0,
    x: 0,
    y: 0,
    button: "none" as "left" | "middle" | "right" | "none",
    count: 0
  };
  let drawSerial = 0;
  let cursorTimer: ReturnType<typeof setTimeout> | null = null;
  let cursorRequest = 0;
  let pendingCursorPoint: { x: number; y: number } | null = null;
  let pendingCursorSessionId: string | null = null;
  let tabStateVersion = 0;

  let activeTab = $derived(tabs.find((tab) => tab.id === activeTabId) ?? tabs[0]);
  let browserControlsEnabled = $derived(Boolean(activeTab && connected));
  let activeDevtools = $derived(activeTab?.devtools ?? []);
  let consoleEvents = $derived(activeDevtools.filter((item) => item.kind === "console"));
  let networkEvents = $derived(activeDevtools.filter((item) => item.kind === "network"));

  onMount(async () => {
    if (!viewport || !canvas) return;
    mounted = true;
    if (!isDaemonReachable()) {
      status = "Browser is available when connected to the Corbina backend.";
      error = "No backend connection is configured for this preview.";
      return;
    }

    const ro = new ResizeObserver(() => {
      const size = measureViewport();
      if (!size) return;
      lastResize = size;
      if (connected && activeTabId) {
        void browserResize(activeBackendSessionId(), size.width, size.height).catch((err) => {
          error = String(err);
        });
      }
    });
    ro.observe(viewport);
    resizeObserver = ro;
    window.addEventListener("pointerup", globalPointerUp);
    window.addEventListener("pointercancel", globalPointerCancel);

    void activateSession(sessionId);
  });

  onDestroy(() => {
    disposed = true;
    mounted = false;
    resizeObserver?.disconnect();
    resizeObserver = null;
    window.removeEventListener("pointerup", globalPointerUp);
    window.removeEventListener("pointercancel", globalPointerCancel);
    clearCursorTimer();
    disposeActiveSubscriptions();
    for (const dispose of disposers) {
      try {
        dispose();
      } catch {
        /* ignore */
      }
    }
    disposers = [];
  });

  $effect(() => {
    if (!mounted || disposed || activeRootSessionId === sessionId) return;
    void activateSession(sessionId);
  });

  function newBrowserTab(id: string, label: string): BrowserTab {
    return {
      id,
      label,
      url: "about:blank",
      title: "",
      loading: false,
      error: null,
      status: "Starting Chrome...",
      connected: false,
      favicon: "",
      frame: null,
      devtools: []
    };
  }

  function tabFromInfo(info: BrowserTabInfo): BrowserTab {
    const existing = tabs.find((tab) => tab.id === info.tabId);
    return {
      ...(existing ?? newBrowserTab(info.tabId, info.label || "New tab")),
      id: info.tabId,
      label: info.label || existing?.label || "New tab",
      url: info.url || "about:blank",
      title: info.title || "",
      loading: info.loading,
      status: info.connected ? (info.loading ? "Loading" : "Connected") : "Disconnected",
      connected: info.connected,
      favicon: faviconFor(info.url || "about:blank"),
      error: null
    };
  }

  function applyTabsState(state: BrowserTabsState, options: ApplyTabsOptions = {}) {
    if (!Array.isArray(state.tabs)) return;
    if (state.tabs.length === 0) {
      if (!options.allowEmpty) return;
      tabStateVersion += 1;
      tabs = [];
      activeTabId = "";
      nextTabNumber = 2;
      saveTabs([]);
      connected = false;
      loading = false;
      error = null;
      status = "No pages";
      title = "";
      currentUrl = "about:blank";
      urlDraft = "about:blank";
      resetPointer(activePointerId ?? undefined);
      disposeActiveSubscriptions();
      clearCanvas();
      return;
    }
    tabStateVersion += 1;
    const previousActiveTabId = activeTabId;
    const nextTabs = state.tabs.map(tabFromInfo);
    const connectedTabId = nextTabs.find((tab) => tab.connected)?.id;
    const validActiveTabId = state.activeTabId && nextTabs.some((tab) => tab.id === state.activeTabId)
      ? state.activeTabId
      : null;
    tabs = nextTabs;
    activeTabId = validActiveTabId || connectedTabId || nextTabs[0].id;
    nextTabNumber = nextTabIndex(nextTabs);
    saveTabs(nextTabs);
    syncFromActiveTab();
    if (!connected || activeTabId !== previousActiveTabId) {
      resetPointer(activePointerId ?? undefined);
    }
  }

  async function activateSession(nextSessionId: string) {
    const generation = ++sessionGeneration;
    activeRootSessionId = nextSessionId;
    activeEventSessionId = "";
    disposeSessionSubscriptions();
    clearCursorTimer();
    resetPointer(activePointerId ?? undefined);
    const restored = loadSavedTabsFor(nextSessionId);
    tabs = restored;
    activeTabId = restored[0]?.id ?? "";
    nextTabNumber = nextTabIndex(restored);
    tabStateVersion += 1;
    syncFromActiveTab();
    if (activeTab?.frame) {
      renderFrame(activeTab.frame);
    } else {
      clearCanvas();
    }
    await syncDaemonTabs(generation, nextSessionId);
    if (generation !== sessionGeneration || disposed) return;
    const client = await ensureLocalDaemonClient();
    if (generation !== sessionGeneration || disposed) return;
    disposers.push(
      client.on<BrowserTabsState>(`browser:${nextSessionId}:tabs`, (next) => {
        if (generation !== sessionGeneration || activeRootSessionId !== nextSessionId) return;
        const previousActiveTabId = activeTabId;
        applyTabsState(next, { allowEmpty: true });
        if (activeTabId && (activeTabId !== previousActiveTabId || !connected)) {
          void connectActiveTab(generation);
        }
      })
    );

    if (activeTabId) await connectActiveTab(generation);
  }

  function disposeSessionSubscriptions() {
    disposeActiveSubscriptions();
    for (const dispose of disposers) {
      try {
        dispose();
      } catch {
        /* ignore */
      }
    }
    disposers = [];
  }

  async function syncDaemonTabs(generation = sessionGeneration, targetSessionId = sessionId) {
    try {
      const state = await browserTabsList(targetSessionId);
      if (generation !== sessionGeneration || activeRootSessionId !== targetSessionId) return;
      applyTabsState(state);
    } catch {
      /* Local saved tabs remain the migration fallback. */
    }
  }

  function storageKeyFor(value: string): string {
    return `puffer-browser-tabs:${value}`;
  }

  function storageKey(): string {
    return storageKeyFor(sessionId);
  }

  function loadSavedTabsFor(value: string): BrowserTab[] {
    if (typeof window === "undefined") return [newBrowserTab("tab-1", "New tab")];
    try {
      const raw = window.localStorage.getItem(storageKeyFor(value));
      const saved = raw ? JSON.parse(raw) : null;
      if (!Array.isArray(saved?.tabs)) return [newBrowserTab("tab-1", "New tab")];
      const restored = saved.tabs
        .filter((tab: Partial<BrowserTab>) => typeof tab.id === "string")
        .map((tab: Partial<BrowserTab>) => ({
          ...newBrowserTab(tab.id!, tab.label || "New tab"),
          url: tab.url || "about:blank",
          title: tab.title || "",
          status: "Disconnected",
          connected: false,
          favicon: tab.favicon || faviconFor(tab.url || "about:blank")
        }));
      return restored.length ? restored : [newBrowserTab("tab-1", "New tab")];
    } catch {
      return [newBrowserTab("tab-1", "New tab")];
    }
  }

  function saveTabs(nextTabs = tabs) {
    if (typeof window === "undefined") return;
    window.localStorage.setItem(
      storageKey(),
      JSON.stringify({
        tabs: nextTabs.map(({ id, label, url, title, favicon }) => ({
          id,
          label,
          url,
          title,
          favicon
        }))
      })
    );
  }

  function nextTabIndex(values: BrowserTab[]): number {
    return values.reduce((next, tab) => {
      const match = /^tab-(\d+)$/.exec(tab.id);
      return match ? Math.max(next, Number(match[1]) + 1) : next;
    }, 2);
  }

  function faviconFor(url: string): string {
    try {
      const parsed = new URL(url);
      if (!["http:", "https:"].includes(parsed.protocol)) return "";
      return `${parsed.origin}/favicon.ico`;
    } catch {
      return "";
    }
  }

  function activeBackendSessionId(): string {
    return `${sessionId}:browser:${activeTabId}`;
  }

  function backendSessionId(tabId: string): string {
    return `${sessionId}:browser:${tabId}`;
  }

  function updateTab(tabId: string, patch: Partial<BrowserTab>, persist = true) {
    tabs = tabs.map((tab) => (tab.id === tabId ? { ...tab, ...patch } : tab));
    if (persist) saveTabs();
  }

  function syncFromActiveTab() {
    const tab = activeTab;
    if (!tab) return;
    urlDraft = tab.url;
    currentUrl = tab.url;
    title = tab.title;
    status = tab.status;
    error = tab.error;
    loading = tab.loading;
    connected = tab.connected;
  }

  async function connectActiveTab(generation = sessionGeneration) {
    if (!mounted || !viewport || !canvas || disposed || !activeTabId || !activeTab) return;
    disposeActiveSubscriptions();
    syncFromActiveTab();
    if (activeTab.frame) {
      renderFrame(activeTab.frame);
    } else {
      clearCanvas();
    }
    clearCursorTimer();
    browserCursorStyle = "default";
    const tabId = activeTabId;
    const eventSessionId = backendSessionId(tabId);
    const shouldOpen = !activeTab.connected;
    activeEventSessionId = eventSessionId;
    try {
      const client = await ensureLocalDaemonClient();
      if (generation !== sessionGeneration || activeEventSessionId !== eventSessionId) return;
      activeDisposers = [
        client.on<BrowserFrameEvent>(`browser:${eventSessionId}:frame`, (frame) => {
          if (activeEventSessionId === eventSessionId) drawFrame(frame);
        }),
        client.on<BrowserState>(`browser:${eventSessionId}:state`, (next) => {
          if (activeEventSessionId === eventSessionId) applyState(next, tabId);
        }),
        client.on<BrowserDevtoolsEvent>(`browser:${eventSessionId}:devtools`, (item) => {
          addDevtoolsEvent(tabId, item);
        })
      ];
      const size = measureViewport() ?? lastResize;
      lastResize = size;
      if (shouldOpen) {
        const next = await browserOpen({ sessionId: eventSessionId, url: activeTab.url, ...size });
        if (generation !== sessionGeneration || activeEventSessionId !== eventSessionId) return;
        applyState(next, tabId);
      } else {
        try {
          await browserResize(eventSessionId, size.width, size.height);
        } catch {
          const next = await browserOpen({
            sessionId: eventSessionId,
            url: activeTab.url,
            ...size
          });
          if (generation !== sessionGeneration || activeEventSessionId !== eventSessionId) return;
          applyState(next, tabId);
        }
      }
      if (generation !== sessionGeneration || activeEventSessionId !== eventSessionId) return;
      updateTab(tabId, { connected: true, status: "Connected", error: null });
      if (activeTabId === tabId && activeEventSessionId === eventSessionId) {
        connected = true;
        status = "Connected";
        error = null;
      }
    } catch (err) {
      if (generation !== sessionGeneration || activeEventSessionId !== eventSessionId) return;
      const message = String(err);
      updateTab(tabId, { connected: false, status: "Chrome failed to start", error: message });
      if (activeTabId === tabId && activeEventSessionId === eventSessionId) {
        error = message;
        status = "Chrome failed to start";
      }
    }
  }

  function disposeActiveSubscriptions() {
    for (const dispose of activeDisposers) {
      try {
        dispose();
      } catch {
        /* ignore */
      }
    }
    activeDisposers = [];
  }

  function measureViewport(): { width: number; height: number } | null {
    if (!viewport) return null;
    const rect = viewport.getBoundingClientRect();
    if (rect.width < 1 || rect.height < 1) return null;
    return {
      width: Math.max(1, Math.round(rect.width)),
      height: Math.max(1, Math.round(rect.height))
    };
  }

  function applyState(next: BrowserState, tabId = activeTabId) {
    if (disposed || !tabId) return;
    const existing = tabs.find((tab) => tab.id === tabId);
    const nextUrl = next.url || existing?.url || "about:blank";
    const nextTitle = next.title ?? "";
    const nextError = next.error ?? null;
    const nextStatus = nextError ? "Chrome error" : next.loading ? "Loading" : "Connected";
    updateTab(tabId, {
      url: nextUrl,
      title: nextTitle,
      loading: next.loading,
      error: nextError,
      status: nextStatus,
      connected: !nextError,
      favicon: faviconFor(nextUrl)
    });
    if (tabId !== activeTabId) return;
    currentUrl = nextUrl;
    urlDraft = nextUrl;
    title = nextTitle;
    loading = next.loading;
    error = nextError;
    status = nextStatus;
  }

  function drawFrame(frame: BrowserFrameEvent) {
    renderFrame(frame);
    updateTab(activeTabId, { frame }, false);
  }

  function renderFrame(frame: BrowserFrameEvent) {
    if (!canvas || disposed) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    frameWidth = Math.max(1, frame.width);
    frameHeight = Math.max(1, frame.height);
    if (canvas.width !== frameWidth) canvas.width = frameWidth;
    if (canvas.height !== frameHeight) canvas.height = frameHeight;
    const serial = ++drawSerial;
    const image = new Image();
    image.onload = () => {
      if (disposed || serial !== drawSerial) return;
      ctx.drawImage(image, 0, 0, frameWidth, frameHeight);
    };
    image.src = `data:${frame.mimeType};base64,${frame.data}`;
  }

  function clearCanvas() {
    drawSerial += 1;
    if (!canvas) return;
    const ctx = canvas.getContext("2d");
    if (!ctx) return;
    ctx.clearRect(0, 0, canvas.width, canvas.height);
  }

  async function submitUrl(event: SubmitEvent) {
    event.preventDefault();
    if (!connected || !activeTabId) return;
    error = null;
    try {
      updateTab(activeTabId, {
        url: urlDraft,
        status: "Loading",
        loading: true,
        favicon: faviconFor(urlDraft)
      });
      await browserNavigate(activeBackendSessionId(), urlDraft);
    } catch (err) {
      error = String(err);
      updateTab(activeTabId, { error, status: "Chrome error" });
    }
  }

  function openExternal() {
    const url = currentUrl && currentUrl !== "about:blank" ? currentUrl : urlDraft;
    if (url && url !== "about:blank") {
      window.open(url, "_blank", "noopener,noreferrer");
    }
  }

  async function addTab() {
    const size = measureViewport() ?? lastResize;
    const tabId = `tab-${nextTabNumber}`;
    nextTabNumber += 1;
    const requestedAtVersion = tabStateVersion;
    const requestedAtGeneration = sessionGeneration;
    try {
      const info = await browserTabOpen({
        sessionId,
        tabId,
        url: "about:blank",
        width: size.width,
        height: size.height,
        activate: true
      });
      if (
        disposed ||
        requestedAtVersion !== tabStateVersion ||
        requestedAtGeneration !== sessionGeneration
      ) return;
      const tab = tabFromInfo(info);
      tabs = [...tabs.filter((item) => item.id !== tab.id), tab];
      activeTabId = tab.id;
      nextTabNumber = nextTabIndex(tabs);
      saveTabs();
      syncFromActiveTab();
      void connectActiveTab(requestedAtGeneration);
    } catch (err) {
      error = String(err);
    }
  }

  function selectTab(tabId: string) {
    if (tabId === activeTabId) return;
    activeTabId = tabId;
    syncFromActiveTab();
    if (activeTab?.frame) renderFrame(activeTab.frame);
    void browserTabFocus(sessionId, tabId).catch((err) => {
      error = String(err);
    });
    void connectActiveTab();
  }

  function closeTab(tabId: string, event: Event) {
    event.stopPropagation();
    void browserTabClose(sessionId, tabId)
      .then((state) => {
        applyTabsState(state, { allowEmpty: true });
      })
      .catch(() => browserClose(backendSessionId(tabId)).catch(() => {}));
    const index = tabs.findIndex((tab) => tab.id === tabId);
    const nextTabs = tabs.filter((tab) => tab.id !== tabId);
    tabStateVersion += 1;
    tabs = nextTabs;
    saveTabs(nextTabs);
    if (nextTabs.length === 0) {
      activeTabId = "";
      connected = false;
      loading = false;
      error = null;
      status = "No pages";
      title = "";
      currentUrl = "about:blank";
      urlDraft = "about:blank";
      disposeActiveSubscriptions();
      clearCanvas();
      return;
    }
    if (tabId === activeTabId) {
      activeTabId = nextTabs[Math.max(0, index - 1)]?.id ?? nextTabs[0].id;
      syncFromActiveTab();
      void connectActiveTab();
    }
  }

  function tabTitle(tab: BrowserTab): string {
    const value = tab.title || (tab.url === "about:blank" ? tab.label : tab.url);
    return value.length > 28 ? `${value.slice(0, 25)}...` : value;
  }

  function canvasPoint(event: MouseEvent | WheelEvent): { x: number; y: number } {
    if (!canvas) return { x: 0, y: 0 };
    const rect = canvas.getBoundingClientRect();
    const point = {
      x: ((event.clientX - rect.left) * frameWidth) / Math.max(1, rect.width),
      y: ((event.clientY - rect.top) * frameHeight) / Math.max(1, rect.height)
    };
    return {
      x: Math.max(0, Math.min(frameWidth, point.x)),
      y: Math.max(0, Math.min(frameHeight, point.y))
    };
  }

  function pointerButton(button: number): "left" | "middle" | "right" | "none" {
    if (button === 0) return "left";
    if (button === 1) return "middle";
    if (button === 2) return "right";
    return "none";
  }

  function pointerButtons(button: "left" | "middle" | "right" | "none"): number {
    if (button === "left") return 1;
    if (button === "right") return 2;
    if (button === "middle") return 4;
    return 0;
  }

  function nextClickCount(point: { x: number; y: number }, button: "left" | "middle" | "right" | "none"): number {
    const now = performance.now();
    const dx = point.x - lastClick.x;
    const dy = point.y - lastClick.y;
    const sameTarget = button === lastClick.button && Math.hypot(dx, dy) <= 8;
    const count = sameTarget && now - lastClick.at <= 500 ? Math.min(lastClick.count + 1, 3) : 1;
    lastClick = { at: now, x: point.x, y: point.y, button, count };
    return count;
  }

  function sendMouse(event: PointerEvent, eventType: "mousePressed" | "mouseReleased" | "mouseMoved") {
    if (!connected || !activeTabId) {
      if (eventType === "mouseReleased") resetPointer(event.pointerId);
      return;
    }
    event.preventDefault();
    canvas?.focus();
    const point = canvasPoint(event);
    let button = pointerButton(event.button);
    let buttons = event.buttons;
    let click_count = 0;
    if (eventType === "mousePressed") {
      activePointerId = event.pointerId;
      activeButton = button;
      activeButtons = pointerButtons(button);
      activeClickCount = nextClickCount(point, button);
      buttons = activeButtons;
      click_count = activeClickCount;
      try {
        canvas?.setPointerCapture(event.pointerId);
      } catch {
        /* ignore */
      }
    } else if (eventType === "mouseMoved") {
      button = activeButtons > 0 ? activeButton : "none";
      buttons = activeButtons || buttons || 0;
      if (buttons === 0) {
        scheduleCursorProbe(point);
      }
    } else if (eventType === "mouseReleased") {
      button = activeButton !== "none" ? activeButton : button;
      buttons = 0;
      click_count = activeClickCount || 1;
    }
    void browserInput(activeBackendSessionId(), {
      kind: "mouse",
      eventType,
      x: point.x,
      y: point.y,
      button,
      buttons,
      clickCount: click_count
    }).catch((err) => {
      error = String(err);
    });
    if (eventType === "mouseReleased") {
      resetPointer(event.pointerId);
    }
  }

  function resetPointer(pointerId?: number) {
    if (pointerId !== undefined) {
      try {
        canvas?.releasePointerCapture(pointerId);
      } catch {
        /* ignore */
      }
    }
    activePointerId = null;
    activeButton = "none";
    activeButtons = 0;
    activeClickCount = 0;
  }

  function pointerCancel(event: PointerEvent) {
    if (activePointerId === event.pointerId) {
      sendMouse(event, "mouseReleased");
    }
  }

  function lostPointerCapture(event: PointerEvent) {
    if (activePointerId === event.pointerId && activeButtons > 0) {
      sendMouse(event, "mouseReleased");
    }
  }

  function globalPointerUp(event: PointerEvent) {
    if (activePointerId === event.pointerId && activeButtons > 0) {
      sendMouse(event, "mouseReleased");
    }
  }

  function globalPointerCancel(event: PointerEvent) {
    if (activePointerId === event.pointerId && activeButtons > 0) {
      sendMouse(event, "mouseReleased");
    }
  }

  function scheduleCursorProbe(point: { x: number; y: number }) {
    pendingCursorPoint = point;
    pendingCursorSessionId = connected && activeTabId ? activeBackendSessionId() : null;
    if (cursorTimer) return;
    cursorTimer = setTimeout(() => {
      cursorTimer = null;
      const next = pendingCursorPoint;
      const sessionId = pendingCursorSessionId;
      pendingCursorPoint = null;
      pendingCursorSessionId = null;
      if (!next || !sessionId || disposed || activeButtons > 0) return;
      if (!connected || !activeTabId || activeBackendSessionId() !== sessionId) return;
      void probeCursor(sessionId, next);
    }, 60);
  }

  async function probeCursor(sessionId: string, point: { x: number; y: number }) {
    const request = ++cursorRequest;
    try {
      const result = await browserCursor(sessionId, point.x, point.y);
      if (
        disposed ||
        request !== cursorRequest ||
        activeButtons > 0 ||
        !connected ||
        !activeTabId ||
        activeBackendSessionId() !== sessionId
      ) return;
      browserCursorStyle = result.cursor || "default";
    } catch {
      if (request === cursorRequest) browserCursorStyle = "default";
    }
  }

  function clearCursorTimer() {
    if (cursorTimer) {
      clearTimeout(cursorTimer);
      cursorTimer = null;
    }
    pendingCursorPoint = null;
    pendingCursorSessionId = null;
  }

  function sendWheel(event: WheelEvent) {
    if (!connected) return;
    event.preventDefault();
    const point = canvasPoint(event);
    void browserInput(activeBackendSessionId(), {
      kind: "wheel",
      x: point.x,
      y: point.y,
      deltaX: event.deltaX,
      deltaY: event.deltaY
    }).catch((err) => {
      error = String(err);
    });
  }

  function modifiers(event: KeyboardEvent): number {
    return (event.altKey ? 1 : 0) |
      (event.ctrlKey ? 2 : 0) |
      (event.metaKey ? 4 : 0) |
      (event.shiftKey ? 8 : 0);
  }

  function keyType(event: KeyboardEvent): "keyDown" | "rawKeyDown" {
    return event.key.length === 1 ? "keyDown" : "rawKeyDown";
  }

  function isCopyShortcut(event: KeyboardEvent): boolean {
    return (event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "c";
  }

  function keyDown(event: KeyboardEvent) {
    if (!connected) return;
    if (isCopyShortcut(event)) {
      event.preventDefault();
      void copySelection();
      return;
    }
    if ((event.metaKey || event.ctrlKey) && ["r", "l", "w"].includes(event.key.toLowerCase())) {
      return;
    }
    event.preventDefault();
    const text = event.key.length === 1 && !event.metaKey && !event.ctrlKey ? event.key : undefined;
    void browserInput(activeBackendSessionId(), {
      kind: "key",
      eventType: keyType(event),
      key: event.key,
      code: event.code,
      ...(text ? { text } : {}),
      modifiers: modifiers(event)
    }).catch((err) => {
      error = String(err);
    });
  }

  function keyUp(event: KeyboardEvent) {
    if (!connected) return;
    if (isCopyShortcut(event)) {
      event.preventDefault();
      return;
    }
    event.preventDefault();
    void browserInput(activeBackendSessionId(), {
      kind: "key",
      eventType: "keyUp",
      key: event.key,
      code: event.code,
      modifiers: modifiers(event)
    }).catch((err) => {
      error = String(err);
    });
  }

  function paste(event: ClipboardEvent) {
    if (!connected || !activeTabId) return;
    const text = event.clipboardData?.getData("text/plain") ?? "";
    if (!text) return;
    event.preventDefault();
    void browserInput(activeBackendSessionId(), { kind: "text", text }).catch((err) => {
      error = String(err);
    });
  }

  async function copySelection() {
    try {
      const result = await browserCopySelection(activeBackendSessionId());
      if (!result.text) {
        status = "No selection";
        return;
      }
      await navigator.clipboard.writeText(result.text);
      error = null;
      status = "Copied selected text";
    } catch (err) {
      error = String(err);
    }
  }

  function addDevtoolsEvent(tabId: string, item: BrowserDevtoolsEvent) {
    const tab = tabs.find((candidate) => candidate.id === tabId);
    if (!tab) return;
    updateTab(tabId, { devtools: [...tab.devtools, item].slice(-400) });
  }

  function clearDevtools() {
    updateTab(activeTabId, { devtools: [] });
  }

  function eventUrl(item: BrowserDevtoolsEvent): string {
    return "url" in item && item.url ? item.url : "";
  }
</script>

<div class="pf-browser-pane">
  <div class="pf-browser-tabs" role="tablist" aria-label="Browser tabs">
    {#each tabs as tab (tab.id)}
      <div
        class="pf-browser-tab"
        class:active={tab.id === activeTabId}
        role="tab"
        tabindex="0"
        aria-selected={tab.id === activeTabId}
        title={tab.title || tab.url}
        onclick={() => selectTab(tab.id)}
        onkeydown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            selectTab(tab.id);
          }
        }}
      >
        {#if tab.favicon}
          <img class="favicon" src={tab.favicon} alt="" onerror={(event) => ((event.currentTarget as HTMLImageElement).style.display = "none")} />
        {:else}
          <span class="dot" class:loading={tab.loading}></span>
        {/if}
        <span class="label">{tabTitle(tab)}</span>
        <button
          class="close"
          type="button"
          title="Close tab"
          aria-label="Close tab"
          onclick={(event) => closeTab(tab.id, event)}
        >
          <Icon name="x" size={11} />
        </button>
      </div>
    {/each}
    <button class="pf-browser-tab-add" type="button" title="New tab" onclick={() => void addTab()}>
      <Icon name="plus" size={13} />
    </button>
  </div>
  <form class="pf-browser-toolbar" onsubmit={submitUrl}>
    <button
      class="pf-browser-icon"
      type="button"
      title="Back"
      disabled={!browserControlsEnabled}
      onclick={() => browserHistory(activeBackendSessionId(), "back").catch((err) => (error = String(err)))}
    >
      <Icon name="chevL" size={14} />
    </button>
    <button
      class="pf-browser-icon"
      type="button"
      title="Forward"
      disabled={!browserControlsEnabled}
      onclick={() => browserHistory(activeBackendSessionId(), "forward").catch((err) => (error = String(err)))}
    >
      <Icon name="chevR" size={14} />
    </button>
    <button
      class="pf-browser-icon"
      type="button"
      title="Reload"
      disabled={!browserControlsEnabled}
      onclick={() => browserReload(activeBackendSessionId()).catch((err) => (error = String(err)))}
    >
      <Icon name="refresh" size={14} />
    </button>
    <input
      class="pf-browser-address"
      aria-label="URL"
      spellcheck="false"
      disabled={!browserControlsEnabled}
      bind:value={urlDraft}
    />
    <button
      class="pf-browser-icon"
      class:active={showDevtools}
      type="button"
      title="DevTools"
      disabled={!activeTab}
      onclick={() => (showDevtools = !showDevtools)}
    >
      <Icon name="terminal" size={14} />
    </button>
    <button class="pf-browser-icon" type="button" title="Open externally" disabled={!activeTab} onclick={openExternal}>
      <Icon name="external" size={14} />
    </button>
    <span class="pf-browser-status" class:loading>{status}</span>
  </form>
  {#if error}
    <div class="pf-browser-error">{error}</div>
  {/if}
  <div class="pf-browser-workspace" class:withDevtools={showDevtools}>
    <div class="pf-browser-viewport" bind:this={viewport}>
      <canvas
        class="pf-browser-canvas"
        bind:this={canvas}
        tabindex="0"
        onpointerdown={(event) => sendMouse(event, "mousePressed")}
        onpointerup={(event) => sendMouse(event, "mouseReleased")}
        onpointermove={(event) => sendMouse(event, "mouseMoved")}
        onpointercancel={pointerCancel}
        onlostpointercapture={lostPointerCapture}
        oncontextmenu={(event) => event.preventDefault()}
        onwheel={sendWheel}
        onkeydown={keyDown}
        onkeyup={keyUp}
        onpaste={paste}
        style:cursor={browserCursorStyle}
      ></canvas>
      {#if !activeTab}
        <div class="pf-browser-empty">
          <button class="pf-browser-empty-action" type="button" onclick={() => void addTab()}>New tab</button>
        </div>
      {:else if !connected && !error}
        <div class="pf-browser-empty">Starting Chrome...</div>
      {/if}
    </div>
    {#if showDevtools}
      <aside class="pf-browser-devtools">
        <div class="pf-browser-devtools-head">
          <div class="pf-browser-devtools-tabs">
            <button
              type="button"
              class:active={devtoolsView === "console"}
              onclick={() => (devtoolsView = "console")}
            >Console</button>
            <button
              type="button"
              class:active={devtoolsView === "network"}
              onclick={() => (devtoolsView = "network")}
            >Network</button>
          </div>
          <button class="pf-browser-icon flat" type="button" title="Clear" onclick={clearDevtools}>
            <Icon name="x" size={12} />
          </button>
        </div>
        <div class="pf-browser-devtools-body">
          {#if devtoolsView === "console"}
            {#if consoleEvents.length === 0}
              <div class="pf-browser-devtools-empty">No console events.</div>
            {:else}
              {#each consoleEvents as item, index (`console-${index}`)}
                <div class="pf-browser-console-row" data-level={item.level}>
                  <span class="level">{item.level}</span>
                  <span class="message">{item.text || eventUrl(item)}</span>
                </div>
              {/each}
            {/if}
          {:else}
            {#if networkEvents.length === 0}
              <div class="pf-browser-devtools-empty">No network events.</div>
            {:else}
              {#each networkEvents as item, index (`network-${index}`)}
                <div class="pf-browser-network-row" data-phase={item.phase}>
                  <span class="phase">{item.phase}</span>
                  <span class="status">{item.status ?? item.method ?? ""}</span>
                  <span class="url">{item.url ?? item.errorText ?? item.requestId}</span>
                </div>
              {/each}
            {/if}
          {/if}
        </div>
      </aside>
    {/if}
  </div>
  {#if title}
    <div class="pf-browser-title">{title}</div>
  {/if}
</div>

<style>
  .pf-browser-pane {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--background);
  }

  .pf-browser-tabs {
    height: 34px;
    flex-shrink: 0;
    display: flex;
    align-items: end;
    gap: 2px;
    padding: 4px 8px 0;
    border-bottom: 1px solid var(--border);
    background: color-mix(in oklab, var(--muted) 45%, var(--background));
    overflow-x: auto;
  }

  .pf-browser-tab,
  .pf-browser-tab-add {
    height: 29px;
    border: 1px solid transparent;
    border-bottom: 0;
    background: transparent;
    color: var(--muted-foreground);
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
    flex-shrink: 0;
  }

  .pf-browser-tab {
    max-width: 190px;
    min-width: 104px;
    padding: 0 8px;
    border-radius: 6px 6px 0 0;
  }

  .pf-browser-tab.active {
    background: var(--background);
    color: var(--foreground);
    border-color: var(--border);
  }

  .pf-browser-tab .label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 12px;
  }

  .pf-browser-tab .dot {
    width: 7px;
    height: 7px;
    border-radius: 999px;
    background: var(--muted-foreground);
    opacity: 0.45;
    flex-shrink: 0;
  }

  .pf-browser-tab .favicon {
    width: 14px;
    height: 14px;
    border-radius: 3px;
    object-fit: contain;
    flex-shrink: 0;
  }

  .pf-browser-tab .dot.loading {
    background: var(--ring);
    opacity: 1;
  }

  .pf-browser-tab .close {
    width: 18px;
    height: 18px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    margin-left: auto;
    color: var(--muted-foreground);
    cursor: pointer;
  }

  .pf-browser-tab .close:hover,
  .pf-browser-tab-add:hover {
    background: var(--accent);
    color: var(--foreground);
  }

  .pf-browser-tab-add {
    width: 30px;
    justify-content: center;
    border-radius: 6px 6px 0 0;
  }

  .pf-browser-toolbar {
    height: 42px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 6px 10px;
    border-bottom: 1px solid var(--border);
    background: var(--background);
  }

  .pf-browser-icon {
    width: 30px;
    height: 30px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    display: inline-flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    flex-shrink: 0;
  }

  .pf-browser-icon:hover,
  .pf-browser-icon.active {
    background: var(--accent);
  }

  .pf-browser-icon:disabled,
  .pf-browser-address:disabled {
    opacity: 0.45;
    cursor: default;
  }

  .pf-browser-icon.flat {
    border-color: transparent;
    background: transparent;
  }

  .pf-browser-address {
    flex: 1;
    min-width: 80px;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--input);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    font-family: var(--font-mono);
    font-size: 12px;
    letter-spacing: 0;
    outline: none;
  }

  .pf-browser-address:focus {
    border-color: var(--ring);
  }

  .pf-browser-status {
    width: 110px;
    color: var(--muted-foreground);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .pf-browser-status.loading {
    color: var(--foreground);
  }

  .pf-browser-error {
    flex-shrink: 0;
    padding: 8px 10px;
    border-bottom: 1px solid color-mix(in oklab, var(--destructive) 25%, var(--border));
    color: var(--destructive);
    background: color-mix(in oklab, var(--destructive) 8%, var(--background));
    font-size: 12px;
  }

  .pf-browser-workspace {
    flex: 1;
    min-height: 0;
    display: grid;
    grid-template-columns: minmax(0, 1fr);
  }

  .pf-browser-workspace.withDevtools {
    grid-template-columns: minmax(0, 1fr) minmax(280px, 34%);
  }

  .pf-browser-viewport {
    min-height: 0;
    position: relative;
    overflow: hidden;
    background: #f5f5f5;
  }

  .pf-browser-canvas {
    width: 100%;
    height: 100%;
    display: block;
    outline: none;
    background: white;
  }

  .pf-browser-empty {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    color: var(--muted-foreground);
    font-size: 13px;
    pointer-events: none;
  }

  .pf-browser-empty-action {
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--foreground);
    cursor: pointer;
    pointer-events: auto;
  }

  .pf-browser-devtools {
    min-width: 0;
    border-left: 1px solid var(--border);
    background: var(--background);
    display: flex;
    flex-direction: column;
  }

  .pf-browser-devtools-head {
    height: 36px;
    flex-shrink: 0;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    padding: 4px 6px;
    border-bottom: 1px solid var(--border);
  }

  .pf-browser-devtools-tabs {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .pf-browser-devtools-tabs button {
    height: 26px;
    padding: 0 8px;
    border: 1px solid transparent;
    border-radius: 5px;
    background: transparent;
    color: var(--muted-foreground);
    font-size: 12px;
    cursor: pointer;
  }

  .pf-browser-devtools-tabs button.active {
    border-color: var(--border);
    background: var(--accent);
    color: var(--foreground);
  }

  .pf-browser-devtools-body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    font-family: var(--font-mono);
    font-size: 11px;
  }

  .pf-browser-devtools-empty {
    padding: 12px;
    color: var(--muted-foreground);
    font-family: var(--font-sans);
    font-size: 12px;
  }

  .pf-browser-console-row,
  .pf-browser-network-row {
    display: grid;
    gap: 8px;
    padding: 5px 8px;
    border-bottom: 1px solid var(--border);
    align-items: start;
  }

  .pf-browser-console-row {
    grid-template-columns: 58px minmax(0, 1fr);
  }

  .pf-browser-network-row {
    grid-template-columns: 56px 48px minmax(0, 1fr);
  }

  .pf-browser-console-row[data-level="error"],
  .pf-browser-network-row[data-phase="failed"] {
    color: var(--destructive);
  }

  .pf-browser-console-row .level,
  .pf-browser-network-row .phase,
  .pf-browser-network-row .status {
    color: var(--muted-foreground);
    white-space: nowrap;
  }

  .pf-browser-console-row .message,
  .pf-browser-network-row .url {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .pf-browser-title {
    flex-shrink: 0;
    height: 24px;
    display: flex;
    align-items: center;
    padding: 0 10px;
    border-top: 1px solid var(--border);
    color: var(--muted-foreground);
    font-size: 12px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
