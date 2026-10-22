import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fetchGatewayToken } from "../src/api.js";

let originalFetch;

beforeEach(() => {
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("fetchGatewayToken", () => {
  // A dial that dies on its token said `status: null` whether the api refused
  // the session or the network was not there at all.
  it("names the HTTP refusal without reading the response", async () => {
    const json = vi.fn();
    globalThis.fetch = vi.fn(async () => ({ ok: false, status: 401, json }));
    await expect(fetchGatewayToken()).rejects.toMatchObject({ message: "could not mint a gateway token", status: 401 });
    expect(json).not.toHaveBeenCalled();
  });

  it("returns the minted token", async () => {
    globalThis.fetch = vi.fn(async () => ({ ok: true, json: async () => ({ token: "tok-1" }) }));
    await expect(fetchGatewayToken()).resolves.toBe("tok-1");
  });
});
