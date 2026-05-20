import type { ReadFileResult } from "../../api/desktop";

export type CsvPreview = {
  kind: "csv";
  rows: string[][];
};

export type DocxPreview = {
  kind: "docx";
  paragraphs: string[];
};

export type FilePreview =
  | { kind: "markdown"; html: string }
  | CsvPreview
  | { kind: "pdf"; dataUrl: string }
  | DocxPreview
  | { kind: "pptx"; slides: { title: string; lines: string[] }[] }
  | { kind: "xlsx"; sheets: { name: string; rows: string[][] }[] }
  | { kind: "office-binary"; title: string; message: string };

type ZipEntry = {
  name: string;
  method: number;
  compressedSize: number;
  uncompressedSize: number;
  localHeaderOffset: number;
};

type RelationshipMap = Map<string, string>;

const utf8Decoder = new TextDecoder("utf-8");

/** Return true when the Files pane has a richer preview than the code editor. */
export function hasRichFilePreview(file: ReadFileResult): boolean {
  return previewFormat(file.path) !== "text";
}

/** Build a display preview for common document and data formats. */
export async function buildFilePreview(file: ReadFileResult): Promise<FilePreview | null> {
  const format = previewFormat(file.path);
  switch (format) {
    case "markdown":
      return file.encoding === "utf8" ? { kind: "markdown", html: renderMarkdown(file.content) } : null;
    case "csv":
      return file.encoding === "utf8" ? { kind: "csv", rows: parseCsv(file.content) } : null;
    case "pdf":
      return file.encoding === "base64"
        ? { kind: "pdf", dataUrl: `data:application/pdf;base64,${file.content}` }
        : null;
    case "docx":
      return file.encoding === "base64" ? previewDocx(file.content) : null;
    case "pptx":
      return file.encoding === "base64" ? previewPptx(file.content) : null;
    case "xlsx":
      return file.encoding === "base64" ? previewXlsx(file.content) : null;
    case "legacy-office":
      return legacyOfficePreview(file.path);
    case "text":
      return null;
  }
}

function previewFormat(path: string):
  | "text"
  | "markdown"
  | "csv"
  | "pdf"
  | "docx"
  | "pptx"
  | "xlsx"
  | "legacy-office" {
  const lower = path.toLowerCase();
  if (lower.endsWith(".md") || lower.endsWith(".markdown")) return "markdown";
  if (lower.endsWith(".csv")) return "csv";
  if (lower.endsWith(".pdf")) return "pdf";
  if (lower.endsWith(".docx")) return "docx";
  if (lower.endsWith(".pptx")) return "pptx";
  if (lower.endsWith(".xlsx") || lower.endsWith(".xlsm")) return "xlsx";
  if (lower.endsWith(".doc") || lower.endsWith(".ppt") || lower.endsWith(".xls")) {
    return "legacy-office";
  }
  return "text";
}

function legacyOfficePreview(path: string): FilePreview {
  const lower = path.toLowerCase();
  const title = lower.endsWith(".ppt")
    ? "PowerPoint preview"
    : lower.endsWith(".xls")
      ? "Excel preview"
      : "Word preview";
  return {
    kind: "office-binary",
    title,
    message:
      "Legacy binary Office files are visible in the file pane, but rich text extraction requires saving as DOCX, PPTX, or XLSX."
  };
}

