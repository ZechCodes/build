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

const v1Opening = { session_id: "session", opening_generation: "opening", greeting: { bridge_epoch: "epoch" } };

function desktopTerminal() {
  const success = (value) => ({ ok: true, value });
  const listeners = new Set();
  const api = {
    open: vi.fn().mockResolvedValue(success(v1Opening)),
    terminalOpen: vi.fn().mockResolvedValue(success({ terminal: "native-terminal" })),
    terminalCall: vi.fn().mockResolvedValue(success()),
    terminalDetach: vi.fn().mockResolvedValue(success()),
    terminalConsume: vi.fn().mockResolvedValue(success()),
    terminalClose: vi.fn().mockResolvedValue(success()),
    onTerminalEvent(listener) { listeners.add(listener); return () => listeners.delete(listener); },
    close: vi.fn().mockResolvedValue(success()),
  };
  const endpoint = createDesktopHttpEndpoint({ api, identity: { handle: "http-handle" } });
  const emit = (event) => {
    const requestId = api.terminalOpen.mock.lastCall[1];
    for (const listener of listeners) listener({ requestId, terminal: "native-terminal", ...event });
  };
  return { api, endpoint, emit, listeners };
}

describe("desktop endpoint terminal carrier", () => {
  it("requires the complete facade before advertising an open method", () => {
    const { api } = desktopTerminal();
    for (const name of Object.keys(api).filter((name) => /terminal/i.test(name))) {
      const endpoint = createDesktopHttpEndpoint({ api: { ...api, [name]: undefined }, identity: { handle: "h" } });
      expect(endpoint.openTerminal).toBeUndefined();
    }
  });

  it("passes only an explicit retry flag and preserves ticket refusal status", async () => {
    const { api, endpoint } = desktopTerminal();
    api.terminalOpen.mockResolvedValueOnce({
      ok: false, error: { code: "snapshot_write_timeout", status: 409, retryable: false },
    });
    const opening = await endpoint.open("range");
    await expect(endpoint.openTerminal(opening, { retry: false })).rejects.toMatchObject({
      code: "snapshot_write_timeout", httpStatus: 409, retryable: false,
    });
    expect(api.terminalOpen.mock.calls[0][2]).toEqual({ opening_generation: "opening" });
    const carrier = await endpoint.openTerminal(opening, { retry: true });
    expect(api.terminalOpen.mock.calls[1][2]).toEqual({ opening_generation: "opening", retry: true });
    carrier.close();
  });

  it("delivers native pushes and replies, ACKs receipt while paint is pending, and preserves close codes", async () => {
    const { api, endpoint, emit } = desktopTerminal();
    const opening = await endpoint.open("range");
    const carrier = await endpoint.openTerminal(opening);
    const onPush = vi.fn(() => new Promise(() => {}));
    const onClose = vi.fn();
    carrier.onPush(onPush);
    carrier.onClose(onClose);
    const attachId = vi.fn();
    const attached = carrier.call("agent.attach", { agent_id: "agent" }, { onAttachId: attachId });
    const [, , , id] = api.terminalCall.mock.lastCall;
    expect(attachId).toHaveBeenCalledWith(id);
    const frame = { id, ok: true, result: { term_id: "term" }, bridge_epoch: "epoch" };
    emit({ requestId: retiredRequestId, sequence: 1, type: "message", data: frame });
    emit({ sequence: 1, type: "message", data: frame });
    expect(await attached).toEqual(frame.result);
    const push = { type: "attachment_error", attach_id: id, reason: "snapshot_busy", bridge_epoch: "epoch" };
    emit({ sequence: 2, type: "message", data: push });
    expect(onPush).toHaveBeenCalledExactlyOnceWith(push);
    expect(api.terminalConsume.mock.calls).toEqual([["native-terminal", 1], ["native-terminal", 2]]);
    carrier.detach(id);
    expect(api.terminalDetach).toHaveBeenCalledWith("native-terminal", id);
    emit({ type: "close", code: "terminal_closed", closeCode: 4008, reason: "snapshot_write_timeout" });
    expect(onClose).toHaveBeenCalledWith(expect.objectContaining({ closeCode: 4008, reason: "snapshot_write_timeout", retryable: false }));
    emit({ type: "message", sequence: 3, data: push });
    expect(onPush).toHaveBeenCalledOnce();
  });

  it("cancels a pending open and closes a late native result", async () => {
    const { api, endpoint, listeners } = desktopTerminal();
    const result = Promise.withResolvers();
    api.terminalOpen.mockReturnValueOnce(result.promise);
    const opening = await endpoint.open("range");
    const controller = new AbortController();
    const pending = endpoint.openTerminal(opening, { signal: controller.signal });
    const [, attempt] = api.terminalOpen.mock.lastCall;
    controller.abort();
    await expect(pending).rejects.toMatchObject({ code: "scope_ended" });
    expect(api.terminalClose).toHaveBeenCalledWith(attempt);
    expect(listeners.size).toBe(0);
    result.resolve({ ok: true, value: { terminal: "late-terminal" } });
    await vi.waitFor(() => expect(api.terminalClose).toHaveBeenCalledWith("late-terminal"));
  });
});

