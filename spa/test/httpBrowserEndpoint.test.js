import { afterEach, describe, expect, it, vi } from "vitest";
import { createBrowserHttpEndpoint } from "../src/core/httpBrowserEndpoint.js";

const origin = "https://bridge.example";
const opening = { session_id: "session", opening_generation: "generation", grant_expires_at: new Date(Date.now() + 30000).toISOString(), greeting: {} };
const json = (value, status = 200) => new Response(JSON.stringify(value), { status, headers: { "content-type": "application/json" } });
const grant = () => ({ identity: { origin, deviceId: "device" }, token: () => "device-token", expiresAt: () => Date.now() + 300000, renew: vi.fn(), close: vi.fn() });
afterEach(() => vi.useRealTimers());

describe("browser HTTP endpoint", () => {
  it("sends only the device bearer with fixed redirect, cookie and metadata policy", async () => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(opening)).mockResolvedValueOnce(json({ ok: true, result: {} })).mockResolvedValue(new Response(null, { status: 204 }));
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    await endpoint.open(">=3.24.0 <4.0.0");
    await endpoint.request(opening, { path: "/v1/tasks/t", method: "PATCH", body: { title: "a" } }, { subscriptionId: "stream", priority: "foreground", write: { operation_id: "op", after: ["previous"] } });
    const [url, options] = fetchImpl.mock.calls[1];
    expect(url).toBe(origin + "/v1/tasks/t");
    expect(options).toMatchObject({ credentials: "omit", redirect: "error", referrerPolicy: "no-referrer", method: "PATCH" });
    expect(new Headers(options.headers).get("authorization")).toBe("Bearer device-token");
    expect(new Headers(options.headers).get("X-Build-Subscription")).toBe("stream");
    expect(new Headers(options.headers).get("X-Build-Priority")).toBe("user");
    expect(new Headers(options.headers).get("Idempotency-Key")).toBe("op");
    expect(new Headers(options.headers).get("X-Build-After")).toBe('["previous"]');
    endpoint.close();
  });

  it("uses a fresh one-use application binding and waits for its hello", async () => {
    let controller;
    const stream = new ReadableStream({ start(c) { controller = c; } });
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(opening))
      .mockResolvedValueOnce(json({ subscription_id: "stream", events_path: "/v1/subscriptions/stream/events", opening_generation: "generation" }))
      .mockResolvedValueOnce(new Response(stream, { headers: { "content-type": "text/event-stream" } }))
      .mockResolvedValue(new Response(null, { status: 204 }));
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    await endpoint.open("range");
    let ready = false;
    const pending = endpoint.subscribe(opening, () => {}).then((value) => { ready = true; return value; });
    await vi.waitFor(() => expect(fetchImpl).toHaveBeenCalledTimes(3));
    expect(ready).toBe(false);
    controller.enqueue(new TextEncoder().encode(`event: hello\ndata: ${JSON.stringify({ subscription_id: "stream", opening_generation: "generation", durable: false })}\n\n`));
    expect((await pending).binding).toBe("stream");
    expect(JSON.parse(fetchImpl.mock.calls[1][1].body)).toEqual({ kind: "application" });
    endpoint.close();
  });

  it("refuses redirected event paths and body overflow without sending", async () => {
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(opening)).mockResolvedValueOnce(json({ subscription_id: "s", events_path: "https://evil.example", opening_generation: "generation" })).mockResolvedValue(new Response(null, { status: 204 }));
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    await endpoint.open("range");
    await expect(endpoint.subscribe(opening, () => {})).rejects.toThrow();
    await expect(endpoint.request(opening, { path: "/v1/data", method: "POST", body: { big: "x".repeat(200) }, requestMaxBytes: 20 }, {})).rejects.toMatchObject({ uncertain: false });
    expect(fetchImpl).toHaveBeenCalledTimes(2);
    endpoint.close();
  });

  it("hard-closes on a hanging revalidation without extending authority", async () => {
    vi.useFakeTimers();
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(opening)).mockImplementation(() => new Promise(() => {}));
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    const lost = vi.fn(); endpoint.onLost(lost);
    await endpoint.open("range");
    await vi.advanceTimersByTimeAsync(31000);
    expect(lost).toHaveBeenCalledTimes(1);
    await expect(endpoint.request(opening, { path: "/v1/health", method: "GET" }, {})).rejects.toThrow();
    endpoint.close();
  });

  it("honors an explicit disabled deadline through response-body completion", async () => {
    vi.useFakeTimers();
    let resolve;
    const fetchImpl = vi.fn().mockResolvedValueOnce(json(opening)).mockImplementationOnce(() => new Promise((done) => { resolve = done; })).mockResolvedValue(new Response(null, { status: 204 }));
    const endpoint = createBrowserHttpEndpoint({ grant: grant(), fetchImpl });
    const held = await endpoint.open("range");
    let settled = false;
    const pending = endpoint.request(held, { path: "/v1/slow", method: "GET" }, { timeoutMs: null, subscriptionId: "stream" }).then((value) => { settled = true; return value; });
    await vi.advanceTimersByTimeAsync(13000);
    expect(settled).toBe(false);
    resolve(json({ ok: true, result: {} }));
    await pending; endpoint.close();
  });
});
