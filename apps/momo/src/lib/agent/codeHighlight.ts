export type HighlightKind =
  | "plain"
  | "keyword"
  | "string"
  | "number"
  | "comment"
  | "property"
  | "operator"
  | "punctuation"
  | "function"
  | "type"
  | "tag"
  | "attribute";

export type HighlightToken = {
  kind: HighlightKind;
  text: string;
};

export type Language =
  | "css"
  | "html"
  | "javascript"
  | "json"
  | "markdown"
  | "rust"
  | "shell"
  | "toml"
  | "typescript"
  | "xml"
  | "yaml"
  | "plain";

const languageByExt = new Map<string, Language>([
  ["cjs", "javascript"],
  ["css", "css"],
  ["html", "html"],
  ["htm", "html"],
  ["js", "javascript"],
  ["json", "json"],
  ["jsonc", "json"],
  ["jsx", "javascript"],
  ["lock", "json"],
  ["md", "markdown"],
  ["mjs", "javascript"],
  ["rs", "rust"],
  ["sh", "shell"],
  ["svelte", "html"],
  ["toml", "toml"],
  ["ts", "typescript"],
  ["tsx", "typescript"],
  ["xml", "xml"],
  ["yaml", "yaml"],
  ["yml", "yaml"]
]);

const rustKeywords = new Set([
  "as",
  "async",
  "await",
  "break",
  "const",
  "continue",
  "crate",
  "dyn",
  "else",
  "enum",
  "false",
  "fn",
  "for",
  "if",
  "impl",
  "in",
  "let",
  "loop",
  "match",
  "mod",
  "move",
  "mut",
  "pub",
  "ref",
  "return",
  "self",
  "Self",
  "static",
  "struct",
  "super",
  "trait",
  "true",
  "type",
  "unsafe",
  "use",
  "where",
  "while"
]);

const jsKeywords = new Set([
  "async",
  "await",
  "break",
  "case",
  "catch",
  "class",
  "const",
  "continue",
  "default",
  "delete",
  "do",
  "else",
  "export",
  "extends",
  "false",
  "finally",
  "for",
  "from",
  "function",
  "if",
  "import",
  "in",
  "instanceof",
  "interface",
  "let",
  "new",
  "null",
  "of",
  "return",
  "satisfies",
  "switch",
  "throw",
  "true",
  "try",
  "type",
  "typeof",
  "undefined",
  "while"
]);

const shellKeywords = new Set([
  "case",
  "do",
  "done",
  "elif",
  "else",
  "esac",
  "fi",
  "for",
  "function",
  "if",
  "in",
  "then",
  "while"
]);

const typeWords = new Set([
  "Array",
  "Boolean",
  "Error",
  "Map",
  "Number",
  "Option",
  "Promise",
  "Record",
  "Result",
  "Set",
  "String",
  "Vec",
  "bool",
  "f32",
  "f64",
  "i32",
  "i64",
  "isize",
  "number",
  "str",
  "string",
  "u32",
  "u64",
  "usize",
  "void"
]);

/** Returns the display language guessed from a file path. */
export function languageFromPath(path: string | null | undefined): Language {
  if (!path) return "plain";
  const clean = path.split("?")[0]?.split("#")[0] ?? path;
  const base = clean.slice(clean.lastIndexOf("/") + 1).toLowerCase();
  if (base === "cargo.lock") return "toml";
  if (base === "makefile" || base === "dockerfile") return "shell";
  const dot = base.lastIndexOf(".");
  if (dot === -1) return "plain";
  return languageByExt.get(base.slice(dot + 1)) ?? "plain";
}

/** Tokenizes one line of source code for lightweight syntax highlighting. */
export function highlightCodeLine(line: string, path?: string | null): HighlightToken[] {
  const text = line.length > 0 ? line : " ";
  const language = languageFromPath(path);
  if (language === "html" || language === "xml") return highlightMarkup(text);
  if (language === "markdown") return highlightMarkdown(text);
  return highlightGeneric(text, language);
}

function push(tokens: HighlightToken[], kind: HighlightKind, text: string) {
  if (!text) return;
  const prev = tokens[tokens.length - 1];
  if (prev?.kind === kind) {
    prev.text += text;
  } else {
    tokens.push({ kind, text });
  }
}

