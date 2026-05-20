import { expect, type Locator, type Page, test } from "@playwright/test";
import { deflateRawSync } from "node:zlib";
import { FakeDaemon } from "./support/fakeDaemon";

async function openRegressionAgent(page: Page): Promise<void> {
  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Browser regression\b/ })
    .click();
}

async function openFilesPanel(page: Page): Promise<void> {
  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Files", exact: true }).click();
}

async function openCreateProjectDialog(page: Page) {
  await page.getByRole("button", { name: "Create Project" }).click();
  const dialog = page.getByRole("dialog", { name: "Create Project" });
  await expect(dialog).toBeVisible();
  return dialog;
}

async function expectFocusInside(dialog: Locator): Promise<void> {
  await expect.poll(() =>
    dialog.evaluate((node) => node.contains(document.activeElement))
  ).toBe(true);
}

async function expectTabFocusTrapped(page: Page, dialog: Locator, count: number): Promise<void> {
  for (let index = 0; index < count; index += 1) {
    await page.keyboard.press("Tab");
    await expectFocusInside(dialog);
  }
}

const codexAuth = [
  {
    providerId: "codex",
    kind: "oauth",
    email: "tester@example.com",
    expiresAtMs: null,
    scopes: [],
    planType: "test",
    organizationName: null
  }
];

const canonicalProviderAuth = [
  {
    providerId: "openai",
    kind: "oauth",
    email: "tester@example.com",
    expiresAtMs: null,
    scopes: [],
    planType: "test",
    organizationName: null
  },
  {
    providerId: "anthropic",
    kind: "api_key",
    email: null,
    expiresAtMs: null,
    scopes: [],
    planType: null,
    organizationName: null
  }
];

const groqProvider = {
  id: "groq",
  displayName: "Groq",
  baseUrl: "https://api.groq.com/openai",
  defaultApi: "openai-completions",
  modelCount: 1,
  authModes: ["api_key"],
  sourceKind: "test",
  sourcePath: null
};

const groqAuth = [
  {
    providerId: "groq",
    kind: "api_key",
    email: null,
    expiresAtMs: null,
    scopes: [],
    planType: null,
    organizationName: null
  }
];

function makePdfBase64(text: string, pageCount = 1, width = 260, height = 160): string {
  const fontObjectId = 3 + pageCount * 2;
  const pageObjectIds = Array.from({ length: pageCount }, (_, index) => 3 + index * 2);
  const pageRefs = pageObjectIds.map((id) => `${id} 0 R`).join(" ");
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    `<< /Type /Pages /Kids [${pageRefs}] /Count ${pageCount} >>`,
    ...Array.from({ length: pageCount }).flatMap((_, index) => {
      const pageObjectId = 3 + index * 2;
      const contentObjectId = pageObjectId + 1;
      const pageText = pageCount === 1 ? text : `${text} ${index + 1}`;
      const stream = `BT /F1 18 Tf 20 100 Td (${pageText.replace(/[()\\]/g, "\\$&")}) Tj ET`;
      return [
        `<< /Type /Page /Parent 2 0 R /MediaBox [0 0 ${width} ${height}] /Resources << /Font << /F1 ${fontObjectId} 0 R >> >> /Contents ${contentObjectId} 0 R >>`,
        `<< /Length ${Buffer.byteLength(stream)} >>\nstream\n${stream}\nendstream`
      ];
    }),
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"
  ];
  let pdf = "%PDF-1.4\n";
  const offsets: number[] = [];
  for (const [index, object] of objects.entries()) {
    offsets.push(Buffer.byteLength(pdf));
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  }
  const xrefOffset = Buffer.byteLength(pdf);
  pdf += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n`;
  pdf += offsets.map((offset) => `${String(offset).padStart(10, "0")} 00000 n \n`).join("");
  pdf += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xrefOffset}\n%%EOF\n`;
  return Buffer.from(pdf, "utf8").toString("base64");
}

function makeLegacyOfficeBase64(text: string): string {
  return Buffer.concat([
    Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]),
    Buffer.from("Puffer legacy Office fixture", "utf8"),
    Buffer.from(text, "utf16le")
  ]).toString("base64");
}

function makeRtfDocBase64(...paragraphs: string[]): string {
  const escapeRtf = (value: string) => value.replace(/[\\{}]/g, "\\$&");
  return Buffer.from(
    `{\\rtf1\\ansi{\\fonttbl{\\f0 Arial;}}\\f0\\fs24 ${paragraphs.map(escapeRtf).join("\\par ")}}`,
    "utf8"
  ).toString("base64");
}

function makeHtmlDocBase64(html: string): string {
  return Buffer.from(html, "utf8").toString("base64");
}

function makeLargeLegacyOfficeBase64(text: string): string {
  return Buffer.concat([
    Buffer.from([0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1]),
    Buffer.alloc(300_000, 0),
    Buffer.from(text, "utf16le")
  ]).toString("base64");
}

async function expectCanvasHasInk(page: Page, selector: string): Promise<void> {
  const canvas = page.locator(selector);
  await expect(canvas).toBeVisible();
  await expect.poll(async () =>
    canvas.evaluate((node: HTMLCanvasElement) => {
      const context = node.getContext("2d");
      if (!context || node.width === 0 || node.height === 0) return 0;
      const pixels = context.getImageData(0, 0, node.width, node.height).data;
      let nonWhite = 0;
      for (let offset = 0; offset < pixels.length; offset += 16) {
        const red = pixels[offset];
        const green = pixels[offset + 1];
        const blue = pixels[offset + 2];
        const alpha = pixels[offset + 3];
        if (alpha > 0 && (red < 248 || green < 248 || blue < 248)) nonWhite += 1;
      }
      return nonWhite;
    })
  ).toBeGreaterThan(25);
}

function makeZipBase64(entries: Record<string, string>): string {
  const localParts: Buffer[] = [];
  const centralParts: Buffer[] = [];
  let offset = 0;

  for (const [name, text] of Object.entries(entries)) {
    const nameBytes = Buffer.from(name, "utf8");
    const content = Buffer.from(text, "utf8");
    const compressed = deflateRawSync(content);

    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x04034b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(0, 6);
    local.writeUInt16LE(8, 8);
    local.writeUInt32LE(0, 10);
    local.writeUInt32LE(0, 14);
    local.writeUInt32LE(compressed.length, 18);
    local.writeUInt32LE(content.length, 22);
    local.writeUInt16LE(nameBytes.length, 26);
    local.writeUInt16LE(0, 28);
    localParts.push(local, nameBytes, compressed);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x02014b50, 0);
    central.writeUInt16LE(20, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(0, 8);
    central.writeUInt16LE(8, 10);
    central.writeUInt32LE(0, 12);
    central.writeUInt32LE(0, 16);
    central.writeUInt32LE(compressed.length, 20);
    central.writeUInt32LE(content.length, 24);
    central.writeUInt16LE(nameBytes.length, 28);
    central.writeUInt16LE(0, 30);
    central.writeUInt16LE(0, 32);
    central.writeUInt16LE(0, 34);
    central.writeUInt16LE(0, 36);
    central.writeUInt32LE(0, 38);
    central.writeUInt32LE(offset, 42);
    centralParts.push(central, nameBytes);
    offset += local.length + nameBytes.length + compressed.length;
  }

  const centralOffset = offset;
  const centralDirectory = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(0, 4);
  end.writeUInt16LE(0, 6);
  end.writeUInt16LE(Object.keys(entries).length, 8);
  end.writeUInt16LE(Object.keys(entries).length, 10);
  end.writeUInt32LE(centralDirectory.length, 12);
  end.writeUInt32LE(centralOffset, 16);
  end.writeUInt16LE(0, 20);
  return Buffer.concat([...localParts, centralDirectory, end]).toString("base64");
}

