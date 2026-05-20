import { decompressSync, inflateSync } from "fflate";
import type { ReadFileResult } from "../../api/desktop";

export type CsvPreview = {
  kind: "csv";
  rows: string[][];
};

export type DocxPreview = {
  kind: "docx";
  paragraphs: string[];
};

export type PdfPreview = {
  kind: "pdf";
  base64: string;
  lines: string[];
};

export type LegacyOfficePreview = {
  kind: "office-binary";
  title: string;
  lines: string[];
  html?: string;
};

export type FilePreview =
  | { kind: "markdown"; html: string }
  | CsvPreview
  | PdfPreview
  | DocxPreview
  | { kind: "pptx"; slides: { title: string; lines: string[] }[] }
  | { kind: "xlsx"; sheets: { name: string; rows: string[][] }[] }
  | LegacyOfficePreview;

type ZipEntry = {
  name: string;
  method: number;
  compressedSize: number;
  uncompressedSize: number;
  localHeaderOffset: number;
};

type RelationshipMap = Map<string, string>;

const utf8Decoder = new TextDecoder("utf-8");
const utf8Encoder = new TextEncoder();
const PDF_TEXT_SCAN_BYTES = 2 * 1024 * 1024;
const PDF_TEXT_MAX_STREAMS = 24;
const PDF_TEXT_MAX_COMPRESSED_STREAM_BYTES = 512 * 1024;
const PDF_TEXT_MAX_DECODED_STREAM_BYTES = 1024 * 1024;

/** Return true when the Files pane has a richer preview than the code editor. */
export function hasRichFilePreview(file: ReadFileResult): boolean {
  return hasRichFilePreviewPath(file.path);
}

/** Return true when the path maps to a richer document preview. */
export function hasRichFilePreviewPath(path: string): boolean {
  return previewFormat(path) !== "text";
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
      return previewPdf(file);
    case "docx":
      return file.encoding === "base64" ? previewDocx(file.content) : null;
    case "pptx":
      return file.encoding === "base64" ? previewPptx(file.content) : null;
    case "xlsx":
      return file.encoding === "base64" ? previewXlsx(file.content) : null;
    case "legacy-office":
      return legacyOfficePreview(file);
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
  if (
    lower.endsWith(".doc") ||
    lower.endsWith(".dot") ||
    lower.endsWith(".rtf") ||
    lower.endsWith(".ppt") ||
    lower.endsWith(".xls")
  ) {
    return "legacy-office";
  }
  return "text";
}

function previewPdf(file: ReadFileResult): PdfPreview | null {
  const base64 =
    file.encoding === "base64"
      ? file.content
      : file.encoding === "utf8"
        ? bytesToBase64(utf8StringToBytes(file.content))
        : null;
  if (!base64) return null;
  const nativeLines = normalizedPdfPreviewLines(file.textPreview ?? [], 200);
  const lines = extractPdfText(base64ToBytes(base64));
  const previewLines = nativeLines.length > 0 ? nativeLines : lines;
  return { kind: "pdf", base64, lines: previewLines.length > 0 ? previewLines : ["No text found."] };
}

function legacyOfficePreview(file: ReadFileResult): LegacyOfficePreview {
  const lower = file.path.toLowerCase();
  const title = lower.endsWith(".ppt")
    ? "Legacy PowerPoint preview"
    : lower.endsWith(".xls")
      ? "Legacy Excel preview"
      : "Legacy Word preview";
  const nativeLines = normalizedProvidedPreviewLines(file.textPreview);
  const nativeHtml = normalizedProvidedPreviewHtml(file.htmlPreview);
  if (nativeLines.length > 0) {
    return {
      kind: "office-binary",
      title,
      lines: nativeLines,
      ...(nativeHtml ? { html: nativeHtml } : {})
    };
  }
  if (nativeHtml) {
    const htmlLines = extractHtmlDocumentText(nativeHtml);
    return {
      kind: "office-binary",
      title,
      lines: htmlLines.length > 0 ? htmlLines : ["No text found."],
      html: nativeHtml
    };
  }
  const bytes =
    file.encoding === "base64" ? base64ToBytes(file.content) : utf8StringToBytes(file.content);
  const lines = extractLegacyOfficeText(bytes);
  return {
    kind: "office-binary",
    title,
    lines: lines.length > 0 ? lines : ["No text found."]
  };
}

