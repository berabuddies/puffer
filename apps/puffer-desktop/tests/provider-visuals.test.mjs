import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const testDir = dirname(fileURLToPath(import.meta.url));
const appRoot = resolve(testDir, "..");
const repoRoot = resolve(appRoot, "../..");
const providerVisualsPath = resolve(appRoot, "src/lib/providerVisuals.ts");
const providerDir = resolve(repoRoot, "resources/providers");
const providerVisuals = readFileSync(providerVisualsPath, "utf8");

const nativeAliases = ["puffer", "codex", "claude"];
const resourceProviderIds = readdirSync(providerDir)
  .filter((fileName) => fileName.endsWith(".yaml"))
  .map((fileName) => fileName.replace(/\.yaml$/, ""))
  .sort();

if (resourceProviderIds.length === 0) {
  throw new Error(`No provider resources found in ${providerDir}`);
}

function parseRecord(name) {
  const pattern = new RegExp(`const ${name}: Record<string, string> = \\{([\\s\\S]*?)\\n\\};`);
  const match = providerVisuals.match(pattern);
  if (!match) throw new Error(`Missing ${name} record`);

  const entries = {};
  for (const line of match[1].split("\n")) {
    const trimmed = line.trim().replace(/,$/, "");
    if (!trimmed) continue;
    const entry = trimmed.match(/^(?:"([^"]+)"|([a-zA-Z0-9_-]+)):\s*"([^"]+)"$/);
    if (entry) entries[entry[1] ?? entry[2]] = entry[3];
  }
  return entries;
}

const icons = parseRecord("PROVIDER_ICONS");
const accents = parseRecord("PROVIDER_ACCENTS");
const ids = [...resourceProviderIds, ...nativeAliases];

for (const providerId of ids) {
  if (!Object.hasOwn(icons, providerId)) {
    throw new Error(`Missing explicit provider icon mapping for ${providerId}`);
  }
  if (!Object.hasOwn(accents, providerId)) {
    throw new Error(`Missing explicit provider accent mapping for ${providerId}`);
  }

  const icon = icons[providerId];
  if (icon === "ai" || icon === "llm") {
    throw new Error(`Known provider ${providerId} must not use generic ${icon}.svg`);
  }

  const assetPath = icon === "brand-logo"
    ? resolve(appRoot, "public/brand-logo.svg")
    : resolve(appRoot, `public/service-icons/${icon}.svg`);
  if (!existsSync(assetPath)) {
    throw new Error(`Mapped icon asset for ${providerId} does not exist: ${assetPath}`);
  }
}

console.log(`Verified provider visuals for ${ids.length} known provider ids.`);