function isIdentStart(ch: string): boolean {
  return /[A-Za-z_$]/.test(ch);
}

function isIdent(ch: string): boolean {
  return /[A-Za-z0-9_$-]/.test(ch);
}

function isNumberStart(ch: string, next: string): boolean {
  return /[0-9]/.test(ch) || (ch === "." && /[0-9]/.test(next));
}

function scanString(text: string, start: number, quote: string): number {
  let i = start + 1;
  while (i < text.length) {
    if (text[i] === "\\") {
      i += 2;
      continue;
    }
    if (text[i] === quote) return i + 1;
    i += 1;
  }
  return text.length;
}

function scanNumber(text: string, start: number): number {
  let i = start;
  while (i < text.length && /[A-Fa-f0-9_xXoObB.]/.test(text[i])) i += 1;
  return i;
}

function nextNonSpace(text: string, start: number): string {
  for (let i = start; i < text.length; i += 1) {
    if (!/\s/.test(text[i])) return text[i];
  }
  return "";
}

function previousNonSpace(text: string, start: number): string {
  for (let i = start; i >= 0; i -= 1) {
    if (!/\s/.test(text[i])) return text[i];
  }
  return "";
}

function classifyIdentifier(word: string, text: string, start: number, end: number, language: Language): HighlightKind {
  if ((language === "rust" && rustKeywords.has(word)) || (isJavaScript(language) && jsKeywords.has(word))) {
    return "keyword";
  }
  if (language === "shell" && shellKeywords.has(word)) return "keyword";
  if (typeWords.has(word) || /^[A-Z][A-Za-z0-9_]*$/.test(word)) return "type";
  if (nextNonSpace(text, end) === "(") return "function";
  if (previousNonSpace(text, start - 1) === ".") return "property";
  return "plain";
}

function isJavaScript(language: Language): boolean {
  return language === "javascript" || language === "typescript";
}

function usesHashComments(language: Language): boolean {
  return language === "shell" || language === "toml" || language === "yaml";
}

function highlightGeneric(text: string, language: Language): HighlightToken[] {
  const tokens: HighlightToken[] = [];
  let i = 0;

  while (i < text.length) {
    const ch = text[i];
    const next = text[i + 1] ?? "";

    if (ch === "/" && next === "/") {
      push(tokens, "comment", text.slice(i));
      break;
    }
    if (ch === "/" && next === "*") {
      const end = text.indexOf("*/", i + 2);
      const j = end === -1 ? text.length : end + 2;
      push(tokens, "comment", text.slice(i, j));
      i = j;
      continue;
    }
    if (usesHashComments(language) && ch === "#") {
      push(tokens, "comment", text.slice(i));
      break;
    }
    if (ch === "\"" || ch === "'" || ch === "`") {
      const j = scanString(text, i, ch);
      const prev = previousNonSpace(text, i - 1);
      const nextChar = nextNonSpace(text, j);
      push(tokens, prev === "" && nextChar === ":" ? "property" : "string", text.slice(i, j));
      i = j;
      continue;
    }
    if (language === "yaml" && isIdentStart(ch)) {
      const j = scanIdentifier(text, i);
      const kind = nextNonSpace(text, j) === ":" ? "property" : classifyIdentifier(text.slice(i, j), text, i, j, language);
      push(tokens, kind, text.slice(i, j));
      i = j;
      continue;
    }
    if (language === "css" && isIdentStart(ch)) {
      const j = scanIdentifier(text, i);
      const word = text.slice(i, j);
      push(tokens, nextNonSpace(text, j) === ":" ? "property" : classifyIdentifier(word, text, i, j, language), word);
      i = j;
      continue;
    }
    if (isNumberStart(ch, next)) {
      const j = scanNumber(text, i);
      push(tokens, "number", text.slice(i, j));
      i = j;
      continue;
    }
    if (isIdentStart(ch)) {
      const j = scanIdentifier(text, i);
      const word = text.slice(i, j);
      let kind = classifyIdentifier(word, text, i, j, language);
      if ((language === "json" || language === "toml") && nextNonSpace(text, j) === ":") kind = "property";
      push(tokens, kind, word);
      i = j;
      continue;
    }
    if (/[{}()[\],.;:]/.test(ch)) {
      push(tokens, "punctuation", ch);
    } else if (/[+\-*/%=!<>&|?:]/.test(ch)) {
      push(tokens, "operator", ch);
    } else {
      push(tokens, "plain", ch);
    }
    i += 1;
  }

  return tokens.length > 0 ? tokens : [{ kind: "plain", text: " " }];
}

