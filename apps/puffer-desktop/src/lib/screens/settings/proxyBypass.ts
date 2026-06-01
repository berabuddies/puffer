export function normalizeProxyBypass(value: string): string[] {
  return value
    .split(/\r?\n|,/)
    .map((entry) => entry.trim())
    .filter(Boolean);
}

export function validateProxyBypassEntries(entries: string[]): string | null {
  const invalid = entries.find((entry) => !validProxyBypassEntry(entry));
  return invalid ? `Invalid bypass entry: ${invalid}` : null;
}

function validProxyBypassEntry(entry: string): boolean {
  if (!entry) return true;
  if (entry.includes("*") || entry.includes("\\") || entry.split("/").length > 2) {
    return false;
  }
  if (entry.includes("/")) {
    return validCidr(entry);
  }
  if (entry.includes(":")) {
    return validIpv6Address(entry);
  }
  if (entry.includes(" ")) {
    return false;
  }
  return true;
}

function validCidr(entry: string): boolean {
  const [address, prefix] = entry.split("/");
  if (!address || !/^\d+$/.test(prefix)) return false;
  const prefixValue = Number(prefix);
  if (validIpv4Address(address)) {
    return prefixValue >= 0 && prefixValue <= 32;
  }
  if (validIpv6Address(address)) {
    return prefixValue >= 0 && prefixValue <= 128;
  }
  return false;
}

function validIpv4Address(value: string): boolean {
  if (!/^\d{1,3}(?:\.\d{1,3}){3}$/.test(value)) return false;
  return value.split(".").every((part) => {
    const parsed = Number(part);
    return Number.isInteger(parsed) && parsed >= 0 && parsed <= 255;
  });
}

function validIpv6Address(value: string): boolean {
  return /^[0-9a-fA-F:]+$/.test(value) && value.includes(":");
}
