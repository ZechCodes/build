import { describe, expect, it } from "vitest";
import { canonicalBridgeOrigin } from "../src/core/httpEndpoint.js";

describe("bridge endpoint authority", () => {
  it.each([
    [" https://BRIDGE.example:443/ ", "https://bridge.example"],
    ["https://bücher.example:9443", "https://xn--bcher-kva.example:9443"],
    ["https://[2001:db8::1]:9443/", "https://[2001:db8::1]:9443"],
    ["https://192.168.1.1", "https://192.168.1.1"],
  ])("canonicalizes %s without network access", (input, origin) => {
    expect(canonicalBridgeOrigin(input)).toBe(origin);
  });

  it.each([
    "http://localhost", "https://user:pass@bridge.example", "https://bridge.example/path",
    "https://bridge.example/?secret=x", "https://bridge.example/#x", "https://bridge.example.",
    "https://127.1", "https://0x7f000001", "https://0177.0.0.1", "https://2130706433",
    "https://[fe80::1%25eth0]", "https://bridge.example\\evil", "https://bridge.example/../",
    "https://bridge.example?", "https://bridge.example#", "https://bridge.example:0",
    "https://bridge.exa\nmple", "https://%62ridge.example", "https://bridge.example//",
    "https:///bridge.example", "https://bridge.example:65536", "x".repeat(2049),
  ])("rejects ambiguous or non-origin address %s", (input) => {
    expect(() => canonicalBridgeOrigin(input)).toThrow();
  });
});