function seedPreviewFiles(daemon: FakeDaemon): void {
  daemon.seedFile(
    "/tmp/puffer/README.md",
    "# Project Notes\n\n- Browser actions stay visible\n- Files render documents\n"
  );
  daemon.seedFile("/tmp/puffer/locations.csv", "Name,Kind\nLibrary,Food\nCafe,Food\n");
  const docx = makeZipBase64({
    "word/document.xml":
      '<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Quarterly planning note</w:t></w:r></w:p></w:body></w:document>'
  });
  daemon.seedBinaryFile("/tmp/puffer/brief.docx", docx);
  const pptx = makeZipBase64({
    "ppt/slides/slide1.xml":
      '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Launch checklist</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>',
    "ppt/slides/slide2.xml":
      '<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>QA signoff</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>'
  });
  daemon.seedBinaryFile("/tmp/puffer/deck.pptx", pptx);
  const xlsx = makeZipBase64({
    "xl/sharedStrings.xml":
      '<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Owner</t></si><si><t>Status</t></si><si><t>Otter</t></si><si><t>Ready</t></si></sst>',
    "xl/workbook.xml":
      '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Tasks" sheetId="1" r:id="rId1"/></sheets></workbook>',
    "xl/_rels/workbook.xml.rels":
      '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="worksheet" Target="worksheets/sheet1.xml"/></Relationships>',
    "xl/worksheets/sheet1.xml":
      '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c><c r="B1" t="s"><v>1</v></c></row><row r="2"><c r="A2" t="s"><v>2</v></c><c r="B2" t="s"><v>3</v></c></row></sheetData></worksheet>'
  });
  daemon.seedBinaryFile("/tmp/puffer/tasks.xlsx", xlsx);
  daemon.seedBinaryFile("/tmp/puffer/sample.pdf", makePdfBase64("Puffer PDF preview"));
  daemon.seedBinaryFile("/tmp/puffer/wide.pdf", makePdfBase64("Wide PDF preview", 1, 960, 620));
  daemon.seedBinaryFile("/tmp/puffer/long.pdf", makePdfBase64("Long PDF preview", 29));
  daemon.seedBinaryFile(
    "/tmp/puffer/tex-garbage.pdf",
    makePdfBase64("Clean PDF preview"),
    undefined,
    [
      "EXTRACTED TEXT",
      "CIDInit",
      "TeX-T1-0",
      "TeX-T1-0 TeX T1 0",
      "ÿ",
      "\u000e",
      "9",
      "\u0010",
      "a",
      "~"
    ]
  );
  daemon.seedFile(
    "/tmp/puffer/ascii-sniffed.pdf",
    Buffer.from(makePdfBase64("ASCII sniffed PDF preview"), "base64").toString("utf8")
  );
  daemon.seedBinaryFile("/tmp/puffer/old-plan.doc", makeLargeLegacyOfficeBase64("Legacy Word agenda"));
  daemon.seedBinaryFile("/tmp/puffer/template.dot", makeLegacyOfficeBase64("Legacy Word template"));
  daemon.seedFile(
    "/tmp/puffer/standalone.rtf",
    "{\\rtf1\\ansi Standalone RTF agenda\\par RTF follow-up}"
  );
  daemon.seedBinaryFile(
    "/tmp/puffer/native-old-word.doc",
    makeLegacyOfficeBase64("garbled fallback"),
    undefined,
    ["Native textutil Word agenda", "Native textutil follow-up"]
  );
  daemon.seedBinaryFile(
    "/tmp/puffer/styled-old-word.doc",
    makeLegacyOfficeBase64("unstyled fallback"),
    undefined,
    ["Styled legacy Word heading", "Italic class note"],
    [
      "<html><head><style>",
      "p.p1 {font-weight: bold; text-align: center; margin: 0px 0px 12px 0px;}",
      "span.s1 {font-style: italic; color: #334155;}",
      "</style></head><body>",
      '<p class="p1">Styled legacy Word heading</p>',
      '<p><span class="s1">Italic class note</span></p>',
      "</body></html>"
    ].join("")
  );
  daemon.seedBinaryFile(
    "/tmp/puffer/old-deck.ppt",
    makeLegacyOfficeBase64("Legacy PowerPoint agenda")
  );
  daemon.seedBinaryFile("/tmp/puffer/old-budget.xls", makeLegacyOfficeBase64("Legacy Excel budget"));
  daemon.seedBinaryFile(
    "/tmp/puffer/old-rtf.doc",
    makeRtfDocBase64("Legacy RTF agenda", "Second RTF paragraph")
  );
  daemon.seedBinaryFile(
    "/tmp/puffer/old-html.doc",
    makeHtmlDocBase64(
      "<!doctype html><html><body><h1>Legacy HTML agenda</h1><p>Owner: Otter</p></body></html>"
    )
  );
}

test("Files tab close button works from the keyboard", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const libTab = page.getByRole("tab", { name: /lib\.rs/ });
  await expect(libTab).toBeVisible();

  await libTab.getByRole("button", { name: "Close src/lib.rs" }).focus();
  await page.keyboard.press("Enter");

  await expect(page.getByRole("tab", { name: /lib\.rs/ })).toHaveCount(0);
});

test("Files tab close controls include paths for duplicate file names", async ({ page }) => {
  const duplicatePath = "/tmp/puffer/tests/main.rs";
  const daemon = new FakeDaemon();
  daemon.seedFile(duplicatePath, "fn duplicate_main() {}\n");
  daemon.setFileTabs(
    [
      { path: "/tmp/puffer/src/main.rs", pinned: true },
      { path: duplicatePath, pinned: true },
      { path: "/tmp/puffer/src/lib.rs", pinned: true }
    ],
    "/tmp/puffer/src/main.rs"
  );
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await expect(page.getByRole("button", { name: "Close main.rs", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Close src/main.rs", exact: true })).toHaveCount(1);
  const closeDuplicate = page.getByRole("button", { name: "Close tests/main.rs", exact: true });
  await expect(closeDuplicate).toHaveCount(1);
  await closeDuplicate.click();

  await expect(page.getByRole("button", { name: "Close tests/main.rs", exact: true })).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Close src/main.rs", exact: true })).toHaveCount(1);
});

test("Files directory rows expose expand and collapse state", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const srcDir = page.locator(".tree-body").getByRole("button", { name: /^src$/ });
  await expect(srcDir).toHaveAttribute("aria-expanded", "false");

  await srcDir.click();
  await expect(srcDir).toHaveAttribute("aria-expanded", "true");
  await expect(page.locator(".tree-body").getByRole("button", { name: "main.rs" })).toBeVisible();

  await srcDir.click();
  await expect(srcDir).toHaveAttribute("aria-expanded", "false");
  await expect(page.locator(".tree-body").getByRole("button", { name: "main.rs" })).toHaveCount(0);
});

test("Files tab saves text edits through the daemon", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");

  const saved = "fn main() {\n    println!(\"saved\");\n}\n";
  await editor.fill(saved);
  await expect(page.locator(".file-tab.active .dirty-dot")).toBeVisible();

  await page.getByRole("button", { name: "Save" }).click();
  const request = await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs"
  );
  expect(request.params).toMatchObject({
    path: "/tmp/puffer/src/main.rs",
    content: saved
  });

  await expect(page.getByRole("button", { name: "Save" })).toHaveCount(0);
  await expect(page.locator(".file-tab.active .dirty-dot")).toHaveCount(0);
  await expect(editor).toHaveValue(saved);
});

