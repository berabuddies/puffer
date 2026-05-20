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

<div class="pdf-renderer">
  <div class="pdf-controls-row">
    <div class="pdf-toolbar" role="group" aria-label="PDF zoom controls">
      <button
        type="button"
        aria-label="Zoom out"
        title="Zoom out"
        onclick={() => setZoom(zoom - PDF_ZOOM_STEP)}
        disabled={zoom <= PDF_MIN_ZOOM}
      >
        <ZoomOutIcon size={14} strokeWidth={2} />
      </button>
      <button type="button" class="zoom-reset" aria-label="Reset zoom" title="Reset zoom" onclick={() => setZoom(1)}>
        <RotateCcwIcon size={13} strokeWidth={2} />
        <span>{zoomPercent}%</span>
      </button>
      <button
        type="button"
        aria-label="Zoom in"
        title="Zoom in"
        onclick={() => setZoom(zoom + PDF_ZOOM_STEP)}
        disabled={zoom >= PDF_MAX_ZOOM}
      >
        <ZoomInIcon size={14} strokeWidth={2} />
      </button>
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
    position: relative;
    z-index: 20;
    display: inline-flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-start;
    justify-self: start;
    gap: 8px;
    max-width: 100%;
    padding: 7px;
    border: 1px solid #94a3b8;
    border-radius: 8px;
    background: #f8fafc;
    box-shadow: 0 12px 32px rgb(15 23 42 / 0.26);
    color: #111827;
    pointer-events: auto;
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
    border: 1px solid var(--border);
    border-radius: 7px;
    background: #eef2f7;
  }

  .pdf-toolbar button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    min-width: 36px;
    height: 30px;
    border: 1px solid var(--border);
    border-radius: 5px;
    background: #ffffff;
    color: #111827;
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    font-weight: 650;
    line-height: 1;
  }

  .pdf-toolbar .zoom-reset {
    min-width: 66px;
    padding: 0 8px;
  }

  .pdf-toolbar button:hover:not(:disabled) {
    background: #e5e7eb;
    border-color: #94a3b8;
  }

  .pdf-toolbar button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .pdf-status {
    width: fit-content;
    max-width: 100%;
    padding: 5px 10px;
    border: 1px solid #1d4ed8;
    border-radius: 6px;
    background: #172554;
    color: #eff6ff;
    font-size: 12px;
    font-weight: 650;
    line-height: 1.35;
    box-shadow: 0 1px 0 rgb(255 255 255 / 0.45);
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
