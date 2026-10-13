import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fetchIceServers } from "../src/api.js";

const SERVERS = [
  {
    urls: ["turn:turn.cloudflare.com:3478?transport=udp"],
    username: "minted-username",
    credential: "minted-credential",
  },
];

let originalFetch;

beforeEach(() => {
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("fetchIceServers", () => {
  it("names the HTTP refusal without retaining the response or credentials", async () => {
    const json = vi.fn();
    globalThis.fetch = vi.fn(async () => ({ ok: false, status: 401, json }));
    await expect(fetchIceServers()).rejects.toMatchObject({ message: "could not mint ICE servers", status: 401 });
    expect(json).not.toHaveBeenCalled();
  });

  it("posts to the authenticated ice-servers route and returns the array", async () => {
    const fetchSpy = vi.fn(async () => ({
      ok: true,
      json: async () => ({ iceServers: SERVERS }),
    }));
    globalThis.fetch = fetchSpy;
    await expect(fetchIceServers()).resolves.toEqual(SERVERS);
    const [url, request] = fetchSpy.mock.calls[0];
    expect(url).toBe("/api/rtc/ice-servers");
    expect(request.method).toBe("POST");
  });

  it("throws when the api refuses, so negotiation cannot proceed", async () => {
    globalThis.fetch = vi.fn(async () => ({ ok: false, status: 502 }));
    await expect(fetchIceServers()).rejects.toThrow();
  });
});