test("Files tab previews common document and data formats", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  const pdfRendererRequests: string[] = [];
  page.on("request", (request) => {
    const url = request.url();
    if (url.includes("pdfjs-dist") || url.includes("pdf.worker")) pdfRendererRequests.push(url);
  });
  await page.addInitScript(() => {
    Object.defineProperty(window, "DecompressionStream", { value: undefined, configurable: true });
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("button", { name: "README.md" }).click();
  await expect(page.getByLabel("Markdown preview")).toContainText("Project Notes");
  await expect(page.getByLabel("Markdown preview")).toContainText("Files render documents");

  await page.getByRole("button", { name: "locations.csv" }).click();
  await expect(page.getByLabel("CSV preview")).toContainText("Library");
  await expect(page.getByLabel("CSV preview")).toContainText("Cafe");
  expect(pdfRendererRequests).toHaveLength(0);

  await page.evaluate(() => {
    Object.defineProperty(Promise, "withResolvers", { value: undefined, configurable: true });
    class BlockedWorker {
      constructor() {
        throw new Error("Worker constructors are blocked in this regression");
      }
    }
    Object.defineProperty(window, "Worker", { value: BlockedWorker, configurable: true });
  });

  await page.getByRole("button", { name: "sample.pdf" }).click();
  await expect(page.getByLabel("PDF preview")).toBeVisible();
  await daemon.waitForRequest(
    "read_file",
    (request) => request.params.path === "/tmp/puffer/sample.pdf" && request.params.maxBytes === 24 * 1024 * 1024
  );
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');
  await expect(page.getByLabel("PDF text fallback")).toHaveCount(0);
  expect(pdfRendererRequests.length).toBeGreaterThan(0);

  const pdfPreview = page.getByLabel("PDF preview");
  const zoomControls = pdfPreview.getByRole("group", { name: "PDF zoom controls" });
  await expect(zoomControls).toBeVisible();
  await expect(zoomControls.getByText("100%")).toBeVisible();
  const initialWidth = await page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
    Math.round(canvas.getBoundingClientRect().width)
  );
  await zoomControls.getByRole("button", { name: "Zoom in" }).click();
  await expect(zoomControls.getByText("110%")).toBeVisible();
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(initialWidth);
  await zoomControls.getByRole("button", { name: "Reset zoom" }).click();
  await expect(zoomControls.getByText("100%")).toBeVisible();

  await page.getByRole("button", { name: "wide.pdf" }).click();
  await expect(page.getByLabel("PDF preview")).toBeVisible();
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');
  const wideInitialWidth = await page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
    Math.round(canvas.getBoundingClientRect().width)
  );
  const shellWidth = await pdfPreview.evaluate((node) => Math.round(node.getBoundingClientRect().width));
  expect(wideInitialWidth).toBeGreaterThan(shellWidth);
  await zoomControls.getByRole("button", { name: "Zoom in" }).click();
  await expect(zoomControls.getByText("110%")).toBeVisible();
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(wideInitialWidth);
  await zoomControls.getByRole("button", { name: "Reset zoom" }).click();
  await expect(zoomControls.getByText("100%")).toBeVisible();

  await page.getByRole("button", { name: "ascii-sniffed.pdf" }).click();
  await expect(page.getByLabel("PDF preview")).toBeVisible();
  await daemon.waitForRequest(
    "read_file",
    (request) =>
      request.params.path === "/tmp/puffer/ascii-sniffed.pdf" &&
      request.params.maxBytes === 24 * 1024 * 1024
  );
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');
  await expect(page.getByLabel("PDF text fallback")).toHaveCount(0);

  await page.getByRole("button", { name: "tex-garbage.pdf" }).click();
  await expect(page.getByLabel("PDF preview")).toBeVisible();
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');
  await expect(page.getByLabel("PDF text fallback")).toHaveCount(0);
  await expect(page.getByLabel("PDF preview")).not.toContainText("EXTRACTED TEXT");
  await expect(page.getByLabel("PDF preview")).not.toContainText("CIDInit");
  await expect(page.getByLabel("PDF preview")).not.toContainText("TeX-T1-0");

  await page.getByRole("button", { name: "long.pdf" }).click();
  await expect(page.getByLabel("PDF preview")).toBeVisible();
  const pageLimit = page.getByText("Showing first 20 of 29 pages.");
  await expect(pageLimit).toBeVisible();
  const pageLimitColors = await pageLimit.evaluate((node) => {
    const style = getComputedStyle(node);
    const shell = node.closest(".pdf-shell");
    const controls = node.closest(".pdf-controls-row");
    const shellStyle = shell ? getComputedStyle(shell) : null;
    const controlsStyle = controls ? getComputedStyle(controls) : null;
    return {
      color: style.color,
      backgroundColor: style.backgroundColor,
      borderTopColor: style.borderTopColor,
      shellBackgroundColor: shellStyle?.backgroundColor ?? "",
      controlsBackgroundColor: controlsStyle?.backgroundColor ?? "",
      controlsBoxShadow: controlsStyle?.boxShadow ?? ""
    };
  });
  expect(pageLimitColors.color).not.toBe(pageLimitColors.backgroundColor);
  expect(pageLimitColors.backgroundColor).not.toBe("rgba(0, 0, 0, 0)");
  expect(pageLimitColors.backgroundColor).not.toBe(pageLimitColors.shellBackgroundColor);
  expect(pageLimitColors.borderTopColor).not.toBe(pageLimitColors.shellBackgroundColor);
  expect(pageLimitColors.controlsBackgroundColor).not.toBe(pageLimitColors.shellBackgroundColor);
  expect(pageLimitColors.controlsBoxShadow).not.toBe("none");
  const longInitialWidth = await page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
    Math.round(canvas.getBoundingClientRect().width)
  );
  await pdfPreview.getByLabel("PDF pages").evaluate((node) => {
    node.scrollTop = 480;
    node.scrollLeft = 0;
  });
  await expect(zoomControls).toBeVisible();
  await zoomControls.getByRole("button", { name: "Zoom in" }).click();
  await expect(zoomControls.getByText("110%")).toBeVisible();
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(longInitialWidth);

  await page.getByRole("button", { name: "brief.docx" }).click();
  await expect(page.getByLabel("DOCX preview")).toContainText("Quarterly planning note");

  await page.getByRole("button", { name: "deck.pptx" }).click();
  await expect(page.getByLabel("PowerPoint preview")).toContainText("Launch checklist");
  await expect(page.getByLabel("PowerPoint preview")).toContainText("QA signoff");

  await page.getByRole("button", { name: "tasks.xlsx" }).click();
  await expect(page.getByLabel("Excel preview")).toContainText("Tasks");
  await expect(page.getByLabel("Excel preview")).toContainText("Otter");
  await expect(page.getByLabel("Excel preview")).toContainText("Ready");

  await page.getByRole("button", { name: "old-plan.doc" }).click();
  await daemon.waitForRequest(
    "read_file",
    (request) => request.params.path === "/tmp/puffer/old-plan.doc" && request.params.maxBytes === 24 * 1024 * 1024
  );
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Legacy Word agenda");

  await page.getByRole("button", { name: "template.dot" }).click();
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Legacy Word template");

  await page.getByRole("button", { name: "standalone.rtf" }).click();
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Standalone RTF agenda");
  await expect(page.getByLabel("Legacy Word preview")).toContainText("RTF follow-up");

  await page.getByRole("button", { name: "native-old-word.doc" }).click();
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Native textutil Word agenda");
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Native textutil follow-up");

  await page.getByRole("button", { name: "styled-old-word.doc" }).click();
  const styledPreview = page.getByLabel("Legacy Word preview");
  const styledHeading = styledPreview.getByText("Styled legacy Word heading");
  const styledNote = styledPreview.getByText("Italic class note");
  await expect(styledHeading).toBeVisible();
  await expect(styledNote).toBeVisible();
  await expect(styledHeading).toHaveCSS("text-align", "center");
  await expect(styledHeading).toHaveCSS("font-weight", /^(700|bold)$/);
  await expect(styledNote).toHaveCSS("font-style", "italic");

  await page.getByRole("button", { name: "old-deck.ppt" }).click();
  await expect(page.getByLabel("Legacy PowerPoint preview")).toContainText(
    "Legacy PowerPoint agenda"
  );

  await page.getByRole("button", { name: "old-budget.xls" }).click();
  await expect(page.getByLabel("Legacy Excel preview")).toContainText("Legacy Excel budget");

  await page.getByRole("button", { name: "old-rtf.doc" }).click();
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Legacy RTF agenda");
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Second RTF paragraph");

  await page.getByRole("button", { name: "old-html.doc" }).click();
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Legacy HTML agenda");
  await expect(page.getByLabel("Legacy Word preview")).toContainText("Owner: Otter");
});

test("Files tab previews files whose read path canonicalizes differently", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.seedCanonicalFile(
    "/tmp/puffer/link-readme.md",
    "/tmp/puffer/real-readme.md",
    "# Canonical Notes\n\nOpened through a symlinked workspace entry.\n"
  );
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("button", { name: "link-readme.md" }).click();
  await daemon.waitForRequest(
    "read_file",
    (request) => request.params.path === "/tmp/puffer/link-readme.md"
  );

  await expect(page.getByLabel("Markdown preview")).toContainText("Canonical Notes");
  await expect(page.getByRole("tab", { name: /link-readme\.md/ })).toHaveAttribute(
    "aria-selected",
    "true"
  );
});