it("sends paste chunks in order while resize and sibling input remain immediate", async () => {
  const { api, endpoint, emit } = desktopTerminal();
  const carrier = await endpoint.openTerminal(await endpoint.open("range"));
  const paste = carrier.call("term.input", { term_id: "shell", data: btoa("x".repeat(65536) + "end") });
  const later = carrier.call("term.input", { term_id: "shell", data: btoa("later") });
  const resize = carrier.call("term.resize", { term_id: "shell", cols: 90, rows: 24 });
  const sibling = carrier.call("term.input", { term_id: "agent", data: btoa("sibling") });
  const sent = () => api.terminalCall.mock.calls.map(([, method, params, id]) => [method, params.term_id, id]);
  expect(sent()).toEqual([["term.input", "shell", "1"], ["term.resize", "shell", "2"], ["term.input", "agent", "3"]]);
  expect(atob(api.terminalCall.mock.calls[0][2].data)).toHaveLength(65536);
  const reply = (id, sequence) => emit({ type: "message", sequence, data: { id, ok: true, result: {}, bridge_epoch: "epoch" } });
  reply("2", 1);
  reply("3", 2);
  reply("1", 3);
  await vi.waitFor(() => expect(atob(api.terminalCall.mock.lastCall[2].data)).toBe("end"));
  reply("4", 4);
  await vi.waitFor(() => expect(atob(api.terminalCall.mock.lastCall[2].data)).toBe("later"));
  reply("5", 5);
  await Promise.all([paste, later, resize, sibling]);
  carrier.close();
});

it("consumes only early pushes for the selected terminal and fences retired effects", async () => {
  const { api, endpoint, emit } = desktopTerminal();
  const result = Promise.withResolvers();
  api.terminalOpen.mockReturnValueOnce(result.promise);
  const opening = await endpoint.open("range");
  const pending = endpoint.openTerminal(opening);
  const data = { type: "term.output", bridge_epoch: "epoch", data: "eA==", cursor: 1 };
  emit({ terminal: "foreign", type: "message", data, sequence: 99 });
  emit({ type: "message", data, sequence: 1 });
  expect(api.terminalConsume).not.toHaveBeenCalled();
  result.resolve({ ok: true, value: { terminal: "native-terminal" } });
  const carrier = await pending;
  const pushed = vi.fn();
  carrier.onPush(pushed);
  expect(pushed).toHaveBeenCalledExactlyOnceWith(data);
  const input = carrier.call("term.input", { term_id: "shell", data: "eA==" });
  endpoint.close();
  await expect(input).rejects.toMatchObject({ code: "uncertain_delivery", uncertain: true });
  expect(api.terminalCall).toHaveBeenCalledOnce();
  emit({ type: "message", data, sequence: 2 });
  expect(pushed).toHaveBeenCalledOnce();
});
