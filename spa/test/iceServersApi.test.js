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
  it("posts to the session-authed ice-servers route and returns the array", async () => {
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

  it("throws when the api refuses, so the caller stays on the relay", async () => {
    globalThis.fetch = vi.fn(async () => ({ ok: false, status: 502 }));
    await expect(fetchIceServers()).rejects.toThrow();
  });
});