test("Files tab PDF zoom is immediate and page-limit status remains readable", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  daemon.seedBinaryFile("/tmp/puffer/wide-long.pdf", makePdfBase64("Wide long PDF preview", 29, 960, 620));
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem("puffer-desktop:tweaks", JSON.stringify({ theme: "dark" }));
  });
  await daemon.open(page);

  await expect(page.locator("html")).toHaveClass(/dark/);
  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("button", { name: "wide-long.pdf" }).click();
  const pdfPreview = page.getByLabel("PDF preview");
  await expect(pdfPreview).toBeVisible();
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');

  const pageLimit = pdfPreview.getByText("Showing first 20 of 29 pages.");
  await expect(pageLimit).toBeVisible();
  await expect(pdfPreview.getByText("Zoom", { exact: true })).toBeVisible();
  const controls = pdfPreview.getByRole("group", { name: "PDF zoom controls" });
  await expect(controls).toBeVisible();
  await expect(page.locator('canvas[aria-label^="PDF page"]')).toHaveCount(20);

  const contrast = await pageLimit.evaluate((node) => {
    const parseRgb = (value: string): [number, number, number] => {
      const match = value.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
      if (!match) return [0, 0, 0];
      return [Number(match[1]), Number(match[2]), Number(match[3])];
    };
    const channelDelta = (left: [number, number, number], right: [number, number, number]): number =>
      Math.abs(left[0] - right[0]) + Math.abs(left[1] - right[1]) + Math.abs(left[2] - right[2]);
    const luminance = ([red, green, blue]: [number, number, number]): number => {
      const channels = [red, green, blue].map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.03928
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
    };
    const status = node.closest(".pdf-status") as HTMLElement | null;
    const style = status ? getComputedStyle(status) : getComputedStyle(node);
    const shell = node.closest(".pdf-shell");
    const controls = node.closest(".pdf-controls-row");
    const controlsMain = controls?.querySelector(".pdf-controls-main");
    const shellStyle = shell ? getComputedStyle(shell) : null;
    const controlsStyle = controls ? getComputedStyle(controls) : null;
    const statusBackground = parseRgb(style.backgroundColor);
    const foreground = luminance(parseRgb(style.color));
    const background = luminance(statusBackground);
    const shellBackground = parseRgb(shellStyle?.backgroundColor ?? "");
    const controlsBackground = parseRgb(controlsStyle?.backgroundColor ?? "");
    const zoomButton = (controls as HTMLElement | null)?.querySelector<HTMLButtonElement>('button[aria-label="Zoom in"]');
    const zoomButtonRect = zoomButton?.getBoundingClientRect();
    const statusRect = (status ?? (node as HTMLElement)).getBoundingClientRect();
    const controlsRect = (controls as HTMLElement | null)?.getBoundingClientRect();
    const controlsMainRect = (controlsMain as HTMLElement | null)?.getBoundingClientRect();
    return {
      ratio: (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05),
      backgroundColor: style.backgroundColor,
      backgroundLuminance: background,
      shellBackgroundColor: shellStyle?.backgroundColor ?? "",
      controlsBackgroundColor: controlsStyle?.backgroundColor ?? "",
      shellDelta: channelDelta(statusBackground, shellBackground),
      controlsDelta: channelDelta(statusBackground, controlsBackground),
      statusLooksAmber: statusBackground[0] > statusBackground[2] + 20 && statusBackground[1] > statusBackground[2] + 10,
      statusLabel: Boolean((controls as HTMLElement | null)?.querySelector(".pdf-status-label")),
      zoomButtonWidth: Math.round(zoomButtonRect?.width ?? 0),
      zoomButtonHeight: Math.round(zoomButtonRect?.height ?? 0),
      statusWidth: Math.round(statusRect.width),
      controlsWidth: Math.round(controlsRect?.width ?? 0),
      statusTop: Math.round(statusRect.top),
      controlsMainBottom: Math.round(controlsMainRect?.bottom ?? 0)
    };
  });
  expect(contrast.ratio).toBeGreaterThanOrEqual(4.5);
  expect(contrast.backgroundLuminance).toBeGreaterThan(0.45);
  expect(contrast.statusLooksAmber).toBe(true);
  expect(contrast.backgroundColor).not.toBe(contrast.shellBackgroundColor);
  expect(contrast.backgroundColor).not.toBe(contrast.controlsBackgroundColor);
  expect(contrast.backgroundColor).not.toBe("rgba(0, 0, 0, 0)");
  expect(contrast.shellDelta).toBeGreaterThan(60);
  expect(contrast.controlsDelta).toBeGreaterThan(60);
  expect(contrast.statusLabel).toBe(true);
  expect(contrast.zoomButtonWidth).toBeGreaterThanOrEqual(48);
  expect(contrast.zoomButtonHeight).toBeGreaterThanOrEqual(42);
  expect(contrast.statusWidth).toBeGreaterThan(contrast.controlsWidth - 28);
  expect(contrast.statusTop).toBeGreaterThanOrEqual(contrast.controlsMainBottom - 2);

  const initialWidth = await page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
    Math.round(canvas.getBoundingClientRect().width)
  );
  const zoomSlider = pdfPreview.getByLabel("PDF zoom level");
  await expect(zoomSlider).toBeVisible();
  const sliderBox = await zoomSlider.boundingBox();
  expect(sliderBox?.width ?? 0).toBeGreaterThan(100);
  expect(sliderBox?.height ?? 0).toBeGreaterThanOrEqual(30);
  const sliderReceivesPointer = await zoomSlider.evaluate((input) => {
    const rect = input.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    return hit === input || input.contains(hit);
  });
  expect(sliderReceivesPointer).toBe(true);
  await zoomSlider.evaluate((input) => {
    const range = input as HTMLInputElement;
    range.value = "140";
    range.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await expect(controls.getByText("140%")).toBeVisible();
  await expect(page.locator('canvas[aria-label^="PDF page"]')).toHaveCount(20, { timeout: 100 });
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(initialWidth);
  await controls.getByRole("button", { name: "Reset zoom" }).click();
  await expect(controls.getByText("100%")).toBeVisible();
  await page.locator('canvas[aria-label="PDF page 1"]').click();
  await page.keyboard.press("Control+=");
  await expect(controls.getByText("110%")).toBeVisible();
  await page.keyboard.press("Control+-");
  await expect(controls.getByText("100%")).toBeVisible();
  await page.keyboard.press("Control+0");
  await expect(controls.getByText("100%")).toBeVisible();
  await pdfPreview.getByLabel("PDF pages").evaluate((node) => {
    const start = new Event("gesturestart", { bubbles: false, cancelable: true });
    Object.defineProperty(start, "scale", { value: 1 });
    node.dispatchEvent(start);
    const change = new Event("gesturechange", { bubbles: false, cancelable: true });
    Object.defineProperty(change, "scale", { value: 1.4 });
    node.dispatchEvent(change);
  });
  await expect(controls.getByText("140%")).toBeVisible();
  await controls.getByRole("button", { name: "Reset zoom" }).click();
  await expect(controls.getByText("100%")).toBeVisible();

  const controlsTop = await controls.evaluate((node) => Math.round(node.getBoundingClientRect().top));
  const scrollState = await pdfPreview.getByLabel("PDF pages").evaluate((node) => {
    node.scrollTop = 520;
    node.scrollLeft = 420;
    return { left: node.scrollLeft, top: node.scrollTop };
  });
  expect(scrollState.left).toBeGreaterThan(0);
  expect(scrollState.top).toBeGreaterThan(0);
  await expect(controls).toBeVisible();
  const controlsTopAfterScroll = await controls.evaluate((node) => Math.round(node.getBoundingClientRect().top));
  expect(Math.abs(controlsTopAfterScroll - controlsTop)).toBeLessThanOrEqual(1);
  const zoomIn = controls.getByRole("button", { name: "Zoom in" });
  const zoomButtonReceivesPointer = await zoomIn.evaluate((button) => {
    const rect = button.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2);
    return hit === button || button.contains(hit);
  });
  expect(zoomButtonReceivesPointer).toBe(true);
  await zoomIn.click();
  await expect(controls.getByText("110%")).toBeVisible();
  await expect(pageLimit).toBeVisible({ timeout: 100 });
  await expect(page.locator('canvas[aria-label^="PDF page"]')).toHaveCount(20, { timeout: 100 });
  await pdfPreview.getByLabel("PDF pages").dispatchEvent("wheel", {
    deltaY: -120,
    ctrlKey: true,
    bubbles: true,
    cancelable: true
  });
  await expect(controls.getByText("120%")).toBeVisible();
  await expect(page.locator('canvas[aria-label^="PDF page"]')).toHaveCount(20, { timeout: 100 });
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(initialWidth);
});

test("Files tab PDF controls stay usable in compact previews", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  daemon.seedBinaryFile("/tmp/puffer/compact-long.pdf", makePdfBase64("Compact long PDF preview", 29, 960, 620));
  await daemon.install(page);
  await page.addInitScript(() => {
    window.localStorage.setItem("puffer-desktop:tweaks", JSON.stringify({ theme: "dark" }));
  });
  await page.setViewportSize({ width: 980, height: 620 });
  await daemon.open(page);

  await page.getByRole("button", { name: /^Browser regression\b/ }).first().click();
  await openFilesPanel(page);
  await page.addStyleTag({
    content: ".pf-files-pane .tree { width: 420px; }"
  });

  await page.getByRole("button", { name: "compact-long.pdf" }).click();
  const pdfPreview = page.getByLabel("PDF preview");
  await expect(pdfPreview).toBeVisible();
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');

  const pageLimit = pdfPreview.getByText("Showing first 20 of 29 pages.");
  const controls = pdfPreview.getByRole("group", { name: "PDF zoom controls" });
  const pages = pdfPreview.getByLabel("PDF pages");
  await expect(pageLimit).toBeVisible();
  await expect(controls).toBeVisible();
  await expect(pages).toBeVisible();

  const metrics = await pageLimit.evaluate((node) => {
    const parseRgb = (value: string): [number, number, number] => {
      const match = value.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
      if (!match) return [0, 0, 0];
      return [Number(match[1]), Number(match[2]), Number(match[3])];
    };
    const luminance = ([red, green, blue]: [number, number, number]): number => {
      const channels = [red, green, blue].map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.03928
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
    };
    const status = node.closest(".pdf-status") as HTMLElement | null;
    const statusElement = status ?? (node as HTMLElement);
    const preview = statusElement.closest('[aria-label="PDF preview"]') as HTMLElement | null;
    const controlsRow = statusElement.closest(".pdf-controls-row") as HTMLElement | null;
    const pagesRegion = preview?.querySelector(".pdf-page-scroll") as HTMLElement | null;
    const zoomIn = preview?.querySelector('button[aria-label="Zoom in"]') as HTMLElement | null;
    const zoomRange = preview?.querySelector<HTMLInputElement>(".pdf-zoom-range") ?? null;
    const statusStyle = getComputedStyle(statusElement);
    const controlsStyle = controlsRow ? getComputedStyle(controlsRow) : null;
    const previewStyle = preview ? getComputedStyle(preview) : null;
    const foreground = luminance(parseRgb(statusStyle.color));
    const background = luminance(parseRgb(statusStyle.backgroundColor));
    const statusRect = statusElement.getBoundingClientRect();
    const controlsRect = controlsRow?.getBoundingClientRect();
    const pagesRect = pagesRegion?.getBoundingClientRect();
    const zoomRect = zoomIn?.getBoundingClientRect();
    const rangeRect = zoomRange?.getBoundingClientRect();
    const zoomHit = zoomRect
      ? document.elementFromPoint(zoomRect.left + zoomRect.width / 2, zoomRect.top + zoomRect.height / 2)
      : null;
    const rangeHit = rangeRect
      ? document.elementFromPoint(rangeRect.left + rangeRect.width / 2, rangeRect.top + rangeRect.height / 2)
      : null;
    return {
      ratio: (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05),
      statusBackground: statusStyle.backgroundColor,
      statusColor: statusStyle.color,
      controlsBackground: controlsStyle?.backgroundColor ?? "",
      previewBackground: previewStyle?.backgroundColor ?? "",
      statusWidth: Math.round(statusRect.width),
      controlsWidth: Math.round(controlsRect?.width ?? 0),
      controlsTop: Math.round(controlsRect?.top ?? 0),
      pagesTop: Math.round(pagesRect?.top ?? 0),
      pagesHeight: Math.round(pagesRegion?.clientHeight ?? 0),
      pagesScrollable: Boolean(pagesRegion && pagesRegion.scrollHeight > pagesRegion.clientHeight),
      zoomHit: zoomHit === zoomIn || Boolean(zoomIn?.contains(zoomHit)),
      rangeHit: rangeHit === zoomRange || Boolean(zoomRange?.contains(rangeHit))
    };
  });
  expect(metrics.ratio).toBeGreaterThanOrEqual(7);
  expect(metrics.statusBackground).not.toBe(metrics.statusColor);
  expect(metrics.statusBackground).not.toBe(metrics.controlsBackground);
  expect(metrics.statusBackground).not.toBe(metrics.previewBackground);
  expect(metrics.statusWidth).toBeGreaterThan(metrics.controlsWidth - 48);
  expect(metrics.pagesTop).toBeGreaterThan(metrics.controlsTop);
  expect(metrics.pagesHeight).toBeGreaterThan(120);
  expect(metrics.pagesScrollable).toBe(true);
  expect(metrics.zoomHit).toBe(true);
  expect(metrics.rangeHit).toBe(true);

  const initialWidth = await page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
    Math.round(canvas.getBoundingClientRect().width)
  );
  await controls.getByRole("button", { name: "Zoom in" }).click();
  await expect(controls.getByText("110%")).toBeVisible();
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(initialWidth);
});

