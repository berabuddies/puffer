import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const css = readFileSync(resolve(root, "../src/app.css"), "utf8");
const sidebarStart = css.indexOf(".pf-sidebar {");
const sidebarEnd = css.indexOf("/* ==========================================================================\n   Responsive", sidebarStart);

if (sidebarStart === -1 || sidebarEnd === -1) {
  throw new Error("Could not locate sidebar CSS section in app.css");
}

const sidebarCss = css.slice(sidebarStart, sidebarEnd);

function escapeRegex(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function ruleFor(selector) {
  const pattern = new RegExp(`(^|})\\s*([^{}]+)\\{([^}]*)\\}`, "gm");
  let match;
  while ((match = pattern.exec(sidebarCss)) !== null) {
    const selectors = match[2].split(",").map((value) => value.trim());
    if (selectors.includes(selector)) return match[3];
  }
  if (!match) throw new Error(`Missing sidebar CSS rule for ${selector}`);
}

function assertRuleContains(selector, expected) {
  const rule = ruleFor(selector);
  if (!rule.includes(expected)) {
    throw new Error(`Expected ${selector} to include ${expected}`);
  }
}

for (const legacyAccent of ["#f2eaff", "#dcccf8", "#8b4de8"]) {
  if (sidebarCss.includes(legacyAccent)) {
    throw new Error(`Sidebar CSS still contains fixed accent ${legacyAccent}`);
  }
}

assertRuleContains(".pf-sidebar", "--pf-sidebar-accent: var(--puffer-accent)");
assertRuleContains('.pf-sidebar-item[data-active="true"]', "var(--pf-sidebar-accent-bg)");
assertRuleContains('.pf-sidebar-item[data-active="true"] svg', "var(--pf-sidebar-accent)");
assertRuleContains('.pf-sidebar-agent-row[data-active="true"]', "var(--pf-sidebar-accent-bg)");
assertRuleContains('.pf-sidebar-agent-row .pf-pin-button[data-pinned="true"]', "var(--pf-sidebar-accent)");
assertRuleContains(".pf-sidebar-avatar", "var(--pf-sidebar-accent)");
