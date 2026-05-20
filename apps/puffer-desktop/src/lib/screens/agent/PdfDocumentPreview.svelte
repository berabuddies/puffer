<script lang="ts">
  import { onDestroy } from "svelte";
  import RotateCcwIcon from "lucide-svelte/icons/rotate-ccw";
  import ZoomInIcon from "lucide-svelte/icons/zoom-in";
  import ZoomOutIcon from "lucide-svelte/icons/zoom-out";
  import type { PDFDocumentLoadingTask } from "pdfjs-dist";

  type Props = {
    base64: string;
    textLines?: string[];
  };

  const PDF_RENDER_SCALE = 1.35;
  const PDF_MIN_ZOOM = 0.5;
  const PDF_MAX_ZOOM = 2;
  const PDF_ZOOM_STEP = 0.1;

  let { base64, textLines = [] }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let renderer = $state<HTMLDivElement | null>(null);
  let status = $state("Loading PDF...");
  let error = $state<string | null>(null);
  let renderedPages = $state(0);
  let zoom = $state(1);
  let zoomPercent = $derived(Math.round(zoom * 100));
  let generation = 0;
  let loadingTask: PDFDocumentLoadingTask | null = null;
  let showTextFallback = $derived(textLines.some((line) => line.trim() && line.trim() !== "No text found."));

  type PdfJsModule = typeof import("pdfjs-dist");
  type PdfWorkerModule = { WorkerMessageHandler: unknown };
  type PdfWorkerGlobal = typeof globalThis & { pdfjsWorker?: PdfWorkerModule };

  let pdfJsModulePromise: Promise<PdfJsModule> | null = null;

  function loadPdfJs(): Promise<PdfJsModule> {
    pdfJsModulePromise ??= Promise.all([
      import("pdfjs-dist/legacy/build/pdf.mjs"),
      import("pdfjs-dist/legacy/build/pdf.worker.mjs")
    ]).then(([module, worker]) => {
      (globalThis as PdfWorkerGlobal).pdfjsWorker = worker as PdfWorkerModule;
      return module as unknown as PdfJsModule;
    });
    return pdfJsModulePromise;
  }

  $effect(() => {
    const target = host;
    const source = base64;
    if (!target || !source) return;

    const current = ++generation;
    renderPdf(target, source, current);
  });

  let lastSource = $state("");
  $effect(() => {
    const source = base64;
    if (source === lastSource) return;
    lastSource = source;
    zoom = 1;
  });

  $effect(() => {
    const target = host;
    const currentZoom = zoom;
    if (!target) return;
    applyZoom(target, currentZoom);
  });

  onDestroy(() => {
    generation += 1;
    void loadingTask?.destroy();
    loadingTask = null;
  });

  function base64ToBytes(value: string): Uint8Array {
    const binary = atob(value);
    const bytes = new Uint8Array(binary.length);
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index);
    }
    return bytes;
  }

  function setZoom(next: number): void {
    zoom = Math.max(PDF_MIN_ZOOM, Math.min(PDF_MAX_ZOOM, Math.round(next * 10) / 10));
  }

  function handleZoomInput(event: Event): void {
    const input = event.currentTarget as HTMLInputElement;
    setZoom(Number(input.value) / 100);
  }

  function handleZoomWheel(event: WheelEvent): void {
    if (!event.metaKey && !event.ctrlKey) return;
    event.preventDefault();
    const direction = event.deltaY < 0 ? 1 : -1;
    setZoom(zoom + PDF_ZOOM_STEP * direction);
  }

  function handleZoomKeydown(event: KeyboardEvent): void {
    if (!event.metaKey && !event.ctrlKey) return;
    if (event.key === "+" || event.key === "=") {
      event.preventDefault();
      setZoom(zoom + PDF_ZOOM_STEP);
    } else if (event.key === "-") {
      event.preventDefault();
      setZoom(zoom - PDF_ZOOM_STEP);
    } else if (event.key === "0") {
      event.preventDefault();
      setZoom(1);
    }
  }

  function handleWindowKeydown(event: KeyboardEvent): void {
    const active = document.activeElement;
    if (!renderer || !active || !renderer.contains(active)) return;
    handleZoomKeydown(event);
  }

  function applyCanvasZoom(canvas: HTMLCanvasElement, currentZoom: number): void {
    const baseWidth = Number(canvas.dataset.pdfBaseWidth ?? 0);
    const baseHeight = Number(canvas.dataset.pdfBaseHeight ?? 0);
    if (!baseWidth || !baseHeight) return;
    canvas.style.width = `${Math.floor(baseWidth * currentZoom)}px`;
    canvas.style.height = `${Math.floor(baseHeight * currentZoom)}px`;
  }

  function applyZoom(target: HTMLDivElement, currentZoom: number): void {
    target
      .querySelectorAll<HTMLCanvasElement>("canvas[data-pdf-base-width][data-pdf-base-height]")
      .forEach((canvas) => applyCanvasZoom(canvas, currentZoom));
  }

  async function renderPdf(
    target: HTMLDivElement,
    source: string,
    current: number
  ): Promise<void> {
    target.replaceChildren();
    error = null;
    renderedPages = 0;
    status = "Loading PDF renderer...";
    void loadingTask?.destroy();

    try {
      const { getDocument } = await loadPdfJs();
      if (current !== generation) return;
      status = "Loading PDF...";

      const task = getDocument({
        data: base64ToBytes(source),
        useSystemFonts: true
      });
      loadingTask = task;

      const pdf = await task.promise;
      if (current !== generation) return;
      const maxPages = Math.min(pdf.numPages, 20);
      status = `Rendering ${maxPages} page${maxPages === 1 ? "" : "s"}...`;

      for (let pageNumber = 1; pageNumber <= maxPages; pageNumber += 1) {
        if (current !== generation) return;
        const page = await pdf.getPage(pageNumber);
        const viewport = page.getViewport({ scale: PDF_RENDER_SCALE });
        const wrapper = document.createElement("section");
        wrapper.className = "pdf-canvas-page";

        const label = document.createElement("div");
        label.className = "pdf-page-label";
        label.textContent = `Page ${pageNumber}`;
        wrapper.appendChild(label);

        const canvas = document.createElement("canvas");
        canvas.setAttribute("aria-label", `PDF page ${pageNumber}`);
        canvas.setAttribute("role", "img");
        const context = canvas.getContext("2d");
        if (!context) throw new Error("Canvas rendering is unavailable");
        const outputScale = window.devicePixelRatio || 1;
        const baseWidth = Math.floor(viewport.width);
        const baseHeight = Math.floor(viewport.height);
        canvas.width = Math.floor(viewport.width * outputScale);
        canvas.height = Math.floor(viewport.height * outputScale);
        canvas.dataset.pdfBaseWidth = String(baseWidth);
        canvas.dataset.pdfBaseHeight = String(baseHeight);
        applyCanvasZoom(canvas, zoom);
        wrapper.appendChild(canvas);
        target.appendChild(wrapper);

        await page.render({
          canvas,
          canvasContext: context,
          viewport,
          transform: outputScale === 1 ? undefined : [outputScale, 0, 0, outputScale, 0, 0]
        }).promise;
        renderedPages = pageNumber;
      }

      status = maxPages < pdf.numPages ? `Showing first ${maxPages} of ${pdf.numPages} pages.` : "";
    } catch (err) {
      if (current !== generation) return;
      error = err instanceof Error ? err.message : String(err);
      status = "";
    }
  }