test("Files tab PDF limit badge and zoom controls stay obvious in narrow light previews", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  daemon.seedBinaryFile("/tmp/puffer/narrow-long.pdf", makePdfBase64("Narrow long PDF preview", 29, 960, 620));
  await daemon.install(page);
  await page.setViewportSize({ width: 820, height: 540 });
  await daemon.open(page);

  await page.getByRole("button", { name: /^Browser regression\b/ }).first().click();
  await openFilesPanel(page);
  await page.addStyleTag({
    content: ".pf-files-pane .tree { width: 360px; }"
  });

  await page.getByRole("button", { name: "narrow-long.pdf" }).click();
  const pdfPreview = page.getByLabel("PDF preview");
  await expect(pdfPreview).toBeVisible();
  await expectCanvasHasInk(page, 'canvas[aria-label="PDF page 1"]');

  const pageLimit = pdfPreview.getByText("Showing first 20 of 29 pages.");
  const controls = pdfPreview.getByRole("group", { name: "PDF zoom controls" });
  const zoomIn = controls.getByRole("button", { name: "Zoom in" });
  const zoomSlider = pdfPreview.getByLabel("PDF zoom level");
  await expect(pageLimit).toBeVisible();
  await expect(controls).toBeVisible();
  await expect(zoomSlider).toBeVisible();

  const metrics = await pageLimit.evaluate((node) => {
    const parseRgb = (value: string): [number, number, number] => {
      const match = value.match(/rgba?\((\d+),\s*(\d+),\s*(\d+)/);
      if (!match) return [0, 0, 0];
      return [Number(match[1]), Number(match[2]), Number(match[3])];
    };
    const luminance = ([red, green, blue]: [number, number, number]): number => {
      const channels = [red, green, blue].map((channel) => {
        const normalized = channel / 255;
        return normalized <= 0.03928
          ? normalized / 12.92
          : ((normalized + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * channels[0] + 0.7152 * channels[1] + 0.0722 * channels[2];
    };
    const status = node.closest(".pdf-status") as HTMLElement | null;
    const statusElement = status ?? (node as HTMLElement);
    const preview = statusElement.closest('[aria-label="PDF preview"]') as HTMLElement | null;
    const controlsRow = statusElement.closest(".pdf-controls-row") as HTMLElement | null;
    const toolbar = preview?.querySelector(".pdf-toolbar") as HTMLElement | null;
    const zoomButton = preview?.querySelector('button[aria-label="Zoom in"]') as HTMLElement | null;
    const zoomRange = preview?.querySelector<HTMLInputElement>(".pdf-zoom-range") ?? null;
    const statusStyle = getComputedStyle(statusElement);
    const previewStyle = preview ? getComputedStyle(preview) : null;
    const foreground = luminance(parseRgb(statusStyle.color));
    const background = luminance(parseRgb(statusStyle.backgroundColor));
    const controlsRect = controlsRow?.getBoundingClientRect();
    const toolbarRect = toolbar?.getBoundingClientRect();
    const rangeRect = zoomRange?.getBoundingClientRect();
    const zoomRect = zoomButton?.getBoundingClientRect();
    const zoomHit = zoomRect
      ? document.elementFromPoint(zoomRect.left + zoomRect.width / 2, zoomRect.top + zoomRect.height / 2)
      : null;
    const rangeHit = rangeRect
      ? document.elementFromPoint(rangeRect.left + rangeRect.width / 2, rangeRect.top + rangeRect.height / 2)
      : null;
    return {
      ratio: (Math.max(foreground, background) + 0.05) / (Math.min(foreground, background) + 0.05),
      backgroundLuminance: background,
      statusBackground: statusStyle.backgroundColor,
      previewBackground: previewStyle?.backgroundColor ?? "",
      controlsWidth: Math.round(controlsRect?.width ?? 0),
      toolbarWidth: Math.round(toolbarRect?.width ?? 0),
      rangeWidth: Math.round(rangeRect?.width ?? 0),
      zoomHit: zoomHit === zoomButton || Boolean(zoomButton?.contains(zoomHit)),
      rangeHit: rangeHit === zoomRange || Boolean(zoomRange?.contains(rangeHit))
    };
  });
  expect(metrics.ratio).toBeGreaterThanOrEqual(7);
  expect(metrics.backgroundLuminance).toBeGreaterThan(0.45);
  expect(metrics.statusBackground).not.toBe(metrics.previewBackground);
  expect(metrics.toolbarWidth).toBeGreaterThan(metrics.controlsWidth - 40);
  expect(metrics.rangeWidth).toBeGreaterThan(110);
  expect(metrics.zoomHit).toBe(true);
  expect(metrics.rangeHit).toBe(true);

  const initialWidth = await page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
    Math.round(canvas.getBoundingClientRect().width)
  );
  await zoomIn.click();
  await expect(controls.getByText("110%")).toBeVisible();
  await expect.poll(async () =>
    page.locator('canvas[aria-label="PDF page 1"]').evaluate((canvas) =>
      Math.round(canvas.getBoundingClientRect().width)
    )
  ).toBeGreaterThan(initialWidth);
  await zoomSlider.evaluate((input) => {
    const range = input as HTMLInputElement;
    range.value = "130";
    range.dispatchEvent(new Event("input", { bubbles: true }));
  });
  await expect(controls.getByText("130%")).toBeVisible();
});

test("Files tab shows PDF text fallback while renderer assets are still loading", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  let delayedRendererRequests = 0;
  await page.route("**/*pdfjs-dist*", async (route) => {
    delayedRendererRequests += 1;
    await new Promise((resolve) => setTimeout(resolve, 1_500));
    await route.abort();
  });
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("button", { name: "sample.pdf" }).click();
  await expect(page.getByText("Loading PDF renderer...")).toBeVisible();
  await expect(page.getByLabel("PDF text fallback")).toContainText("Puffer PDF preview", {
    timeout: 700
  });
  expect(delayedRendererRequests).toBeGreaterThan(0);
});

test("Files tab shows PDF text fallback when renderer assets fail to load", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  await page.route("**/*pdfjs-dist*", (route) => route.abort());
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("button", { name: "tex-garbage.pdf" }).click();
  await expect(page.getByLabel("PDF preview")).toBeVisible();
  await expect(page.getByText("PDF renderer failed:")).toBeVisible();
  const fallback = page.getByLabel("PDF text fallback");
  await expect(fallback).toContainText("Clean PDF preview");
  await expect(fallback).not.toContainText("EXTRACTED TEXT");
  await expect(fallback).not.toContainText("CIDInit");
  await expect(fallback).not.toContainText("TeX-T1-0");
});

test("Files tab keeps raw editing available for previewed text files", async ({ page }) => {
  const daemon = new FakeDaemon();
  seedPreviewFiles(daemon);
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("button", { name: "README.md" }).click();
  await expect(page.getByLabel("Markdown preview")).toContainText("Project Notes");

  await page.getByRole("button", { name: "Raw" }).click();
  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue(/# Project Notes/);

  const draft = "# Project Notes\n\n- Edited in raw mode\n";
  await editor.fill(draft);
  await page.getByRole("button", { name: "Save" }).click();

  const request = await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/README.md"
  );
  expect(request.params).toMatchObject({
    path: "/tmp/puffer/README.md",
    content: draft
  });

  await page.getByRole("button", { name: "Preview" }).click();
  await expect(page.getByLabel("Markdown preview")).toContainText("Edited in raw mode");
});

test("Files tab applies the first watch event before fs_watch resolves", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("fs_watch", () => true, 250);
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");

  const watchRequest = await daemon.waitForRequest("fs_watch");
  expect(watchRequest.params).toMatchObject({
    paths: ["/tmp/puffer"],
    recursive: true
  });

  const updated = "fn main() {\n    println!(\"watch refreshed\");\n}\n";
  daemon.seedFile("/tmp/puffer/src/main.rs", updated);
  daemon.emit("workspace:fs:changed", {
    watchId: String(watchRequest.params.watchId ?? "watch-fixture"),
    paths: ["/tmp/puffer/src/main.rs"],
    changedAtMs: Date.now()
  });

  await expect(editor).toHaveValue(updated);
});

