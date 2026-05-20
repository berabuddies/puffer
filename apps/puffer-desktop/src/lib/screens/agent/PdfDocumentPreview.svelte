<script lang="ts">
  import { onDestroy } from "svelte";
  import type { PDFDocumentLoadingTask } from "pdfjs-dist";

  type Props = {
    base64: string;
    textLines?: string[];
  };

  let { base64, textLines = [] }: Props = $props();

  let host = $state<HTMLDivElement | null>(null);
  let status = $state("Loading PDF...");
  let error = $state<string | null>(null);
  let renderedPages = $state(0);
  let zoom = $state(1);
  let zoomPercent = $derived(Math.round(zoom * 100));
  let renderScale = $derived(1.35 * zoom);
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
    const scale = renderScale;
    if (!target || !source) return;

    const current = ++generation;
    renderPdf(target, source, current, scale);
  });

  let lastSource = $state("");
  $effect(() => {
    const source = base64;
    if (source === lastSource) return;
    lastSource = source;
    zoom = 1;
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
    zoom = Math.max(0.5, Math.min(2, Math.round(next * 10) / 10));
  }

  async function renderPdf(
    target: HTMLDivElement,
    source: string,
    current: number,
    scale: number
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
        const viewport = page.getViewport({ scale });
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
        canvas.width = Math.floor(viewport.width * outputScale);
        canvas.height = Math.floor(viewport.height * outputScale);
        canvas.style.width = `${Math.floor(viewport.width)}px`;
        canvas.style.height = `${Math.floor(viewport.height)}px`;
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
  <div class="pdf-toolbar" role="group" aria-label="PDF zoom controls">
    <button type="button" aria-label="Zoom out" onclick={() => setZoom(zoom - 0.1)} disabled={zoom <= 0.5}>
      -
    </button>
    <button type="button" aria-label="Reset zoom" onclick={() => setZoom(1)}>
      {zoomPercent}%
    </button>
    <button type="button" aria-label="Zoom in" onclick={() => setZoom(zoom + 0.1)} disabled={zoom >= 2}>
      +
    </button>
  </div>
  {#if status}
    <div class="pdf-status">{status}</div>
  {/if}
  {#if error}
    <div class="pdf-error">PDF renderer failed: {error}</div>
  {/if}
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

<style>
  .pdf-renderer {
    display: grid;
    gap: 14px;
    width: 100%;
  }

  .pdf-toolbar {
    display: inline-flex;
    align-items: center;
    justify-self: center;
    gap: 4px;
    padding: 3px;
    border: 1px solid var(--border);
    border-radius: 7px;
    background: var(--background);
    box-shadow: 0 1px 2px rgb(15 23 42 / 0.06);
  }

  .pdf-toolbar button {
    min-width: 34px;
    height: 26px;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--foreground);
    cursor: pointer;
    font: inherit;
    font-size: 12px;
    line-height: 1;
  }

  .pdf-toolbar button:hover:not(:disabled) {
    background: color-mix(in oklab, var(--background) 88%, var(--muted));
  }

  .pdf-toolbar button:disabled {
    cursor: not-allowed;
    opacity: 0.45;
  }

  .pdf-status,
  .pdf-error {
    justify-self: center;
    width: fit-content;
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--background);
    color: var(--muted-foreground);
    font-size: 12px;
  }

  .pdf-error {
    color: #b3261e;
  }

  .pdf-canvas-stack {
    display: grid;
    gap: 18px;
    justify-items: center;
    overflow: auto;
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
    max-width: 100%;
    height: auto !important;
    background: #fff;
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 10px 28px rgba(15, 23, 42, 0.12);
  }

  .pdf-text-fallback {
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
