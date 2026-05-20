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
  let generation = 0;
  let loadingTask: PDFDocumentLoadingTask | null = null;

  type PdfJsModule = typeof import("pdfjs-dist");

  let pdfJsModulePromise: Promise<PdfJsModule> | null = null;

  function loadPdfJs(): Promise<PdfJsModule> {
    pdfJsModulePromise ??= import("pdfjs-dist").then((module) => {
      module.GlobalWorkerOptions.workerSrc = new URL(
        "pdfjs-dist/build/pdf.worker.mjs",
        import.meta.url
      ).toString();
      return module;
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

  async function renderPdf(target: HTMLDivElement, source: string, current: number): Promise<void> {
    target.replaceChildren();
    error = null;
    status = "Loading PDF renderer...";
    void loadingTask?.destroy();

    const { getDocument } = await loadPdfJs();
    if (current !== generation) return;
    status = "Loading PDF...";

    const task = getDocument({
      data: base64ToBytes(source),
      useSystemFonts: true
    });
    loadingTask = task;

    try {
      const pdf = await task.promise;
      if (current !== generation) return;
      const maxPages = Math.min(pdf.numPages, 20);
      status = `Rendering ${maxPages} page${maxPages === 1 ? "" : "s"}...`;

      for (let pageNumber = 1; pageNumber <= maxPages; pageNumber += 1) {
        if (current !== generation) return;
        const page = await pdf.getPage(pageNumber);
        const viewport = page.getViewport({ scale: 1.35 });
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
  {#if status}
    <div class="pdf-status">{status}</div>
  {/if}
  {#if error}
    <div class="pdf-error">PDF renderer failed: {error}</div>
  {/if}
  <div bind:this={host} class="pdf-canvas-stack" aria-label="PDF rendered pages"></div>
  {#if error && textLines.length > 0}
    <article class="pdf-text-fallback" aria-label="PDF text fallback">
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

  .pdf-status,
  .pdf-error {
    color: var(--muted);
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
    color: var(--muted);
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

  .pdf-text-fallback p {
    margin: 0 0 8px;
  }
</style>