test("Files tab starts the next watch without waiting for old unwatch", async ({ page }) => {
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-files-watch-a",
        displayName: "Files watch A",
        title: "Files watch A",
        cwd: "/tmp/watch-a",
        folderPath: "/tmp/watch-a",
        timeline: []
      },
      {
        sessionId: "session-files-watch-b",
        displayName: "Files watch B",
        title: "Files watch B",
        cwd: "/tmp/watch-b",
        folderPath: "/tmp/watch-b",
        timeline: []
      }
    ]
  });
  daemon.delayResponse("fs_unwatch", () => true, 500);
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files watch A\b/ })
    .click();
  await openFilesPanel(page);
  await daemon.waitForRequest("fs_watch", (request) =>
    Array.isArray(request.params.paths) &&
    request.params.paths.includes("/tmp/watch-a")
  );

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files watch B\b/ })
    .click();
  await daemon.waitForRequest("fs_unwatch");
  await page.waitForTimeout(80);

  expect(daemon.requests.some((request) =>
    request.method === "fs_watch" &&
    Array.isArray(request.params.paths) &&
    request.params.paths.includes("/tmp/watch-b")
  )).toBe(true);
});

test("Files tab releases save state after switching tabs mid-save", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  const mainDraft = "fn main() {\n    println!(\"background save\");\n}\n";
  await editor.fill(mainDraft);
  daemon.delayResponse(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs",
    250
  );
  await page.getByRole("button", { name: "Save" }).click();

  await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs"
  );
  await page.getByRole("tab", { name: /lib\.rs/ }).click();

  await expect(page.getByRole("tab", { name: /main\.rs/ }).locator(".dirty-dot")).toHaveCount(0);
  await expect(editor).toHaveValue("pub fn fixture() {}\n");

  const libDraft = "pub fn fixture() {\n    println!(\"after save\");\n}\n";
  await editor.fill(libDraft);
  await expect(page.getByRole("button", { name: "Save" })).toBeEnabled();
});

test("Files editor keeps global find shortcuts while focused", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await editor.focus();
  await expect(editor).toBeFocused();

  await page.keyboard.press("Control+F");

  await expect(page.getByRole("search", { name: "Find in agent view" })).toHaveCount(0);
  await expect(editor).toBeFocused();
});

test("Files tab keeps dirty edits visible after save failure", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");

  const draft = "fn main() {\n    println!(\"retry me\");\n}\n";
  await editor.fill(draft);
  daemon.failNext("write_file", "disk full");
  await page.getByRole("button", { name: "Save" }).click();

  await daemon.waitForRequest(
    "write_file",
    (candidate) => candidate.params.path === "/tmp/puffer/src/main.rs"
  );
  await expect(page.locator(".save-error")).toContainText("disk full");
  await expect(page.getByRole("button", { name: "Save" })).toBeVisible();
  await expect(page.locator(".file-tab.active .dirty-dot")).toBeVisible();
  await expect(editor).toHaveValue(draft);
});

test("Files tab ignores late save failures from the previous session", async ({ page }) => {
  const path = "/tmp/puffer/src/main.rs";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-files-save-a",
        displayName: "Files save A",
        title: "Files save A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        timeline: []
      },
      {
        sessionId: "session-files-save-b",
        displayName: "Files save B",
        title: "Files save B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        timeline: []
      }
    ]
  });
  daemon.delayFailure(
    "write_file",
    (request) => request.params.path === path,
    "stale save from Files A",
    250
  );
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files save A\b/ })
    .click();
  await openFilesPanel(page);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");
  await editor.fill("fn main() {\n    println!(\"alpha draft\");\n}\n");
  await page.getByRole("button", { name: "Save", exact: true }).click();
  await daemon.waitForRequest("write_file", (request) => request.params.path === path);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files save B\b/ })
    .click();
  await openFilesPanel(page);
  await daemon.waitForRequest(
    "read_file",
    (request) =>
      request.params.path === path &&
      daemon.requests.filter((item) => item.method === "read_file" && item.params.path === path)
        .length >= 2
  );

  await expect(editor).toHaveValue("fn main() {}\n");
  await page.waitForTimeout(300);

  await expect(page.locator(".save-error")).toHaveCount(0);
  await expect(editor).toHaveValue("fn main() {}\n");
});

test("Files tab opens symbol context from the editor cursor", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("tab", { name: /lib\.rs/ }).click();
  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("pub fn fixture() {}\n");
  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.focus();
    textarea.setSelectionRange(9, 9);
  });
  await editor.press("ArrowRight");

  const inspect = await daemon.waitForRequest(
    "lsp_inspect",
    (candidate) => candidate.params.path === "/tmp/puffer/src/lib.rs"
  );
  expect(inspect.params).toMatchObject({
    path: "/tmp/puffer/src/lib.rs",
    cwd: "/tmp/puffer",
    line: 0
  });

  const popup = page.getByLabel("Symbol references");
  await expect(popup).toBeVisible();
  await expect(popup.locator(".symbol")).toContainText("fixture");
  await expect(popup.getByText("fixture() -> demo value")).toBeVisible();
  await expect(popup.locator(".lsp-location")).toHaveCount(2);
  await expect(popup.locator(".lsp-location").first()).toContainText("src/lib.rs:1:8");

  await popup.getByRole("button", { name: "Close symbol popup" }).click();
  await expect(popup).toHaveCount(0);
});

test("Files tab jumps to the clicked LSP location line", async ({ page }) => {
  const path = "/tmp/puffer/src/lib.rs";
  const content = [
    "pub fn fixture() {}",
    ...Array.from({ length: 28 }, (_, index) => `// filler ${index + 2}`),
    "pub fn target_line() {}",
    ...Array.from({ length: 10 }, (_, index) => `// tail ${index + 31}`)
  ].join("\n") + "\n";
  const targetLineOffset = content
    .split("\n")
    .slice(0, 29)
    .reduce((offset, line) => offset + line.length + 1, 0);
  const targetOffset = targetLineOffset + 4;
  const daemon = new FakeDaemon();
  daemon.seedFile(path, content);
  daemon.setLspLocation(path, "src/lib.rs:30:5");
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("tab", { name: /lib\.rs/ }).click();
  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue(content);
  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.focus();
    textarea.setSelectionRange(9, 9);
  });
  await editor.press("ArrowRight");
  await daemon.waitForRequest(
    "lsp_inspect",
    (candidate) => candidate.params.path === path
  );

  const popup = page.getByLabel("Symbol references");
  await expect(popup).toBeVisible();
  const location = popup.locator(".lsp-location").filter({ hasText: "src/lib.rs:30:5" }).first();
  await expect(location).toBeVisible();
  await location.click();

  await expect(editor).toBeFocused();
  await expect
    .poll(() => editor.evaluate((node) => (node as HTMLTextAreaElement).selectionStart))
    .toBe(targetOffset);
});

test("Files tab ignores stale symbol inspect results after switching files", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "lsp_inspect",
    (request) => request.params.path === "/tmp/puffer/src/lib.rs",
    120
  );
  await daemon.install(page);
  await daemon.open(page);

  await openRegressionAgent(page);
  await openFilesPanel(page);

  await page.getByRole("tab", { name: /lib\.rs/ }).click();
  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("pub fn fixture() {}\n");
  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.focus();
    textarea.setSelectionRange(9, 9);
  });
  await editor.press("ArrowRight");
  await daemon.waitForRequest(
    "lsp_inspect",
    (request) => request.params.path === "/tmp/puffer/src/lib.rs"
  );

  await page.getByRole("tab", { name: /main\.rs/ }).click();
  await expect(editor).toHaveValue("fn main() {}\n");
  await editor.evaluate((node) => {
    const textarea = node as HTMLTextAreaElement;
    textarea.focus();
    textarea.setSelectionRange(3, 3);
  });
  await editor.press("ArrowRight");
  await daemon.waitForRequest(
    "lsp_inspect",
    (request) => request.params.path === "/tmp/puffer/src/main.rs"
  );

  const popup = page.getByLabel("Symbol references");
  await expect(popup).toBeVisible();
  await expect(popup.locator(".symbol")).toContainText("main");
  await expect(popup.getByText("main() -> demo value")).toBeVisible();

  await page.waitForTimeout(170);
  await expect(popup.locator(".symbol")).toContainText("main");
  await expect(popup.getByText("main() -> demo value")).toBeVisible();
  await expect(popup.getByText("fixture() -> demo value")).toHaveCount(0);
  await expect(popup.locator(".lsp-location").first()).toContainText("src/main.rs:1:4");
});