function normalizedProvidedPreviewLines(lines: string[] | undefined): string[] {
  if (!lines) return [];
  return normalizePreviewLines(lines, 200);
}

function normalizedProvidedPreviewHtml(html: string | undefined): string | undefined {
  if (!html) return undefined;
  return sanitizePreviewHtml(html);
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

function extractPdfText(bytes: Uint8Array): string[] {
  const streamTexts = decodePdfStreams(bytes.slice(0, PDF_TEXT_SCAN_BYTES));
  const values = streamTexts.flatMap((stream) => extractPdfStrings(stream));
  return normalizedPdfPreviewLines(values, 200);
}

function normalizedPdfPreviewLines(values: string[], limit: number): string[] {
  return normalizePreviewLines(values, limit * 2)
    .filter(isReadablePdfPreviewLine)
    .slice(0, limit);
}

function isReadablePdfPreviewLine(line: string): boolean {
  const trimmed = line.trim();
  if (trimmed.length < 3) return false;
  if (/[\u0000-\u001f\u007f-\u009f]/.test(trimmed)) return false;
  if (isInternalPdfToken(trimmed)) return false;
  const textChars = Array.from(trimmed.matchAll(/[\p{L}\p{N}]/gu)).length;
  if (textChars < 3) return false;
  const symbolChars = Array.from(
    trimmed.matchAll(/[^\p{L}\p{N}\s.,:;!?()[\]\-_/&%+#'"$@]/gu)
  ).length;
  return symbolChars / Math.max(trimmed.length, 1) <= 0.35;
}

function isInternalPdfToken(line: string): boolean {
  const lower = line.toLowerCase();
  if (/^extracted\s+text:?$/i.test(line)) return true;
  if (lower.includes("cidinit")) return true;
  if (lower.includes("tex-t1") || /^tex(?:\b|-)/i.test(line)) return true;
  return /^(?:begin|end)(?:cmap|bfchar|bfrange|codespacerange)$/i.test(line) ||
    /^(?:cmapname|cmaptype|cidsysteminfo|identity-h|adobe-identity-ucs)$/i.test(line);
}

function decodePdfStreams(bytes: Uint8Array): string[] {
  const binary = bytesToBinaryString(bytes);
  const streams: string[] = [];
  const streamMarker = /stream\r?\n?/g;
  let match: RegExpExecArray | null;
  while ((match = streamMarker.exec(binary)) && streams.length < PDF_TEXT_MAX_STREAMS) {
    const streamStart = match.index + match[0].length;
    const streamEnd = binary.indexOf("endstream", streamStart);
    if (streamEnd < 0) break;
    const header = binary.slice(Math.max(0, match.index - 320), match.index);
    const raw = trimPdfStreamBytes(bytes.slice(streamStart, streamEnd));
    if (raw.length <= PDF_TEXT_MAX_COMPRESSED_STREAM_BYTES) {
      const decoded = decodePdfStream(raw, header).slice(0, PDF_TEXT_MAX_DECODED_STREAM_BYTES);
      streams.push(bytesToBinaryString(decoded));
    }
    streamMarker.lastIndex = streamEnd + "endstream".length;
  }
  return streams;
}

function decodePdfStream(bytes: Uint8Array, header: string): Uint8Array {
  if (!/\/FlateDecode\b/.test(header)) return bytes;
  try {
    return decompressSync(bytes);
  } catch (_err) {
    try {
      return inflateSync(bytes);
    } catch (_fallbackErr) {
      return bytes;
    }
  }
}

function trimPdfStreamBytes(bytes: Uint8Array): Uint8Array {
  let start = 0;
  let end = bytes.length;
  while (start < end && (bytes[start] === 0x0a || bytes[start] === 0x0d)) start += 1;
  while (end > start && (bytes[end - 1] === 0x0a || bytes[end - 1] === 0x0d)) end -= 1;
  return bytes.slice(start, end);
}

function extractPdfStrings(stream: string): string[] {
  const values: string[] = [];
  for (let index = 0; index < stream.length; index += 1) {
    const char = stream[index];
    if (char === "(") {
      const literal = readPdfLiteral(stream, index);
      values.push(literal.value);
      index = literal.nextIndex;
    } else if (char === "<" && stream[index + 1] !== "<") {
      const end = stream.indexOf(">", index + 1);
      if (end > index) {
        const decoded = decodePdfHexString(stream.slice(index + 1, end));
        if (decoded) values.push(decoded);
        index = end;
      }
    }
  }
  return values;
}

function readPdfLiteral(input: string, start: number): { value: string; nextIndex: number } {
  let value = "";
  let depth = 1;
  let index = start + 1;
  while (index < input.length && depth > 0) {
    const char = input[index];
    index += 1;
    if (char === "\\") {
      const escaped = readPdfEscape(input, index);
      value += escaped.value;
      index = escaped.nextIndex;
      continue;
    }
    if (char === "(") {
      depth += 1;
      value += char;
      continue;
    }
    if (char === ")") {
      depth -= 1;
      if (depth > 0) value += char;
      continue;
    }
    value += char;
  }
  return { value, nextIndex: index - 1 };
}

function readPdfEscape(input: string, start: number): { value: string; nextIndex: number } {
  const char = input[start];
  if (char == null) return { value: "", nextIndex: start };
  if (char === "\r" || char === "\n") {
    const nextIndex = char === "\r" && input[start + 1] === "\n" ? start + 2 : start + 1;
    return { value: "", nextIndex };
  }
  const mapped = new Map([
    ["n", "\n"],
    ["r", "\r"],
    ["t", "\t"],
    ["b", "\b"],
    ["f", "\f"]
  ]).get(char);
  if (mapped != null) return { value: mapped, nextIndex: start + 1 };
  if (/[0-7]/.test(char)) {
    let octal = char;
    let index = start + 1;
    while (index < start + 3 && /[0-7]/.test(input[index] ?? "")) {
      octal += input[index];
      index += 1;
    }
    return { value: String.fromCharCode(parseInt(octal, 8)), nextIndex: index };
  }
  return { value: char, nextIndex: start + 1 };
}

function decodePdfHexString(input: string): string {
  let hex = input.replace(/\s+/g, "");
  if (hex.length < 2 || /[^0-9a-f]/i.test(hex)) return "";
  if (hex.length % 2 === 1) hex += "0";
  const bytes = new Uint8Array(hex.length / 2);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = parseInt(hex.slice(index * 2, index * 2 + 2), 16);
  }
  if (bytes[0] === 0xfe && bytes[1] === 0xff) return decodeUtf16Bytes(bytes.slice(2), true);
  if (bytes[0] === 0xff && bytes[1] === 0xfe) return decodeUtf16Bytes(bytes.slice(2), false);
  return bytesToBinaryString(bytes);
}

function extractLegacyOfficeText(bytes: Uint8Array): string[] {
  const textDocument = extractLegacyTextDocument(bytes);
  if (textDocument.length > 0) return textDocument;
  const structured = extractCompoundOfficeText(bytes);
  if (structured.length > 0) return structured;
  return normalizePreviewLines(
    [...extractUtf16Runs(bytes, false), ...extractUtf16Runs(bytes, true), ...extractAsciiRuns(bytes)],
    160
  );
}

function extractLegacyTextDocument(bytes: Uint8Array): string[] {
  const text = decodeBytes(bytes);
  const trimmed = text.trimStart();
  if (trimmed.startsWith("{\\rtf")) return extractRtfText(text);
  if (/^(?:<!doctype\s+html\b|<html\b|<head\b|<body\b|<\?xml\b)/i.test(trimmed)) {
    return extractHtmlDocumentText(text);
  }
  return [];
}

function extractRtfText(input: string): string[] {
  let output = "";
  let depth = 0;
  let skippedDepth: number | null = null;

  for (let index = 0; index < input.length; index += 1) {
    const char = input[index];
    if (char === "{") {
      depth += 1;
      continue;
    }
    if (char === "}") {
      if (skippedDepth === depth) skippedDepth = null;
      depth = Math.max(0, depth - 1);
      continue;
    }
    if (skippedDepth != null) continue;
    if (char !== "\\") {
      output += char;
      continue;
    }

    const next = input[index + 1];
    if (next == null) continue;
    if (next === "'" && /[0-9a-f]{2}/i.test(input.slice(index + 2, index + 4))) {
      output += String.fromCharCode(parseInt(input.slice(index + 2, index + 4), 16));
      index += 3;
      continue;
    }
    if (next === "{" || next === "}" || next === "\\") {
      output += next;
      index += 1;
      continue;
    }

    const control = input.slice(index + 1).match(/^([a-zA-Z]+)(-?\d+)? ?/);
    if (!control) {
      index += 1;
      continue;
    }
    const [, word, rawValue] = control;
    index += control[0].length;
    if (word === "par" || word === "line") output += "\n";
    else if (word === "tab") output += "\t";
    else if (word === "u" && rawValue != null) {
      const value = Number(rawValue);
      output += String.fromCharCode(value < 0 ? value + 65536 : value);
    } else if (RTF_DESTINATIONS.has(word)) {
      skippedDepth = depth;
    }
  }

  return normalizePreviewLines(output.split(/\n+/), 160);
}

function extractHtmlDocumentText(input: string): string[] {
  const text = input
    .replace(/<script\b[\s\S]*?<\/script>/gi, " ")
    .replace(/<style\b[\s\S]*?<\/style>/gi, " ")
    .replace(/<br\s*\/?>/gi, "\n")
    .replace(/<\/(?:p|div|h[1-6]|li|tr|section|article)>/gi, "\n")
    .replace(/<[^>]+>/g, " ");
  return normalizePreviewLines(decodeHtmlEntities(text).split(/\n+/), 160);
}

function decodeHtmlEntities(input: string): string {
  const textarea = document.createElement("textarea");
  textarea.innerHTML = input;
  return textarea.value;
}

function sanitizePreviewHtml(input: string): string | undefined {
  const parsed = new DOMParser().parseFromString(input, "text/html");
  inlinePreviewClassStyles(parsed);
  for (const node of Array.from(parsed.querySelectorAll("script, iframe, object, embed, link, meta, style"))) {
    node.remove();
  }
  for (const element of Array.from(parsed.body.querySelectorAll("*"))) {
    for (const attr of Array.from(element.attributes)) {
      const name = attr.name.toLowerCase();
      const value = attr.value.trim();
      if (name.startsWith("on")) {
        element.removeAttribute(attr.name);
      } else if ((name === "href" || name === "src") && /^javascript:/i.test(value)) {
        element.removeAttribute(attr.name);
      } else if (name === "style") {
        const style = sanitizeStyleAttribute(value);
        if (style) element.setAttribute("style", style);
        else element.removeAttribute(attr.name);
      } else if (!["class", "colspan", "rowspan", "title", "alt"].includes(name)) {
        element.removeAttribute(attr.name);
      }
    }
  }
  const html = parsed.body.innerHTML.trim();
  return html ? html.slice(0, 200_000) : undefined;
}

function inlinePreviewClassStyles(document: Document): void {
  const classStyles = collectPreviewClassStyles(document);
  if (classStyles.size === 0) return;
  for (const element of Array.from(document.body.querySelectorAll("*"))) {
    const rules = Array.from(element.classList).flatMap((className) => classStyles.get(className) ?? []);
    if (rules.length === 0) continue;
    const existing = element.getAttribute("style") ?? "";
    const style = sanitizeStyleAttribute([...rules, existing].filter(Boolean).join("; "));
    if (style) element.setAttribute("style", style);
  }
}

function collectPreviewClassStyles(document: Document): Map<string, string[]> {
  const classStyles = new Map<string, string[]>();
  for (const style of Array.from(document.querySelectorAll("style"))) {
    const css = style.textContent ?? "";
    const rules = css.matchAll(/([^{}]+)\{([^}]*)\}/g);
    for (const match of rules) {
      const selector = match[1].trim();
      const className = selector.match(/^(?:[a-z][\w-]*)?\.([A-Za-z0-9_-]+)$/i)?.[1];
      if (!className) continue;
      const declarations = sanitizeStyleAttribute(match[2]);
      if (!declarations) continue;
      classStyles.set(className, [...(classStyles.get(className) ?? []), declarations]);
    }
  }
  return classStyles;
}

function sanitizeStyleAttribute(input: string): string {
  const allowed = new Set([
    "background-color",
    "color",
    "font",
    "font-family",
    "font-size",
    "font-style",
    "font-weight",
    "line-height",
    "margin",
    "margin-bottom",
    "margin-left",
    "margin-right",
    "margin-top",
    "padding-left",
    "text-align",
    "text-decoration"
  ]);
  return input
    .split(";")
    .map((rule) => rule.trim())
    .filter((rule) => {
      const [property, ...rest] = rule.split(":");
      const value = rest.join(":").trim();
      return allowed.has(property.trim().toLowerCase()) && value.length > 0 && !/url\s*\(|expression\s*\(/i.test(value);
    })
    .join("; ");
}

const RTF_DESTINATIONS = new Set([
  "colortbl",
  "datastore",
  "fonttbl",
  "info",
  "object",
  "pict",
  "stylesheet"
]);

function extractCompoundOfficeText(bytes: Uint8Array): string[] {
  const streams = readCompoundFileStreams(bytes);
  if (!streams) return [];
  const wordDocument = streams.get("worddocument");
  if (wordDocument) {
    const wordLines = extractWordDocumentStreamText(wordDocument);
    if (wordLines.length > 0) return wordLines;
  }
  const candidates = ["powerpoint document", "workbook", "book"]
    .map((name) => streams.get(name))
    .filter((stream): stream is Uint8Array => stream != null);
  return normalizePreviewLines(
    candidates.flatMap((stream) => [
      ...extractUtf16Runs(stream, false),
      ...extractUtf16Runs(stream, true),
      ...extractAsciiRuns(stream)
    ]),
    160
  );
}

function extractWordDocumentStreamText(stream: Uint8Array): string[] {
  if (stream.length < 0x20) return [];
  const fcMin = readU32(stream, 0x18);
  const fcMac = readU32(stream, 0x1c);
  const textBytes = fcMac > fcMin && fcMac <= stream.length ? stream.slice(fcMin, fcMac) : stream;
  return normalizePreviewLines(
    [...extractUtf16Runs(textBytes, false), ...extractAsciiRuns(textBytes)],
    160
  );
}

type CompoundDirectoryEntry = {
  name: string;
  objectType: number;
  startSector: number;
  streamSize: number;
};

const CFB_SIGNATURE = [0xd0, 0xcf, 0x11, 0xe0, 0xa1, 0xb1, 0x1a, 0xe1];
const CFB_FREE_SECTOR = 0xffffffff;
const CFB_END_OF_CHAIN = 0xfffffffe;
const CFB_MAX_REGULAR_SECTOR = 0xfffffffa;

function readCompoundFileStreams(bytes: Uint8Array): Map<string, Uint8Array> | null {
  if (!CFB_SIGNATURE.every((byte, index) => bytes[index] === byte)) return null;
  const sectorSize = 1 << readU16(bytes, 30);
  const miniSectorSize = 1 << readU16(bytes, 32);
  if (![512, 4096].includes(sectorSize) || miniSectorSize !== 64) return null;

  const fatSectorCount = readU32(bytes, 44);
  const firstDirectorySector = readU32(bytes, 48);
  const miniStreamCutoff = readU32(bytes, 56);
  const firstMiniFatSector = readU32(bytes, 60);
  const miniFatSectorCount = readU32(bytes, 64);
  const fatSectorIds = readDifatSectorIds(bytes, sectorSize, fatSectorCount);
  const fatEntries = readFatEntries(bytes, sectorSize, fatSectorIds);
  const directoryBytes = readRegularSectorChain(bytes, sectorSize, fatEntries, firstDirectorySector);
  if (!directoryBytes) return null;

  const entries = readCompoundDirectoryEntries(directoryBytes);
  const root = entries.find((entry) => entry.objectType === 5) ?? null;
  const rootMiniStream =
    root && isRegularSector(root.startSector)
      ? readRegularSectorChain(bytes, sectorSize, fatEntries, root.startSector)?.slice(0, root.streamSize)
      : null;
  const miniFatBytes =
    isRegularSector(firstMiniFatSector) && miniFatSectorCount > 0
      ? readRegularSectorChain(bytes, sectorSize, fatEntries, firstMiniFatSector)
      : null;
  const miniFatEntries = miniFatBytes ? readSectorIds(miniFatBytes, miniFatBytes.length / 4) : [];
  const streams = new Map<string, Uint8Array>();

  for (const entry of entries) {
    if (entry.objectType !== 2 || !entry.name || entry.streamSize <= 0) continue;
    let content: Uint8Array | null = null;
    if (entry.streamSize < miniStreamCutoff && rootMiniStream && miniFatEntries.length > 0) {
      content = readMiniSectorChain(rootMiniStream, miniSectorSize, miniFatEntries, entry.startSector);
    } else if (isRegularSector(entry.startSector)) {
      content = readRegularSectorChain(bytes, sectorSize, fatEntries, entry.startSector);
    }
    if (content) streams.set(entry.name.toLowerCase(), content.slice(0, entry.streamSize));
  }

  return streams;
}

function readDifatSectorIds(bytes: Uint8Array, sectorSize: number, fatSectorCount: number): number[] {
  const sectorIds: number[] = [];
  for (let offset = 76; offset + 3 < 512 && sectorIds.length < fatSectorCount; offset += 4) {
    const sectorId = readU32(bytes, offset);
    if (isRegularSector(sectorId)) sectorIds.push(sectorId);
  }
  let nextDifatSector = readU32(bytes, 68);
  const difatSectorCount = readU32(bytes, 72);
  for (
    let sectorIndex = 0;
    sectorIndex < difatSectorCount && isRegularSector(nextDifatSector) && sectorIds.length < fatSectorCount;
    sectorIndex += 1
  ) {
    const sector = regularSectorBytes(bytes, sectorSize, nextDifatSector);
    if (!sector) break;
    for (let offset = 0; offset + 7 < sector.length && sectorIds.length < fatSectorCount; offset += 4) {
      const sectorId = readU32(sector, offset);
      if (isRegularSector(sectorId)) sectorIds.push(sectorId);
    }
    nextDifatSector = readU32(sector, sector.length - 4);
  }
  return sectorIds;
}

function readFatEntries(bytes: Uint8Array, sectorSize: number, fatSectorIds: number[]): number[] {
  const entries: number[] = [];
  for (const sectorId of fatSectorIds) {
    const sector = regularSectorBytes(bytes, sectorSize, sectorId);
    if (!sector) continue;
    entries.push(...readSectorIds(sector, sector.length / 4));
  }
  return entries;
}

function readSectorIds(bytes: Uint8Array, count: number): number[] {
  const entries: number[] = [];
  for (let index = 0; index < count; index += 1) {
    entries.push(readU32(bytes, index * 4));
  }
  return entries;
}

function readRegularSectorChain(
  bytes: Uint8Array,
  sectorSize: number,
  fatEntries: number[],
  startSector: number
): Uint8Array | null {
  const chunks: Uint8Array[] = [];
  let sectorId = startSector;
  const seen = new Set<number>();
  while (isRegularSector(sectorId) && !seen.has(sectorId) && seen.size <= fatEntries.length) {
    seen.add(sectorId);
    const sector = regularSectorBytes(bytes, sectorSize, sectorId);
    if (!sector) return null;
    chunks.push(sector);
    sectorId = fatEntries[sectorId] ?? CFB_END_OF_CHAIN;
  }
  return concatBytes(chunks);
}

function readMiniSectorChain(
  rootMiniStream: Uint8Array,
  miniSectorSize: number,
  miniFatEntries: number[],
  startSector: number
): Uint8Array | null {
  const chunks: Uint8Array[] = [];
  let sectorId = startSector;
  const seen = new Set<number>();
  while (isRegularSector(sectorId) && !seen.has(sectorId) && seen.size <= miniFatEntries.length) {
    seen.add(sectorId);
    const offset = sectorId * miniSectorSize;
    if (offset + miniSectorSize > rootMiniStream.length) return null;
    chunks.push(rootMiniStream.slice(offset, offset + miniSectorSize));
    sectorId = miniFatEntries[sectorId] ?? CFB_END_OF_CHAIN;
  }
  return concatBytes(chunks);
}

function regularSectorBytes(bytes: Uint8Array, sectorSize: number, sectorId: number): Uint8Array | null {
  const offset = (sectorId + 1) * sectorSize;
  if (offset + sectorSize > bytes.length) return null;
  return bytes.slice(offset, offset + sectorSize);
}

function readCompoundDirectoryEntries(directoryBytes: Uint8Array): CompoundDirectoryEntry[] {
  const entries: CompoundDirectoryEntry[] = [];
  for (let offset = 0; offset + 127 < directoryBytes.length; offset += 128) {
    const entry = directoryBytes.slice(offset, offset + 128);
    const nameLength = readU16(entry, 64);
    const nameBytes = nameLength >= 2 ? entry.slice(0, Math.min(nameLength - 2, 64)) : new Uint8Array();
    const streamSizeHigh = readU32(entry, 124);
    entries.push({
      name: decodeUtf16Bytes(nameBytes, false),
      objectType: entry[66],
      startSector: readU32(entry, 116),
      streamSize: streamSizeHigh === 0 ? readU32(entry, 120) : 0
    });
  }
  return entries;
}

function isRegularSector(sectorId: number): boolean {
  return sectorId < CFB_MAX_REGULAR_SECTOR && sectorId !== CFB_FREE_SECTOR && sectorId !== CFB_END_OF_CHAIN;
}

function concatBytes(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((sum, chunk) => sum + chunk.length, 0);
  const output = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.length;
  }
  return output;
}

function extractAsciiRuns(bytes: Uint8Array): string[] {
  const runs: string[] = [];
  let value = "";
  const flush = () => {
    const normalized = value.replace(/\s+/g, " ").trim();
    if (normalized.length >= 4) runs.push(normalized);
    value = "";
  };
  for (const byte of bytes) {
    if (byte === 0x09 || byte === 0x0a || byte === 0x0d || (byte >= 0x20 && byte <= 0x7e)) {
      value += String.fromCharCode(byte);
    } else {
      flush();
    }
  }
  flush();
  return runs;
}

function extractUtf16Runs(bytes: Uint8Array, bigEndian: boolean): string[] {
  const runs: string[] = [];
  for (let offset = 0; offset < 2; offset += 1) {
    let value = "";
    const flush = () => {
      const normalized = value.replace(/\s+/g, " ").trim();
      if (normalized.length >= 4) runs.push(normalized);
      value = "";
    };
    for (let index = offset; index + 1 < bytes.length; index += 2) {
      const code = bigEndian
        ? (bytes[index] << 8) | bytes[index + 1]
        : bytes[index] | (bytes[index + 1] << 8);
      if (isReadableUtf16Code(code)) {
        value += String.fromCharCode(code);
      } else {
        flush();
      }
    }
    flush();
  }
  return runs;
}

function decodeUtf16Bytes(bytes: Uint8Array, bigEndian: boolean): string {
  let value = "";
  for (let index = 0; index + 1 < bytes.length; index += 2) {
    const code = bigEndian
      ? (bytes[index] << 8) | bytes[index + 1]
      : bytes[index] | (bytes[index + 1] << 8);
    if (isReadableUtf16Code(code)) value += String.fromCharCode(code);
  }
  return value;
}

function isReadableUtf16Code(code: number): boolean {
  return code === 0x09 || code === 0x0a || code === 0x0d || (code >= 0x20 && code < 0xd800);
}

function normalizePreviewLines(values: string[], limit: number): string[] {
  const seen = new Set<string>();
  const lines: string[] = [];
  for (const value of values) {
    const normalized = value.replace(/\0/g, "").replace(/\s+/g, " ").trim();
    if (!normalized || seen.has(normalized)) continue;
    seen.add(normalized);
    lines.push(normalized.slice(0, 600));
    if (lines.length >= limit) break;
  }
  return lines;
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
  const output = inflateSync(input);
  if (expectedSize > 0 && output.length !== expectedSize) {
    throw new Error("Office preview decompressed to an unexpected size");
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

function bytesToBinaryString(bytes: Uint8Array): string {
  const chunks: string[] = [];
  for (let offset = 0; offset < bytes.length; offset += 0x8000) {
    chunks.push(String.fromCharCode(...bytes.slice(offset, offset + 0x8000)));
  }
  return chunks.join("");
}

function utf8StringToBytes(value: string): Uint8Array {
  return utf8Encoder.encode(value);
}

function bytesToBase64(bytes: Uint8Array): string {
  return btoa(bytesToBinaryString(bytes));
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
