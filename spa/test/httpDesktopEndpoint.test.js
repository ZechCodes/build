import { describe, expect, it, vi } from "vitest";
import { createDesktopHttpEndpoint } from "../src/core/httpDesktopEndpoint.js";

describe("typed desktop HTTP facade", () => {
  it("calls only operation IDs and cancels exact pending requests", async () => {
    let resolve;
    const api = { open: vi.fn().mockResolvedValue({ ok: true, value: { session_id: "s" } }), request: vi.fn(() => new Promise((done) => { resolve = done; })), cancel: vi.fn().mockResolvedValue({ ok: true }), close: vi.fn().mockResolvedValue({ ok: true }) };
    const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "h", deviceId: "d" } });
    await endpoint.open("range");
    const controller = new AbortController();
    const pending = endpoint.request({}, { operationId: "taskUpdate", path: "/ignored", method: "PATCH", body: { title: "x" }, params: { task_id: "t", title: "x" } }, { signal: controller.signal, requestId: "r", priority: "background" });
    controller.abort();
    resolve({ ok: true, value: { ok: true, result: {} } });
    await pending;
    expect(api.cancel).toHaveBeenCalledWith("r");
    expect(api.request).toHaveBeenCalledWith("h", "taskUpdate", { task_id: "t", title: "x" }, { requestId: "r", priority: "background" });
    endpoint.close();
  });

  it("ACKs events only after consumer application and ignores foreign streams", async () => {
    let take; let apply;
    const api = {
      subscribe: vi.fn().mockResolvedValue({ ok: true, value: { stream: "stream" } }),
      onEvent(fn) { take = fn; return vi.fn(); }, ack: vi.fn().mockResolvedValue({ ok: true }),
      unsubscribe: vi.fn().mockResolvedValue({ ok: true }), close: vi.fn().mockResolvedValue({ ok: true }),
    };
    const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "h", deviceId: "d" } });
    const pending = endpoint.subscribe({}, ({ event }) => event === "hello" ? undefined : new Promise((done) => { apply = done; }));
    await vi.waitFor(() => expect(api.subscribe).toHaveBeenCalled());
    take({ stream: "stream", sequence: 1, event: "hello", data: {} });
    await pending;
    take({ stream: "other", sequence: 1, event: "event", data: {} });
    take({ stream: "stream", sequence: 2, event: "event", data: {} });
    await vi.waitFor(() => expect(apply).toBeTypeOf("function"));
    expect(api.ack).toHaveBeenCalledTimes(1);
    apply();
    await vi.waitFor(() => expect(api.ack).toHaveBeenCalledWith("stream", 2));
    endpoint.close();
  });

  it("propagates safe uncertainty without leaking arbitrary response objects", async () => {
    const api = { request: vi.fn().mockResolvedValue({ ok: false, error: { code: "network", message: "Device request failed", uncertain: true } }), close: vi.fn() };
    const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "h", deviceId: "d" } });
    await expect(endpoint.request({}, { operationId: "op", params: {} }, {})).rejects.toMatchObject({ code: "network", uncertain: true });
    endpoint.close();
  });
});