test("Files tab does not reopen a linked file from the previous session", async ({ page }) => {
  const linkedPath = "/tmp/project-a/src/main.rs";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-files-a",
        displayName: "Files A",
        title: "Files A",
        cwd: "/tmp/project-a",
        folderPath: "/tmp/project-a",
        timeline: [
          {
            kind: "assistant_message",
            id: "files-a-link",
            text: `Open [alpha main](${linkedPath}) for context.`
          }
        ]
      },
      {
        sessionId: "session-files-b",
        displayName: "Files B",
        title: "Files B",
        cwd: "/tmp/project-b",
        folderPath: "/tmp/project-b",
        timeline: [
          {
            kind: "assistant_message",
            id: "files-b-note",
            text: "This session should not inherit linked files from Files A."
          }
        ]
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files A\b/ })
    .click();
  await page.getByRole("link", { name: "alpha main" }).click();
  await daemon.waitForRequest("read_file", (request) => request.params.path === linkedPath);

  await page.locator(".pf-agent-tabs").getByRole("button", { name: "Chat", exact: true }).click();
  const linkedReadsBefore = daemon.requests.filter(
    (request) => request.method === "read_file" && request.params.path === linkedPath
  ).length;

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files B\b/ })
    .click();
  await openFilesPanel(page);
  await page.waitForTimeout(150);

  const linkedReadsAfter = daemon.requests.filter(
    (request) => request.method === "read_file" && request.params.path === linkedPath
  ).length;
  expect(linkedReadsAfter).toBe(linkedReadsBefore);
  await expect(page.locator(".viewer-head .path", { hasText: linkedPath })).toHaveCount(0);
});

test("Files tab jumps to the line from a chat file link", async ({ page }) => {
  const linkedPath = "/tmp/puffer/src/main.rs";
  const content = `${Array.from({ length: 40 }, (_, index) => `line ${index + 1}`).join("\n")}\n`;
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-files-line-link",
        displayName: "Files line link",
        title: "Files line link",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        timeline: [
          {
            kind: "assistant_message",
            id: "files-line-link",
            text: `Open [main line 30](${linkedPath}:30) before editing.`
          }
        ]
      }
    ]
  });
  (daemon as unknown as { files: Map<string, string> }).files.set(linkedPath, content);
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files line link\b/ })
    .click();
  await page.getByRole("link", { name: "main line 30" }).click();
  await daemon.waitForRequest("read_file", (request) => request.params.path === linkedPath);

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue(content);
  const lineThirtyOffset = Array.from({ length: 29 }, (_, index) => `line ${index + 1}\n`).join("").length;
  await expect.poll(async () => editor.evaluate((node) => (node as HTMLTextAreaElement).selectionStart)).toBe(lineThirtyOffset);
  await expect(editor).toBeFocused();
});

test("Files tab ignores late read failures from the previous session", async ({ page }) => {
  const path = "/tmp/puffer/src/main.rs";
  const daemon = new FakeDaemon({
    sessions: [
      {
        sessionId: "session-files-read-a",
        displayName: "Files read A",
        title: "Files read A",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        timeline: []
      },
      {
        sessionId: "session-files-read-b",
        displayName: "Files read B",
        title: "Files read B",
        cwd: "/tmp/puffer",
        folderPath: "/tmp/puffer",
        timeline: []
      }
    ]
  });
  daemon.delayFailure(
    "read_file",
    (request) => request.params.path === path,
    "stale read from Files A",
    250
  );
  await daemon.install(page);
  await daemon.open(page);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files read A\b/ })
    .click();
  await openFilesPanel(page);
  await daemon.waitForRequest("read_file", (request) => request.params.path === path);

  await page
    .locator(".pf-sidebar-agents-list")
    .getByRole("button", { name: /^Files read B\b/ })
    .click();
  await openFilesPanel(page);
  await daemon.waitForRequest(
    "read_file",
    (request) =>
      request.params.path === path &&
      daemon.requests.filter((item) => item.method === "read_file" && item.params.path === path)
        .length >= 2
  );

  const editor = page.getByLabel("Edit file contents");
  await expect(editor).toHaveValue("fn main() {}\n");
  await page.waitForTimeout(300);

  await expect(page.locator(".viewer-msg.err")).toHaveCount(0);
  await expect(editor).toHaveValue("fn main() {}\n");
});

test("New agent modal closes with Escape", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await page.keyboard.press("Escape");

  await expect(dialog).toHaveCount(0);
});

test("New agent modal receives and traps keyboard focus", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await expectFocusInside(dialog);
  await expectTabFocusTrapped(page, dialog, 8);
  await page.keyboard.press("Shift+Tab");
  await expectFocusInside(dialog);
});

test("Create Project modal receives and traps keyboard focus", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);

  await expectFocusInside(dialog);
  await expectTabFocusTrapped(page, dialog, 16);
  await page.keyboard.press("Shift+Tab");
  await expectFocusInside(dialog);
});

test("new agent provider choice is used for the first turn", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("radio", { name: /Anthropic/ }).click();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "anthropic"
  });

  await expect(page.locator(".pf-composer textarea")).toBeVisible();
  await page.locator(".pf-composer textarea").fill("Hello from Anthropic");
  await page.getByRole("button", { name: "Send" }).click();

  const turnRequest = await daemon.waitForRequest(
    "run_agent_turn",
    (request) => request.params.message === "Hello from Anthropic"
  );
  expect(turnRequest.params).toMatchObject({
    providerId: "anthropic",
    modelId: "test-model"
  });
});

test("new agent provider choice does not create a session until Start agent", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await dialog.getByRole("radio", { name: /Anthropic/ }).click();
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Anthropic/ })).toBeChecked();
  await expect(page.locator(".pf-agent-detail")).toHaveCount(0);
  await page.waitForTimeout(80);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(0);

  await dialog.getByRole("button", { name: "Start agent" }).click();
  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "anthropic"
  });
});

test("new agent ignores repeated start clicks while creating", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("create_session", () => true, 250);
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();

  await dialog.getByRole("button", { name: "Start agent" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest("create_session");
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(1);
});

test("new agent fallback providers use daemon provider ids", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: canonicalProviderAuth, providers: [] });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Codex/ })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Anthropic/ })).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "openai"
  });
});

test("new agent provider picker only shows authenticated providers", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: codexAuth });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Codex/ })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Anthropic/ })).toHaveCount(0);
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "openai"
  });
});

test("new agent provider picker supports authenticated custom model providers", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: groqAuth, providers: [groqProvider] });
  await daemon.install(page);
  await daemon.open(page);

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByRole("radio", { name: /Groq/ })).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    providerId: "groq"
  });
});

test("new agent explains when no agent provider is authenticated", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  await page.getByRole("button", { name: "New agent in puffer" }).click();
  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await expect(dialog.getByText("Connect a provider in Settings before starting an agent.")).toBeVisible();
  await expect(dialog.getByRole("button", { name: "Start agent" })).toBeDisabled();
  await dialog.getByRole("button", { name: "Start agent" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
  });

  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(0);
});

test("empty workspace can start a new agent in the default workspace", async ({ page }) => {
  const daemon = new FakeDaemon({ sessions: [] });
  await daemon.install(page);
  await daemon.open(page);

  await expect(page.getByRole("heading", { name: "No sessions yet" })).toBeVisible();
  await page.getByRole("button", { name: "New agent in default workspace" }).click();

  const dialog = page.getByRole("dialog", { name: "New agent" });
  await expect(dialog).toBeVisible();
  await dialog.getByRole("button", { name: "Start agent" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer",
    providerId: "openai"
  });
});

test("connect project provider choice includes Anthropic", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);

  await expect(dialog.getByRole("radio", { name: "Anthropic" })).toBeVisible();
  await dialog.getByRole("radio", { name: "Anthropic" }).click();
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer-new-project",
    providerId: "anthropic"
  });
});

test("connect project ignores repeated start clicks while creating", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse("create_session", () => true, 250);
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");

  await dialog.getByRole("button", { name: "Create" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
    (button as HTMLButtonElement).click();
  });

  await daemon.waitForRequest("create_session");
  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(1);
});

test("connect project provider picker only shows authenticated providers", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: codexAuth });
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);

  await expect(dialog.getByRole("radio", { name: "Codex" })).toBeVisible();
  await expect(dialog.getByRole("radio", { name: "Anthropic" })).toHaveCount(0);
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer-new-project",
    providerId: "openai"
  });
});