</script>

<svelte:window onkeydown={handleWindowKeydown} />

<div
  bind:this={renderer}
  class="pdf-renderer"
  onwheel={handleZoomWheel}
>
  <div class="pdf-controls-row" aria-label="Document controls">
    <div class="pdf-controls-main">
      <span class="pdf-zoom-label">Zoom</span>
      <div class="pdf-toolbar" role="group" aria-label="PDF zoom controls">
        <button
          type="button"
          aria-label="Zoom out"
          title="Zoom out"
          onclick={() => setZoom(zoom - PDF_ZOOM_STEP)}
          disabled={zoom <= PDF_MIN_ZOOM}
        >
          <ZoomOutIcon size={15} strokeWidth={2.2} />
        </button>
        <button type="button" class="zoom-reset" aria-label="Reset zoom" title="Reset zoom" onclick={() => setZoom(1)}>
          <RotateCcwIcon size={14} strokeWidth={2.2} />
          <span>{zoomPercent}%</span>
        </button>
        <button
          type="button"
          aria-label="Zoom in"
          title="Zoom in"
          onclick={() => setZoom(zoom + PDF_ZOOM_STEP)}
          disabled={zoom >= PDF_MAX_ZOOM}
        >
          <ZoomInIcon size={15} strokeWidth={2.2} />
        </button>
      </div>
      <input
        class="pdf-zoom-range"
        type="range"
        min={PDF_MIN_ZOOM * 100}
        max={PDF_MAX_ZOOM * 100}
        step={PDF_ZOOM_STEP * 100}
        value={zoomPercent}
        aria-label="PDF zoom level"
        aria-valuetext={`${zoomPercent}%`}
        title={`PDF zoom ${zoomPercent}%`}
        oninput={handleZoomInput}
        onchange={handleZoomInput}
      />
      <span class="pdf-zoom-value" aria-hidden="true">{zoomPercent}%</span>
    </div>
    {#if status}
      <div class="pdf-status" role="status" aria-live="polite">{status}</div>
    {/if}
  </div>
  {#if error}
    <div class="pdf-error">PDF renderer failed: {error}</div>
  {/if}
  <div class="pdf-page-scroll" aria-label="PDF pages">
    <div bind:this={host} class="pdf-canvas-stack" aria-label="PDF rendered pages"></div>
    {#if showTextFallback}
      <article class="pdf-text-fallback" aria-label="PDF text fallback">
        <h2>Extracted text</h2>
        {#each textLines as line}
          <p>{line}</p>
        {/each}
      </article>
    {/if}
  </div>
</div>

<style>
  .pdf-renderer {
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr);
    gap: 12px;
    height: 100%;
    min-height: 0;
    position: relative;
    width: 100%;
  }

  .pdf-controls-row {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    width: 100%;
    max-width: 100%;
    padding: 10px;
    border: 1px solid #94a3b8;
    border-radius: 8px;
    background: #f8fafc;
    box-shadow: 0 14px 34px rgb(15 23 42 / 0.22);
    color: #0f172a;
    pointer-events: auto;
  }

  :global(html.dark) .pdf-controls-row {
    border-color: #38bdf8;
    background: #0f172a;
    color: #f8fafc;
    box-shadow: 0 18px 40px rgb(0 0 0 / 0.42);
  }

  .pdf-controls-main {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    min-width: 0;
  }

  .pdf-zoom-label {
    color: #475569;
    font-size: 11px;
    font-weight: 800;
    letter-spacing: 0.08em;
    line-height: 1;
    text-transform: uppercase;
  }

  :global(html.dark) .pdf-zoom-label {
    color: #cbd5e1;
  }

  .pdf-page-scroll {
    min-height: 0;
    overflow: auto;
    padding: 2px 2px 18px;
    overscroll-behavior: contain;
  }

  .pdf-toolbar {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px;
    border: 1px solid #94a3b8;
    border-radius: 7px;
    background: #e2e8f0;
  }

  :global(html.dark) .pdf-toolbar {
    border-color: #475569;
    background: #1e293b;
  }

  .pdf-toolbar button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    min-width: 42px;
    height: 36px;
    border: 1px solid #64748b;
    border-radius: 5px;
    background: #ffffff;
    color: #0f172a;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    font-weight: 650;
    line-height: 1;
  }

  :global(html.dark) .pdf-toolbar button {
    border-color: #38bdf8;
    background: #f8fafc;
    color: #0f172a;
  }

  .pdf-toolbar .zoom-reset {
    min-width: 84px;
    padding: 0 8px;
  }

  .pdf-toolbar button:hover:not(:disabled) {
    background: #e5e7eb;
    border-color: #94a3b8;
  }

  .pdf-zoom-range {
    width: clamp(150px, 22vw, 260px);
    min-width: 140px;
    height: 36px;
    padding: 0 2px;
    appearance: none;
    background: transparent;
    accent-color: #2563eb;
    cursor: pointer;
  }

  .pdf-zoom-value {
    min-width: 42px;
    color: #334155;
    font-size: 12px;
    font-variant-numeric: tabular-nums;
    font-weight: 750;
    text-align: right;
  }

  :global(html.dark) .pdf-zoom-value {
    color: #f8fafc;
  }

  .pdf-zoom-range::-webkit-slider-runnable-track {
    height: 7px;
    border: 1px solid #60a5fa;
    border-radius: 999px;
    background: #bfdbfe;
  }

  .pdf-zoom-range::-webkit-slider-thumb {
    width: 20px;
    height: 20px;
    margin-top: -7px;
    appearance: none;
    border: 2px solid #1d4ed8;
    border-radius: 999px;
    background: #ffffff;
    box-shadow: 0 2px 6px rgb(15 23 42 / 0.28);
  }

  .pdf-zoom-range::-moz-range-track {
    height: 7px;
    border: 1px solid #60a5fa;
    border-radius: 999px;
    background: #bfdbfe;
  }

  .pdf-zoom-range::-moz-range-thumb {
    width: 18px;
    height: 18px;
    border: 2px solid #1d4ed8;
    border-radius: 999px;
    background: #ffffff;
    box-shadow: 0 2px 6px rgb(15 23 42 / 0.28);
  }

  .pdf-zoom-range:focus-visible {
    outline: 2px solid #2563eb;
    outline-offset: 3px;
  }

  .pdf-toolbar button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .pdf-status {
    margin-left: auto;
    width: fit-content;
    max-width: 100%;
    padding: 6px 11px;
    border: 1px solid #38bdf8;
    border-radius: 999px;
    background: #020617;
    color: #f8fafc;
    font-size: 12px;
    font-weight: 750;
    line-height: 1.35;
    box-shadow: 0 0 0 2px rgb(248 250 252 / 0.9), 0 8px 18px rgb(15 23 42 / 0.2);
  }

  :global(html.dark) .pdf-status {
    border-color: #f8fafc;
    background: #e0f2fe;
    color: #020617;
    box-shadow: 0 0 0 2px rgb(15 23 42 / 0.95), 0 10px 20px rgb(0 0 0 / 0.45);
  }

  .pdf-error {
    justify-self: center;
    width: fit-content;
    padding: 4px 8px;
    border: 1px solid #d99a95;
    border-radius: 6px;
    background: #fff5f4;
    color: #b3261e;
    font-size: 12px;
  }

  .pdf-canvas-stack {
    display: grid;
    gap: 18px;
    width: max-content;
    min-width: 100%;
    justify-items: center;
    overflow: visible;
  }

  :global(.pdf-canvas-page) {
    display: grid;
    gap: 8px;
    justify-items: center;
  }

  :global(.pdf-page-label) {
    color: var(--muted-foreground);
    font-size: 11px;
    text-transform: uppercase;
  }

  :global(.pdf-canvas-page canvas) {
    max-width: none;
    height: auto !important;
    background: #fff;
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 10px 28px rgba(15, 23, 42, 0.12);
  }

  .pdf-text-fallback {
    margin: 18px auto 0;
    max-width: 860px;
    background: #fff;
    border: 1px solid var(--border);
    border-radius: 6px;
    color: var(--ink);
    padding: 20px 24px;
  }

  .pdf-text-fallback h2 {
    font-size: 12px;
    margin: 0 0 12px;
    text-transform: uppercase;
    color: var(--muted);
  }

  .pdf-text-fallback p {
    margin: 0 0 8px;
  }
</style>