function scanIdentifier(text: string, start: number): number {
  let i = start + 1;
  while (i < text.length && isIdent(text[i])) i += 1;
  return i;
}

function highlightMarkdown(text: string): HighlightToken[] {
  const trimmed = text.trimStart();
  const leading = text.length - trimmed.length;
  const tokens: HighlightToken[] = [];
  if (trimmed.startsWith("#")) {
    const markerEnd = trimmed.search(/[^#]/);
    const end = markerEnd === -1 ? text.length : leading + markerEnd;
    push(tokens, "keyword", text.slice(0, end));
    push(tokens, "plain", text.slice(end));
    return tokens;
  }
  if (trimmed.startsWith("```")) return [{ kind: "comment", text }];
  if (/^\s*[-*+]\s/.test(text)) {
    const bullet = text.match(/^\s*[-*+]\s/)?.[0] ?? "";
    push(tokens, "operator", bullet);
    push(tokens, "plain", text.slice(bullet.length));
    return tokens;
  }
  return highlightGeneric(text, "plain");
}

function highlightMarkup(text: string): HighlightToken[] {
  const trimmed = text.trim();
  if (!text.includes("<") && looksLikeCssLine(trimmed)) return highlightGeneric(text, "css");
  if (!text.includes("<") && looksLikeScriptLine(trimmed)) return highlightGeneric(text, "typescript");

  const tokens: HighlightToken[] = [];
  let i = 0;

  while (i < text.length) {
    if (text.startsWith("<!--", i)) {
      const end = text.indexOf("-->", i + 4);
      const j = end === -1 ? text.length : end + 3;
      push(tokens, "comment", text.slice(i, j));
      i = j;
      continue;
    }
    if (text[i] !== "<") {
      const nextTag = text.indexOf("<", i);
      const j = nextTag === -1 ? text.length : nextTag;
      push(tokens, "plain", text.slice(i, j));
      i = j;
      continue;
    }

    push(tokens, "punctuation", "<");
    i += 1;
    if (text[i] === "/") {
      push(tokens, "punctuation", "/");
      i += 1;
    }
    const tagStart = i;
    while (i < text.length && /[A-Za-z0-9:_-]/.test(text[i])) i += 1;
    push(tokens, "tag", text.slice(tagStart, i));

    while (i < text.length) {
      const ch = text[i];
      if (ch === ">") {
        push(tokens, "punctuation", ">");
        i += 1;
        break;
      }
      if (ch === "/" && text[i + 1] === ">") {
        push(tokens, "punctuation", "/>");
        i += 2;
        break;
      }
      if (ch === "\"" || ch === "'") {
        const j = scanString(text, i, ch);
        push(tokens, "string", text.slice(i, j));
        i = j;
        continue;
      }
      if (/[A-Za-z_:]/.test(ch)) {
        const attrStart = i;
        while (i < text.length && /[A-Za-z0-9:_-]/.test(text[i])) i += 1;
        push(tokens, "attribute", text.slice(attrStart, i));
        continue;
      }
      if (/[={}()[\],.;]/.test(ch)) {
        push(tokens, "punctuation", ch);
      } else if (/[+\-*%=!<>&|?:]/.test(ch)) {
        push(tokens, "operator", ch);
      } else {
        push(tokens, "plain", ch);
      }
      i += 1;
    }
  }

  return tokens.length > 0 ? tokens : [{ kind: "plain", text: " " }];
}

function looksLikeCssLine(trimmed: string): boolean {
  return /^[.#]?[A-Za-z_-][A-Za-z0-9_-]*\s*\{/.test(trimmed) ||
    /^[A-Za-z-]+\s*:/.test(trimmed) ||
    trimmed === "}";
}

function looksLikeScriptLine(trimmed: string): boolean {
  return /^(const|let|var|function|if|for|while|return|import|export|type|interface)\b/.test(trimmed);
}
