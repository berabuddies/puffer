import { expect, test } from "@playwright/test";
import {
  normalizeProxyBypass,
  validateProxyBypassEntries
} from "../src/lib/screens/settings/proxyBypass";

test("proxy bypass entries normalize line and comma separated values", () => {
  expect(normalizeProxyBypass("localhost, 127.0.0.1\n\n10.0.0.0/8")).toEqual([
    "localhost",
    "127.0.0.1",
    "10.0.0.0/8"
  ]);
});

test("proxy bypass validation accepts hosts, IPs, and CIDR ranges", () => {
  expect(validateProxyBypassEntries([
    "localhost",
    "api.example.com",
    "127.0.0.1",
    "::1",
    "10.0.0.0/8",
    "fd00::/8"
  ])).toBeNull();
});

test("proxy bypass validation rejects unsupported patterns", () => {
  expect(validateProxyBypassEntries(["*.example.com"])).toBe(
    "Invalid bypass entry: *.example.com"
  );
  expect(validateProxyBypassEntries(["10.0.0.0/99"])).toBe(
    "Invalid bypass entry: 10.0.0.0/99"
  );
  expect(validateProxyBypassEntries(["bad\\host"])).toBe(
    "Invalid bypass entry: bad\\host"
  );
});
