import { describe, expect, it, vi } from "vitest";
import { createDesktopHttpEndpoint } from "../src/core/httpDesktopEndpoint.js";

const retiredRequestId = "00000000-0000-4000-8000-000000000001";

describe("typed desktop HTTP facade", () => {
  it("isolates a delayed subscription from another stream replenishing 129 events", async () => {
    const listeners = new Set(); let accept;
    const api = {
      subscribe: vi.fn((handle) => handle === "pending-handle" ? new Promise((resolve) => { accept = resolve; }) : Promise.resolve({ ok: true, value: { stream: "active-stream" } })),
      onEvent(fn) { listeners.add(fn); return () => listeners.delete(fn); },
      ack: vi.fn().mockResolvedValue({ ok: true }), unsubscribe: vi.fn().mockResolvedValue({ ok: true }), close: vi.fn().mockResolvedValue({ ok: true }),
    };
    const emit = (message) => {
      const requestId = api.subscribe.mock.calls.find(([handle]) => handle === message.handle)?.[2]?.requestId;
      for (const listener of listeners) listener({ requestId, ...message });
    };
    const waiting = createDesktopHttpEndpoint({ api, identity: { handle: "pending-handle" } });
    const active = createDesktopHttpEndpoint({ api, identity: { handle: "active-handle" } });
    const lost = vi.fn(); waiting.onLost(lost);
    const consumed = vi.fn();
    const pending = waiting.subscribe({}, consumed).then((value) => ({ value }), (error) => ({ error }));
    const activeReady = active.subscribe({}, () => {});
    const attempts = api.subscribe.mock.calls.map(([, , options]) => options.requestId);
    expect(new Set(attempts).size).toBe(2);
    emit({ handle: "pending-handle", stream: "pending-stream", sequence: 1, event: "hello", data: {} });
    emit({ handle: "active-handle", stream: "active-stream", sequence: 1, event: "hello", data: {} });
    await activeReady;
    for (let sequence = 2; sequence <= 130; sequence += 1) {
      emit({ handle: "active-handle", stream: "active-stream", sequence, event: "event", data: {} });
      // The other stream can replenish its own native ACK window while this
      // invocation remains pending; a one-window global buffer is insufficient.
      await vi.waitFor(() => expect(api.ack).toHaveBeenLastCalledWith("active-stream", sequence), { interval: 1 });
    }
    accept({ ok: true, value: { stream: "pending-stream" } });
    const result = await pending;
    expect(result.error).toBeUndefined();
    expect(lost).not.toHaveBeenCalled();
    expect(api.close).not.toHaveBeenCalled();
    expect(consumed).toHaveBeenCalledExactlyOnceWith({ event: "hello", data: {} });
    expect(api.ack).toHaveBeenCalledWith("pending-stream", 1);
    waiting.close(); active.close();
  });

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
    const requestId = api.subscribe.mock.calls[0][2].requestId;
    take({ handle: "h", requestId, stream: "stream", sequence: 1, event: "hello", data: {} });
    await pending;
    take({ handle: "h", requestId, stream: "other", sequence: 1, event: "event", data: {} });
    take({ handle: "h", requestId, stream: "stream", sequence: 2, event: "event", data: {} });
    await vi.waitFor(() => expect(apply).toBeTypeOf("function"));
    expect(api.ack).toHaveBeenCalledTimes(1);
    apply();
    await vi.waitFor(() => expect(api.ack).toHaveBeenCalledWith("stream", 2));
    endpoint.close();
  });

  it.each(["items", "bytes"])("retains the early own-stream %s bound", async (limit) => {
    const { endpoint, api, emit, accept, lost } = heldSubscription();
    const pending = endpoint.subscribe({}, () => {}).catch((error) => error);
    const total = limit === "items" ? 129 : 1;
    for (let sequence = 1; sequence <= total; sequence += 1) {
      emit({ handle: "h", stream: "stream", sequence, event: sequence === 1 ? "hello" : "event", data: limit === "bytes" ? "x".repeat(1024 * 1024) : {} });
    }
    expect(lost).toHaveBeenCalledOnce();
    expect(api.close).toHaveBeenCalledExactlyOnceWith("h");
    accept();
    expect(await pending).toMatchObject({ code: "scope_ended" });
  });

  it("retains 128 early own events and drops uncorrelated bytes before accounting", async () => {
    const { endpoint, api, emit, accept, lost } = heldSubscription();
    const consumed = vi.fn();
    const pending = endpoint.subscribe({}, consumed);
    const uncorrelated = { toJSON() { throw new Error("Uncorrelated data must not be inspected"); } };
    emit({ stream: "stream", sequence: 1, event: "hello", data: uncorrelated });
    emit({ handle: "foreign", stream: "stream", sequence: 1, event: "hello", data: uncorrelated });
    emit({ handle: "h", requestId: undefined, stream: "stream", sequence: 1, event: "hello", data: uncorrelated });
    emit({ handle: "h", requestId: retiredRequestId, stream: "stream", sequence: 1, event: "hello", data: uncorrelated });
    for (let sequence = 1; sequence <= 128; sequence += 1) {
      emit({ handle: "h", stream: "stream", sequence, event: sequence === 1 ? "hello" : "event", data: {} });
    }
    accept(); await pending;
    await vi.waitFor(() => expect(api.ack).toHaveBeenCalledWith("stream", 128));
    expect(consumed).toHaveBeenCalledTimes(128);
    expect(lost).not.toHaveBeenCalled();
    expect(api.close).not.toHaveBeenCalled();
    endpoint.close();
  });

  it("counts the applying own event in the bounded queue", async () => {
    const { endpoint, api, emit, accept, lost } = heldSubscription();
    let finish;
    const pending = endpoint.subscribe({}, ({ event }) => event === "hello" ? undefined : new Promise((resolve) => { finish = resolve; }));
    emit({ handle: "h", stream: "stream", sequence: 1, event: "hello", data: {} });
    accept(); await pending;
    emit({ handle: "h", stream: "stream", sequence: 2, event: "event", data: {} });
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
    for (let sequence = 3; sequence <= 129; sequence += 1) emit({ handle: "h", stream: "stream", sequence, event: "event", data: {} });
    expect(lost).not.toHaveBeenCalled();
    emit({ handle: "h", stream: "stream", sequence: 130, event: "event", data: {} });
    expect(lost).toHaveBeenCalledOnce();
    expect(api.unsubscribe).toHaveBeenCalledWith("stream");
    finish();
  });

  it("permits only one subscription attempt on its fresh endpoint handle", async () => {
    const { endpoint, api, emit, accept } = heldSubscription();
    const pending = endpoint.subscribe({}, () => {});
    await expect(endpoint.subscribe({}, () => {})).rejects.toMatchObject({ code: "protocol" });
    emit({ handle: "h", stream: "stream", sequence: 1, event: "hello", data: {} });
    accept(); await pending;
    await expect(endpoint.subscribe({}, () => {})).rejects.toMatchObject({ code: "protocol" });
    expect(api.subscribe).toHaveBeenCalledOnce();
    endpoint.close();
  });

  it("does not reuse a failed subscription attempt", async () => {
    const api = { subscribe: vi.fn().mockResolvedValue({ ok: false, error: { code: "busy" } }), onEvent: () => vi.fn(), close: vi.fn() };
    const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "h" } });
    await expect(endpoint.subscribe({}, () => {})).rejects.toMatchObject({ code: "busy" });
    await expect(endpoint.subscribe({}, () => {})).rejects.toMatchObject({ code: "protocol" });
    expect(api.subscribe).toHaveBeenCalledOnce();
    endpoint.close();
  });

  it("ignores same-handle stale stream controls after selection and all events after retirement", async () => {
    const { endpoint, api, emit, accept, lost } = heldSubscription();
    const consumed = vi.fn();
    const pending = endpoint.subscribe({}, consumed);
    emit({ handle: "h", stream: "stream", sequence: 1, event: "hello", data: {} });
    accept(); await pending;
    for (const event of ["reset_required", "closing"]) {
      emit({ handle: "h", stream: "retired-stream", sequence: 2, event, data: {} });
      emit({ handle: "other-handle", stream: "stream", sequence: 2, event, data: {} });
      emit({ stream: "stream", sequence: 2, event, data: {} });
      emit({ handle: "h", requestId: retiredRequestId, stream: "stream", sequence: 2, event, data: {} });
    }
    expect(consumed).toHaveBeenCalledOnce();
    expect(api.close).not.toHaveBeenCalled();
    endpoint.close();
    emit({ handle: "h", stream: "stream", sequence: 2, event: "reset_required", data: {} });
    expect(consumed).toHaveBeenCalledOnce();
    expect(lost).not.toHaveBeenCalled();
    expect(api.close).toHaveBeenCalledOnce();
  });

  it("isolates 129 retired-attempt controls on the same handle before subscribe resolves", async () => {
    const { endpoint, api, emit, accept, lost } = heldSubscription();
    const consumed = vi.fn();
    const pending = endpoint.subscribe({}, consumed).then((value) => ({ value }), (error) => ({ error }));
    emit({ handle: "h", stream: "stream", sequence: 1, event: "hello", data: {} });
    for (let sequence = 1; sequence <= 129; sequence += 1) {
      emit({ handle: "h", requestId: retiredRequestId, stream: "stream", sequence, event: sequence % 2 ? "reset_required" : "closing", data: {} });
    }
    accept();
    expect((await pending).error).toBeUndefined();
    expect(api.subscribe).toHaveBeenCalledWith("h", { kind: "application" }, { requestId: expect.stringMatching(/^[0-9a-f-]{36}$/) });
    expect(consumed).toHaveBeenCalledExactlyOnceWith({ event: "hello", data: {} });
    expect(lost).not.toHaveBeenCalled();
    expect(api.close).not.toHaveBeenCalled();
    endpoint.close();
  });


  it("propagates safe uncertainty without leaking arbitrary response objects", async () => {
    const api = { request: vi.fn().mockResolvedValue({ ok: false, error: { code: "network", message: "Device request failed", uncertain: true } }), close: vi.fn() };
    const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "h", deviceId: "d" } });
    await expect(endpoint.request({}, { operationId: "op", params: {} }, {})).rejects.toMatchObject({ code: "network", uncertain: true });
    endpoint.close();
  });
});

function heldSubscription() {
  let listener; let resolve;
  const api = {
    subscribe: vi.fn(() => new Promise((done) => { resolve = done; })),
    onEvent(fn) { listener = fn; return vi.fn(); }, ack: vi.fn().mockResolvedValue({ ok: true }),
    unsubscribe: vi.fn().mockResolvedValue({ ok: true }), close: vi.fn().mockResolvedValue({ ok: true }),
  };
  const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "h" } });
  const lost = vi.fn(); endpoint.onLost(lost);
  return { endpoint, api, lost,
    emit: (message) => listener({ requestId: api.subscribe.mock.calls[0]?.[2]?.requestId, ...message }),
    accept: () => resolve({ ok: true, value: { stream: "stream" } }) };
}