function renderMarkdown(markdown: string): string {
  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  const html: string[] = [];
  let inCode = false;
  let listItems: string[] = [];
  let paragraph: string[] = [];

  const flushParagraph = () => {
    if (paragraph.length === 0) return;
    html.push(`<p>${inlineMarkdown(paragraph.join(" "))}</p>`);
    paragraph = [];
  };
  const flushList = () => {
    if (listItems.length === 0) return;
    html.push(`<ul>${listItems.map((item) => `<li>${inlineMarkdown(item)}</li>`).join("")}</ul>`);
    listItems = [];
  };

  for (const raw of lines) {
    const line = raw.trimEnd();
    if (line.startsWith("```")) {
      flushParagraph();
      flushList();
      if (inCode) {
        html.push("</code></pre>");
      } else {
        html.push("<pre><code>");
      }
      inCode = !inCode;
      continue;
    }
    if (inCode) {
      html.push(`${escapeHtml(raw)}\n`);
      continue;
    }
    if (!line.trim()) {
      flushParagraph();
      flushList();
      continue;
    }
    const heading = line.match(/^(#{1,6})\s+(.+)$/);
    if (heading) {
      flushParagraph();
      flushList();
      const level = heading[1].length;
      html.push(`<h${level}>${inlineMarkdown(heading[2])}</h${level}>`);
      continue;
    }
    const list = line.match(/^[-*+]\s+(.+)$/);
    if (list) {
      flushParagraph();
      listItems.push(list[1]);
      continue;
    }
    const quote = line.match(/^>\s?(.+)$/);
    if (quote) {
      flushParagraph();
      flushList();
      html.push(`<blockquote>${inlineMarkdown(quote[1])}</blockquote>`);
      continue;
    }
    flushList();
    paragraph.push(line.trim());
  }
  flushParagraph();
  flushList();
  if (inCode) html.push("</code></pre>");
  return html.join("");
}

function inlineMarkdown(value: string): string {
  return escapeHtml(value)
    .replace(/`([^`]+)`/g, "<code>$1</code>")
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/\*([^*]+)\*/g, "<em>$1</em>");
}

function escapeHtml(value: string): string {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
}

function parseCsv(content: string): string[][] {
  const rows: string[][] = [];
  let row: string[] = [];
  let cell = "";
  let quoted = false;

  for (let index = 0; index < content.length; index += 1) {
    const char = content[index];
    const next = content[index + 1];
    if (quoted) {
      if (char === '"' && next === '"') {
        cell += '"';
        index += 1;
      } else if (char === '"') {
        quoted = false;
      } else {
        cell += char;
      }
      continue;
    }
    if (char === '"') {
      quoted = true;
    } else if (char === ",") {
      row.push(cell);
      cell = "";
    } else if (char === "\n") {
      row.push(cell);
      rows.push(row);
      row = [];
      cell = "";
    } else if (char !== "\r") {
      cell += char;
    }
  }
  if (cell.length > 0 || row.length > 0 || content.endsWith(",")) {
    row.push(cell);
    rows.push(row);
  }
  return rows.slice(0, 200).map((cells) => cells.slice(0, 40));
}

async function previewDocx(base64: string): Promise<DocxPreview> {
  const entries = await readZip(base64);
  const documentXml = decodeEntry(entries, "word/document.xml");
  const xml = parseXml(documentXml);
  const paragraphs = Array.from(xml.getElementsByTagName("w:p"))
    .map((paragraph) => collectText(paragraph, "w:t").join(""))
    .map((line) => line.trim())
    .filter(Boolean);
  return { kind: "docx", paragraphs: paragraphs.length > 0 ? paragraphs : ["No text found."] };
}

async function previewPptx(base64: string): Promise<FilePreview> {
  const entries = await readZip(base64);
  const slideNames = Array.from(entries.keys())
    .filter((name) => /^ppt\/slides\/slide\d+\.xml$/.test(name))
    .sort(compareSlideNames);
  const slides = slideNames.map((name, index) => {
    const xml = parseXml(decodeEntry(entries, name));
    const lines = collectText(xml.documentElement, "a:t")
      .map((line) => line.trim())
      .filter(Boolean);
    return {
      title: `Slide ${index + 1}`,
      lines: lines.length > 0 ? lines : ["No text found."]
    };
  });
  return { kind: "pptx", slides };
}

async function previewXlsx(base64: string): Promise<FilePreview> {
  const entries = await readZip(base64);
  const sharedStrings = parseSharedStrings(entries);
  const sheetNames = parseWorkbookSheets(entries);
  const worksheetNames = Array.from(entries.keys())
    .filter((name) => /^xl\/worksheets\/sheet\d+\.xml$/.test(name))
    .sort(compareSlideNames);

  const sheets = worksheetNames.map((name, index) => ({
    name: sheetNames.get(name) ?? `Sheet ${index + 1}`,
    rows: parseWorksheet(decodeEntry(entries, name), sharedStrings)
  }));
  return { kind: "xlsx", sheets };
}

function parseSharedStrings(entries: Map<string, Uint8Array>): string[] {
  const raw = entries.get("xl/sharedStrings.xml");
  if (!raw) return [];
  const xml = parseXml(decodeBytes(raw));
  return Array.from(xml.getElementsByTagName("si")).map((item) =>
    collectText(item, "t").join("")
  );
}

function parseWorkbookSheets(entries: Map<string, Uint8Array>): Map<string, string> {
  const workbook = entries.get("xl/workbook.xml");
  const rels = entries.get("xl/_rels/workbook.xml.rels");
  if (!workbook || !rels) return new Map();

  const relationMap: RelationshipMap = new Map();
  const relXml = parseXml(decodeBytes(rels));
  for (const rel of Array.from(relXml.getElementsByTagName("Relationship"))) {
    const id = rel.getAttribute("Id");
    const target = rel.getAttribute("Target");
    if (id && target) relationMap.set(id, normalizeZipPath("xl", target));
  }

  const result = new Map<string, string>();
  const workbookXml = parseXml(decodeBytes(workbook));
  for (const sheet of Array.from(workbookXml.getElementsByTagName("sheet"))) {
    const name = sheet.getAttribute("name");
    const relId = sheet.getAttribute("r:id");
    const target = relId ? relationMap.get(relId) : null;
    if (name && target) result.set(target, name);
  }
  return result;
}

function parseWorksheet(xmlText: string, sharedStrings: string[]): string[][] {
  const xml = parseXml(xmlText);
  const rows: string[][] = [];
  for (const row of Array.from(xml.getElementsByTagName("row")).slice(0, 100)) {
    const cells: string[] = [];
    for (const cell of Array.from(row.getElementsByTagName("c")).slice(0, 40)) {
      const column = columnIndex(cell.getAttribute("r") ?? "");
      const value = cellValue(cell, sharedStrings);
      while (cells.length < column) cells.push("");
      cells[column] = value;
    }
    rows.push(cells);
  }
  return rows;
}

function cellValue(cell: Element, sharedStrings: string[]): string {
  const type = cell.getAttribute("t");
  if (type === "inlineStr") return collectText(cell, "t").join("");
  const value = cell.getElementsByTagName("v")[0]?.textContent ?? "";
  if (type === "s") return sharedStrings[Number(value)] ?? value;
  return value;
}

function collectText(node: Element, tagName: string): string[] {
  return Array.from(node.getElementsByTagName(tagName)).map((child) => child.textContent ?? "");
}

function parseXml(text: string): Document {
  return new DOMParser().parseFromString(text, "application/xml");
}

async function readZip(base64: string): Promise<Map<string, Uint8Array>> {
  const bytes = base64ToBytes(base64);
  const directoryOffset = findCentralDirectoryOffset(bytes);
  const entries = readCentralDirectory(bytes, directoryOffset);
  const result = new Map<string, Uint8Array>();
  for (const entry of entries) {
    result.set(entry.name, await readZipEntry(bytes, entry));
  }
  return result;
}

function findCentralDirectoryOffset(bytes: Uint8Array): number {
  const min = Math.max(0, bytes.length - 0xffff - 22);
  for (let offset = bytes.length - 22; offset >= min; offset -= 1) {
    if (readU32(bytes, offset) === 0x06054b50) {
      return readU32(bytes, offset + 16);
    }
  }
  throw new Error("Office ZIP directory not found");
}

function readCentralDirectory(bytes: Uint8Array, start: number): ZipEntry[] {
  const entries: ZipEntry[] = [];
  let offset = start;
  while (offset + 46 <= bytes.length && readU32(bytes, offset) === 0x02014b50) {
    const method = readU16(bytes, offset + 10);
    const compressedSize = readU32(bytes, offset + 20);
    const uncompressedSize = readU32(bytes, offset + 24);
    const nameLength = readU16(bytes, offset + 28);
    const extraLength = readU16(bytes, offset + 30);
    const commentLength = readU16(bytes, offset + 32);
    const localHeaderOffset = readU32(bytes, offset + 42);
    const name = decodeBytes(bytes.slice(offset + 46, offset + 46 + nameLength));
    entries.push({ name, method, compressedSize, uncompressedSize, localHeaderOffset });
    offset += 46 + nameLength + extraLength + commentLength;
  }
  return entries;
}

async function readZipEntry(bytes: Uint8Array, entry: ZipEntry): Promise<Uint8Array> {
  const header = entry.localHeaderOffset;
  if (readU32(bytes, header) !== 0x04034b50) {
    throw new Error(`Invalid ZIP entry header for ${entry.name}`);
  }
  const nameLength = readU16(bytes, header + 26);
  const extraLength = readU16(bytes, header + 28);
  const dataStart = header + 30 + nameLength + extraLength;
  const compressed = bytes.slice(dataStart, dataStart + entry.compressedSize);
  if (entry.method === 0) return compressed;
  if (entry.method === 8) return inflateRaw(compressed, entry.uncompressedSize);
  throw new Error(`Unsupported ZIP compression method ${entry.method}`);
}

async function inflateRaw(input: Uint8Array, expectedSize: number): Promise<Uint8Array> {
  type DecompressionStreamConstructor = new (format: string) => TransformStream<Uint8Array, Uint8Array>;
  const Decompression = (globalThis as { DecompressionStream?: DecompressionStreamConstructor })
    .DecompressionStream;
  if (!Decompression) {
    throw new Error("This WebView cannot decompress Office previews");
  }
  const payload = input.buffer.slice(
    input.byteOffset,
    input.byteOffset + input.byteLength
  ) as ArrayBuffer;
  const stream = new Blob([payload]).stream().pipeThrough(new Decompression("deflate-raw"));
  const output = new Uint8Array(await new Response(stream).arrayBuffer());
  if (expectedSize > 0 && output.length === 0) {
    throw new Error("Office preview decompressed to an empty document");
  }
  return output;
}

function decodeEntry(entries: Map<string, Uint8Array>, name: string): string {
  const value = entries.get(name);
  if (!value) throw new Error(`${name} not found`);
  return decodeBytes(value);
}

function decodeBytes(bytes: Uint8Array): string {
  return utf8Decoder.decode(bytes);
}

function base64ToBytes(base64: string): Uint8Array {
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function readU16(bytes: Uint8Array, offset: number): number {
  return bytes[offset] | (bytes[offset + 1] << 8);
}

function readU32(bytes: Uint8Array, offset: number): number {
  return (
    bytes[offset] |
    (bytes[offset + 1] << 8) |
    (bytes[offset + 2] << 16) |
    (bytes[offset + 3] << 24)
  ) >>> 0;
}

function compareSlideNames(left: string, right: string): number {
  return numericSuffix(left) - numericSuffix(right) || left.localeCompare(right);
}

function numericSuffix(value: string): number {
  return Number(value.match(/(\d+)\D*$/)?.[1] ?? 0);
}

function columnIndex(ref: string): number {
  const letters = ref.match(/^[A-Z]+/i)?.[0].toUpperCase() ?? "A";
  let value = 0;
  for (const letter of letters) {
    value = value * 26 + letter.charCodeAt(0) - 64;
  }
  return Math.max(0, value - 1);
}

function normalizeZipPath(base: string, target: string): string {
  const parts = (target.startsWith("/") ? target.slice(1) : `${base}/${target}`).split("/");
  const stack: string[] = [];
  for (const part of parts) {
    if (!part || part === ".") continue;
    if (part === "..") stack.pop();
    else stack.push(part);
  }
  return stack.join("/");
}