test("connect project provider picker supports authenticated custom model providers", async ({ page }) => {
  const daemon = new FakeDaemon({ auth: groqAuth, providers: [groqProvider] });
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);
  await expect(dialog.getByRole("radio", { name: /Groq/ })).toBeVisible();
  await dialog.getByLabel("Directory").fill("/tmp/puffer-groq-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  const createRequest = await daemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/puffer-groq-project",
    providerId: "groq"
  });
});

test("connect project requires an authenticated provider before starting", async ({ page }) => {
  const daemon = new FakeDaemon({
    auth: [
      {
        providerId: "github",
        kind: "oauth",
        email: "tester@example.com",
        expiresAtMs: null,
        scopes: [],
        planType: "test",
        organizationName: null
      }
    ]
  });
  await daemon.install(page);
  await daemon.open(page, { allowUnauthenticatedWorkspace: true });

  const dialog = await openCreateProjectDialog(page);
  await expect(dialog.getByText("Connect a provider in Settings before starting a project.")).toBeVisible();

  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await expect(dialog.getByRole("button", { name: "Create" })).toBeDisabled();
  await dialog.getByRole("button", { name: "Create" }).evaluate((button) => {
    (button as HTMLButtonElement).click();
  });

  await page.waitForTimeout(50);
  expect(daemon.requests.filter((request) => request.method === "create_session")).toHaveLength(0);
});

test("connect project remote mode exposes binary override", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("tab", { name: /Remote/ }).click();

  await expect(dialog.getByLabel("Remote binary")).toBeVisible();
  await dialog.getByLabel("Remote binary").fill("/opt/puffer/bin/puffer");
  await expect(dialog.getByLabel("Remote binary")).toHaveValue("/opt/puffer/bin/puffer");
});

test("connect project directory picker ignores stale path responses", async ({ page }) => {
  const daemon = new FakeDaemon();
  daemon.delayResponse(
    "list_dir",
    (request) => request.params.path === "/tmp/puffer",
    220
  );
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("button", { name: "Browse…" }).click();

  const picker = dialog.getByLabel("Choose directory");
  const pickerInput = picker.getByPlaceholder("/Users/me/src");
  await expect(picker).toBeVisible();
  await pickerInput.fill("/tmp/puffer/src");
  await picker.getByRole("button", { name: "Go" }).click();

  await expect(pickerInput).toHaveValue("/tmp/puffer/src");
  await expect(picker.getByText("No child directories.")).toBeVisible();
  await page.waitForTimeout(260);
  await expect(pickerInput).toHaveValue("/tmp/puffer/src");
  await expect(picker.getByText("No child directories.")).toBeVisible();
  await expect(picker.getByRole("button", { name: "src" })).toHaveCount(0);
});

test("connect project mode switch closes the local directory picker", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("button", { name: "Browse…" }).click();

  const picker = dialog.getByLabel("Choose directory");
  await expect(picker).toBeVisible();

  await dialog.getByRole("tab", { name: /Remote/ }).click();

  await expect(picker).toHaveCount(0);
  await expect(dialog.getByLabel("SSH target")).toBeVisible();
});

test("connect project Escape closes directory picker before parent modal", async ({ page }) => {
  const daemon = new FakeDaemon();
  await daemon.install(page);
  await daemon.open(page);

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByLabel("Directory").fill("/tmp/puffer-new-project");
  await dialog.getByRole("button", { name: "Browse…" }).click();

  const picker = dialog.getByLabel("Choose directory");
  await expect(picker).toBeVisible();

  await page.keyboard.press("Escape");

  await expect(dialog).toBeVisible();
  await expect(picker).toHaveCount(0);
  await expect(dialog.getByLabel("Directory")).toHaveValue("/tmp/puffer-new-project");
});

test("failed remote project creation restores the previous daemon", async ({ page }) => {
  const localDaemon = new FakeDaemon();
  localDaemon.failNext("create_session", "remote create failed");

  await localDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: localDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  await localDaemon.waitForRequest("create_session");
  await expect(dialog.locator(".pf-modal-status-row .pf-modal-status-text")).toContainText(
    "remote create failed"
  );

  const activeToken = await page.evaluate(async () => {
    const mod = await import("/src/lib/api/daemonClient.ts");
    return mod.currentDaemonClient()?.handshake.token ?? null;
  });
  expect(activeToken).toBe("test");
});

test("connect project clears remote errors when switching modes", async ({ page }) => {
  const localDaemon = new FakeDaemon();
  localDaemon.failNext("create_session", "remote create failed");

  await localDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: localDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  const staleError = dialog.locator(".pf-modal-status-row", { hasText: "remote create failed" });
  await expect(staleError).toBeVisible();

  await dialog.getByRole("tab", { name: /Local/ }).click();

  await expect(staleError).toHaveCount(0);
  await expect(dialog.getByLabel("Directory")).toBeVisible();
});

test("successful remote project creation adopts remote daemon state", async ({ page }) => {
  const localDaemon = new FakeDaemon({ workspaceRoot: "/tmp/puffer-local" });
  const remoteDaemon = new FakeDaemon({
    url: "ws://127.0.0.1:17778/ws",
    workspaceRoot: "/tmp/puffer-remote"
  });
  await localDaemon.install(page);
  await remoteDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: remoteDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  const createRequest = await remoteDaemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/remote-project",
    providerId: "openai"
  });
  await expect(dialog).toHaveCount(0);

  const active = await page.evaluate(async () => {
    const mod = await import("/src/lib/api/daemonClient.ts");
    const handshake = mod.currentDaemonClient()?.handshake ?? null;
    return handshake
      ? { token: handshake.token, workspaceRoot: handshake.workspaceRoot }
      : null;
  });
  expect(active).toEqual({
    token: "remote-token",
    workspaceRoot: "/tmp/puffer-remote"
  });

  await page.getByRole("button", { name: "Settings" }).click();
  await expect(page.getByRole("heading", { name: "General" })).toBeVisible();
  const settingsPane = page.locator(".pf-settings-pane");
  await expect(settingsPane.locator(".pf-settings-row").filter({ hasText: "Workspace root" })).toContainText(
    "/tmp/puffer-remote"
  );
  await expect(settingsPane.locator(".pf-settings-row").filter({ hasText: "Daemon" })).toContainText(
    remoteDaemon.url
  );

  const localPermissionRequestsBefore = localDaemon.requests.filter(
    (request) => request.method === "list_permissions"
  ).length;
  await page.getByRole("button", { name: "Permissions" }).click();
  await remoteDaemon.waitForRequest("list_permissions");
  expect(
    localDaemon.requests.filter((request) => request.method === "list_permissions")
  ).toHaveLength(localPermissionRequestsBefore);
  await expect(page.getByText("Stored at")).toContainText(
    "/tmp/puffer-remote/.puffer/permissions.json"
  );
});

test("remote project creation uses remote authenticated provider", async ({ page }) => {
  const localDaemon = new FakeDaemon({
    workspaceRoot: "/tmp/puffer-local",
    auth: canonicalProviderAuth
  });
  const remoteDaemon = new FakeDaemon({
    url: "ws://127.0.0.1:17779/ws",
    workspaceRoot: "/tmp/puffer-remote",
    auth: codexAuth
  });
  await localDaemon.install(page);
  await remoteDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: remoteDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("radio", { name: "Anthropic" }).click();
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  const createRequest = await remoteDaemon.waitForRequest("create_session");
  expect(createRequest.params).toMatchObject({
    cwd: "/tmp/remote-project",
    providerId: "openai"
  });
  expect(
    remoteDaemon.requests.some((request) => request.method === "load_settings_snapshot")
  ).toBe(true);
});

test("remote project creation requires a remote authenticated provider", async ({ page }) => {
  const localDaemon = new FakeDaemon({
    workspaceRoot: "/tmp/puffer-local",
    auth: canonicalProviderAuth
  });
  const remoteDaemon = new FakeDaemon({
    url: "ws://127.0.0.1:17780/ws",
    workspaceRoot: "/tmp/puffer-remote",
    auth: []
  });
  await localDaemon.install(page);
  await remoteDaemon.install(page);
  await localDaemon.open(page, {
    extraParams: {
      pufferRemoteBackend: remoteDaemon.url,
      pufferRemoteToken: "remote-token",
      pufferRemoteWorkspaceRoot: "/tmp/puffer-remote"
    }
  });

  const dialog = await openCreateProjectDialog(page);
  await dialog.getByRole("tab", { name: /Remote/ }).click();
  await dialog.getByLabel("SSH target").fill("devbox");
  await dialog.getByLabel("Destination directory").fill("/tmp/remote-project");
  await dialog.getByRole("button", { name: "Create" }).click();

  await remoteDaemon.waitForRequest("load_settings_snapshot");
  await expect(dialog.locator(".pf-modal-status-row .pf-modal-status-text")).toContainText(
    "Connect an agent provider on the remote host before starting a remote project."
  );
  expect(remoteDaemon.requests.some((request) => request.method === "create_session")).toBe(false);

  const activeToken = await page.evaluate(async () => {
    const mod = await import("/src/lib/api/daemonClient.ts");
    return mod.currentDaemonClient()?.handshake.token ?? null;
  });
  expect(activeToken).toBe("test");
});
