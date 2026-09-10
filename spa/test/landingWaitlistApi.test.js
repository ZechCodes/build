import { describe, expect, it, beforeEach, afterEach, vi } from "vitest";
import {
  WAITLIST_ENDPOINT,
  submitWaitlistEmail,
} from "../../skriftapp/buildapp/landing/waitlist-api.js";

const EMAIL = "someone@example.com";

let originalFetch;

beforeEach(() => {
  originalFetch = globalThis.fetch;
});

afterEach(() => {
  globalThis.fetch = originalFetch;
});

describe("waitlist api", () => {
  it("posts the email as JSON to the waitlist endpoint", async () => {
    const fetchSpy = vi.fn(async () => ({ ok: true }));
    globalThis.fetch = fetchSpy;
    await submitWaitlistEmail(EMAIL);
    expect(fetchSpy).toHaveBeenCalledTimes(1);
    const [url, request] = fetchSpy.mock.calls[0];
    expect(url).toBe(WAITLIST_ENDPOINT);
    expect(WAITLIST_ENDPOINT).toBe("/api/waitlist");
    expect(request.method).toBe("POST");
    expect(request.headers["Content-Type"]).toBe("application/json");
    expect(JSON.parse(request.body)).toEqual({ email: EMAIL });
  });

  it("resolves true for an ok response and false for a non-ok response", async () => {
    globalThis.fetch = vi.fn(async () => ({ ok: true }));
    await expect(submitWaitlistEmail(EMAIL)).resolves.toBe(true);
    globalThis.fetch = vi.fn(async () => ({ ok: false }));
    await expect(submitWaitlistEmail(EMAIL)).resolves.toBe(false);
  });

  it("a rejected fetch propagates", async () => {
    globalThis.fetch = vi.fn(async () => {
      throw new Error("offline");
    });
    await expect(submitWaitlistEmail(EMAIL)).rejects.toThrow("offline");
  });
});
