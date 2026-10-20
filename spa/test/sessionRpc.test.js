// One E2EE session's crypto and correlation, over one carrier at a time.

import { afterEach, describe, it, expect, vi } from "vitest";
import { createSessionRpc } from "../src/core/sessionRpc.js";
import { openCarrier } from "../src/core/carrier.js";
import { CHUNK_BYTES } from "../src/core/chunk.js";
import { SQL_READ_LIMIT, isSqlRead } from "../src/core/sqlReadCredits.js";
import { readFileSync } from "node:fs";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** Sealed under a key and readable only under that one, as the real transport
 *  has it. */
const fakeTransport = {
  encryptFrame: async ({ sessionKeyB64, outerFields, frameFields }) => ({
    key: sessionKeyB64,
    outerFields,
    frameFields,
  }),
  decryptEnvelope: async ({ sessionKeyB64, envelope }) => {
    if (envelope.key !== sessionKeyB64) throw new Error("not this session's key");
    return { payload: envelope.frameFields.payload };
  },
};

/** A wire the test drives: what it was asked to carry, and what it delivers. */
function fakeCarrier({ sendFails = null, sent = () => {} } = {}) {
  const listeners = new Set();
  const closes = new Set();
  return {
    sent: [],
    send(envelope) {
      if (sendFails) return Promise.reject(new Error(sendFails));
      this.sent.push(envelope);
      sent(envelope, this);
      return Promise.resolve();
    },
    onEnvelope: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    onClose: (fn) => { closes.add(fn); return () => closes.delete(fn); },
    close: () => closes.forEach((fn) => fn()),
    deliver: (payload, key = "key-1") => listeners.forEach((fn) => fn({ key, frameFields: { payload } })),
    listeners,
  };
}

function rpcOn(carrier, overrides = {}) {
  const rpc = createSessionRpc({
    transport: fakeTransport,
    sessionId: "sess-1",
    sessionKeyB64: "key-1",
    deviceId: "dev-a",
    ...overrides,
  });
  rpc.rideOn(carrier);
  return rpc;
}

describe("createSessionRpc", () => {
  it("addresses the device, rides the carrier it was given, and answers its own reply", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);

    const reply = rpc.call("project.list", { scope: "all" });
    await tick();
    const [envelope] = carrier.sent;
    expect(envelope.key).toBe("key-1");
    expect(envelope.outerFields).toEqual({ session_id: "sess-1", route_to: "device:dev-a" });
    expect(envelope.frameFields.payload).toMatchObject({ method: "project.list", params: { scope: "all" } });

    carrier.deliver({ id: envelope.frameFields.payload.id, ok: true, result: { projects: [] } });
    await expect(reply).resolves.toEqual({ projects: [] });
  });

  it("stamps a background read's priority on the request envelope, and nothing on a foreground one", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);

    rpc.call("git.status", { run_id: "run-7" }, { priority: "background" }).catch(() => {});
    rpc.call("board.list", {}).catch(() => {});
    await tick();

    expect(carrier.sent[0].frameFields.payload.priority).toBe("background");
    expect(carrier.sent[1].frameFields.payload).not.toHaveProperty("priority");
  });

  it("rejects with the bridge's own error", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const reply = rpc.call("run.get", {});
    reply.catch(() => {});
    await tick();

    carrier.deliver({ id: carrier.sent[0].frameFields.payload.id, ok: false, error: "unknown id" });
    await expect(reply).rejects.toThrow("unknown id");
  });

  it("carries a coded refusal's error_code, retryable and details on the rejection", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const reply = rpc.call("run.get", {});
    reply.catch(() => {});
    await tick();

    carrier.deliver({
      id: carrier.sent[0].frameFields.payload.id,
      ok: false,
      error: "run-7 is busy",
      error_code: "busy",
      retryable: true,
      details: { run_id: "run-7" },
    });
    await expect(reply).rejects.toMatchObject({
      message: "run-7 is busy",
      error_code: "busy",
      retryable: true,
      details: { run_id: "run-7" },
    });
  });

  it("gives up on a call nobody answers", async () => {
    const rpc = rpcOn(fakeCarrier());
    await expect(rpc.call("slow.thing", {}, { timeoutMs: 5 })).rejects.toMatchObject({
      message: expect.stringContaining("slow.thing"),
      timedOut: true,
      uncertain: true,
    });
  });

  it("keeps a deadline-free call pending until the session fails", async () => {
    vi.useFakeTimers();
    try {
      const rpc = rpcOn(fakeCarrier());
      const waiting = rpc.call("workspace.get", {}, { timeoutMs: null });
      const observed = vi.fn();
      waiting.catch(observed);

      await vi.advanceTimersByTimeAsync(60_000);
      expect(observed).not.toHaveBeenCalled();

      rpc.fail(new Error("your device went offline"));
      await expect(waiting).rejects.toThrow("your device went offline");
    } finally {
      vi.useRealTimers();
    }
  });

  it("fails a call whose envelope never crossed the wire, rather than waiting out its timeout", async () => {
    const rpc = rpcOn(fakeCarrier({ sendFails: "the channel closed" }));
    await expect(rpc.call("term.input", {})).rejects.toThrow("the channel closed");
  });

  it("says so when nothing is carrying, in the words its owner chose", async () => {
    const rpc = createSessionRpc({
      transport: fakeTransport,
      sessionId: "sess-1",
      sessionKeyB64: "key-1",
      deviceId: "dev-a",
      noCarrier: () => new Error("your device went offline"),
    });
    await expect(rpc.call("project.list", {})).rejects.toThrow("your device went offline");
  });

  it("keeps the key when it swaps the wire, and reads from every carrier it holds", async () => {
    const relay = fakeCarrier();
    const channel = fakeCarrier();
    const rpc = rpcOn(relay);

    rpc.rideOn(channel);
    const reply = rpc.call("project.list", {});
    await tick();
    expect(channel.sent).toHaveLength(1);
    expect(relay.sent).toHaveLength(0);
    expect(channel.sent[0].key).toBe("key-1");

    // The answer to a call may come back over the wire it did not go out on:
    // signaling is pinned to the relay while a channel carries everything else.
    relay.deliver({ id: channel.sent[0].frameFields.payload.id, ok: true, result: "either wire" });
    await expect(reply).resolves.toBe("either wire");
  });

  it("registers one reader per carrier however many times it is handed the same wire", () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    rpc.rideOn(carrier);
    expect(carrier.listeners.size).toBe(1);
  });

  it("pins one call to the wire the caller names", async () => {
    const relay = fakeCarrier();
    const channel = fakeCarrier();
    const rpc = rpcOn(relay);
    rpc.rideOn(channel);

    rpc.call("rtc.ice", {}, { carrier: relay }).catch(() => {});
    await tick();

    expect(relay.sent).toHaveLength(1);
    expect(channel.sent).toHaveLength(0);
  });

  it("waits for a wire that is on its way, inside the call's own deadline", async () => {
    const relay = fakeCarrier();
    const rpc = rpcOn(fakeCarrier());
    let arrive;
    const onItsWay = new Promise((resolve) => (arrive = resolve));

    const pinned = rpc.call("rtc.offer", {}, { carrier: onItsWay });
    await tick();
    expect(relay.sent).toHaveLength(0);

    rpc.readFrom(relay); // the switch hands over every wire it holds
    arrive(relay);
    await tick();
    relay.deliver({ id: relay.sent[0].frameFields.payload.id, ok: true, result: { sdp: "v=0" } });
    await expect(pinned).resolves.toEqual({ sdp: "v=0" });
  });

  it("gives up on a wire that never comes, at the deadline the call set", async () => {
    const rpc = rpcOn(fakeCarrier());
    const waiting = rpc.call("rtc.offer", {}, { carrier: new Promise(() => {}), timeoutMs: 5 });
    await expect(waiting).rejects.toMatchObject({
      message: expect.stringContaining("rtc.offer"),
      timedOut: true,
    });
    await expect(waiting).rejects.not.toMatchObject({ uncertain: true });
  });

  it("hands a frame nobody asked for to whoever is listening for pushes", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const pushes = [];
    const unsubscribe = rpc.onPush((payload) => pushes.push(payload));

    carrier.deliver({ type: "entity.changed", id: "run-7" });
    await tick();
    expect(pushes).toEqual([{ type: "entity.changed", id: "run-7" }]);

    unsubscribe();
    carrier.deliver({ type: "board.changed" });
    await tick();
    expect(pushes).toHaveLength(1);
  });

  it("drops a reply to a call that already gave up, rather than calling it a push", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const pushes = [];
    rpc.onPush((payload) => pushes.push(payload));

    await expect(rpc.call("slow.thing", {}, { timeoutMs: 5 })).rejects.toThrow();
    carrier.deliver({ id: carrier.sent[0].frameFields.payload.id, ok: true, result: {} });
    await tick();

    expect(pushes).toEqual([]);
  });

  it("cannot read a frame written for another session", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const pushes = [];
    rpc.onPush((payload) => pushes.push(payload));

    carrier.listeners.forEach((fn) => fn({ key: "key-other", frameFields: { payload: { type: "board.changed" } } }));
    await tick();

    expect(pushes).toEqual([]);
  });

  it("vouches for the connection with whatever it just read", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    expect(rpc.lastFrameAt()).toBe(0);

    carrier.deliver({ type: "board.changed" });
    await tick();
    expect(rpc.lastFrameAt()).toBeGreaterThan(0);

    rpc.rideOn(fakeCarrier()); // another wire's traffic vouches for nothing here
    expect(rpc.lastFrameAt()).toBe(0);
  });

  it("tells every call still waiting why it will never answer", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const waiting = rpc.call("project.list", {});
    await tick();

    rpc.fail(new Error("nothing is carrying this session"));

    await expect(waiting).rejects.toThrow("nothing is carrying this session");
  });

  it("marks a carrier loss after handoff as delivery-uncertain", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const waiting = rpc.call("thread.post", { body: "do it" });
    waiting.catch(() => {});
    await tick();

    rpc.fail(new Error("your device went offline"));

    await expect(waiting).rejects.toMatchObject({
      message: "your device went offline",
      uncertain: true,
    });
  });

  it("keeps a loss before carrier handoff definite", async () => {
    let arrive;
    const onItsWay = new Promise((resolve) => (arrive = resolve));
    const rpc = rpcOn(fakeCarrier());
    const waiting = rpc.call("thread.post", { body: "do it" }, { carrier: onItsWay });
    waiting.catch(() => {});
    await tick();

    rpc.fail(new Error("your device went offline"));

    await expect(waiting).rejects.toMatchObject({
      message: "your device went offline",
    });
    await expect(waiting).rejects.not.toMatchObject({ uncertain: true });
    arrive(null);
  });

  it("does not send later when a carrier arrives after the call was failed", async () => {
    let arrive;
    const onItsWay = new Promise((resolve) => (arrive = resolve));
    const carrier = fakeCarrier();
    const rpc = rpcOn(fakeCarrier());
    const waiting = rpc.call("thread.post", { body: "do it" }, { carrier: onItsWay });
    waiting.catch(() => {});
    await tick();

    rpc.fail(new Error("your device went offline"));
    arrive(carrier);
    await expect(waiting).rejects.toThrow("offline");
    await tick();

    expect(carrier.sent).toEqual([]);
  });

  it("does not send an envelope whose encryption finishes after close", async () => {
    let finishEncryption;
    const delayedTransport = {
      ...fakeTransport,
      encryptFrame: vi.fn(() => new Promise((resolve) => (finishEncryption = resolve))),
    };
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier, { transport: delayedTransport });
    const waiting = rpc.call("thread.post", { body: "do it" });
    waiting.catch(() => {});
    await tick();

    rpc.close(new Error("session closed"));
    finishEncryption({ key: "key-1", outerFields: {}, frameFields: {} });
    await expect(waiting).rejects.toMatchObject({ message: "session closed" });
    await tick();

    expect(carrier.sent).toEqual([]);
  });

  it("keeps an explicit carrier send failure definite", async () => {
    const rpc = rpcOn(fakeCarrier({ sendFails: "the channel closed before handoff" }));

    const waiting = rpc.call("thread.post", { body: "do it" });

    await expect(waiting).rejects.toMatchObject({ message: "the channel closed before handoff" });
    await expect(waiting).rejects.not.toMatchObject({ uncertain: true });
  });

  it("answers nothing once its client has said so, in the words it closed with", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier, { noCarrier: () => new Error("your device went offline") });
    const waiting = rpc.call("project.list", {});
    await tick();

    rpc.close(new Error("session closed"));

    await expect(waiting).rejects.toMatchObject({ message: "session closed", uncertain: true });
    await expect(rpc.call("project.list", {})).rejects.toThrow("your device went offline");
    expect(carrier.sent).toHaveLength(1);
  });
});

describe("SQL admission under held work", () => {
  const sessions = [];
  const open = (wire, options = {}) => {
    const rpc = rpcOn(wire, { timeoutMs: 0, ...options });
    sessions.push(rpc);
    return rpc;
  };
  const ask = (rpc, method = "tasks.get", options = {}, params = {}) => {
    const answer = rpc.call(method, params, options);
    answer.catch(() => {});
    return answer;
  };
  const payloadOf = (envelope) => envelope.frameFields.payload;
  afterEach(() => sessions.splice(0).forEach((rpc) => rpc.close(new Error("test ended"))));

  it("at fixture N=B=16 holds extra SQL unsent while same-carrier control and snapshots answer", async () => {
    const serverSource = readFileSync(new URL("../../bridge/src/carrier/sql.rs", import.meta.url), "utf8");
    const backlogLimit = Number(serverSource.match(/const SQL_BACKLOG: usize = (\d+);/)[1]);
    expect(backlogLimit).toBe(16);
    const backlog = [];
    // The intake fixture holds every SQL execution until a connection returns.
    // The bridge counterpart holds all eight actual pool connections.
    const wire = fakeCarrier({ sent: (envelope, sending) => {
      const request = payloadOf(envelope);
      if (isSqlRead(request.method)) backlog.push(request);
      else sending.deliver({ id: request.id, ok: true, result: { method: request.method } });
    } });
    const rpc = open(wire, { sqlReadLimit: backlogLimit });
    const reads = Array.from({ length: 17 }, () => ask(rpc));
    await tick();
    expect(backlog).toHaveLength(16);
    const extraId = "r17";
    expect(wire.sent.some((entry) => payloadOf(entry).id === extraId)).toBe(false);
    await expect(ask(rpc, "ping")).resolves.toEqual({ method: "ping" });
    await expect(ask(rpc, "bridge.stats")).resolves.toEqual({ method: "bridge.stats" });
    await expect(ask(rpc, "tasks.columns")).resolves.toEqual({ method: "tasks.columns" });
    expect(backlog).toHaveLength(16);

    const first = backlog.shift();
    wire.deliver({ id: first.id, ok: true, result: { released: true } });
    await expect(reads[0]).resolves.toEqual({ released: true });
    await tick();
    expect(backlog).toHaveLength(16);
    expect(backlog.at(-1).id).toBe(extraId);
  });

  it("keeps every interleaved carrier and priority burst within the shared credit bound", async () => {
    const live = new Map();
    const sent = (envelope, wire) => {
      const request = payloadOf(envelope);
      expect(live.size).toBeLessThan(SQL_READ_LIMIT);
      live.set(request.id, { wire, request });
    };
    const wires = [fakeCarrier({ sent }), fakeCarrier({ sent })];
    const rpc = open(wires[0]);
    rpc.readFrom(wires[1]);
    const replies = [];
    const completed = [];
    for (let index = 0; index < 40; index += 1) {
      const wire = wires[index % 2];
      rpc.rideOn(wire); // a switch alone returns no credit
      replies.push(ask(rpc, "tasks.get", { carrier: wire, priority: index % 3 ? "foreground" : "background" }));
      await tick();
      expect(live.size).toBe(Math.min(index + 1, SQL_READ_LIMIT));
    }
    while (live.size) {
      const { request, wire } = [...live.values()].at(-1);
      wire.deliver({ id: request.id, accepted: true });
      wires.find((other) => other !== wire).deliver({ id: request.id, ok: true, result: "wrong carrier" });
      await tick();
      expect(live.size).toBeLessThanOrEqual(SQL_READ_LIMIT);
      live.delete(request.id);
      completed.push(request.id);
      wire.deliver({ id: request.id, ok: true, result: request.id });
      wire.deliver({ id: request.id, ok: true, result: "duplicate" });
      await tick();
      expect(live.size).toBe(Math.min(40 - completed.length, SQL_READ_LIMIT));
    }
    expect(new Set(await Promise.all(replies)).size).toBe(40);
    for (const wire of wires) {
      expect(wire.sent.some((entry) => payloadOf(entry).priority === "background")).toBe(true);
      expect(wire.sent.some((entry) => payloadOf(entry).priority === undefined)).toBe(true);
    }
  });

  it("cannot settle replacement-opening callers with old completions even when request ids repeat", async () => {
    const wire = fakeCarrier();
    const old = open(wire);
    for (let index = 0; index < SQL_READ_LIMIT; index += 1) ask(old);
    await tick();
    const oldIds = wire.sent.map((entry) => payloadOf(entry).id);
    old.close(new Error("old opening ended"));
    const fresh = open(wire, { sessionId: "sess-2", sessionKeyB64: "key-2" });
    const settled = vi.fn();
    const replies = Array.from({ length: SQL_READ_LIMIT + 1 }, () => ask(fresh).then(settled));
    replies.forEach((reply) => reply.catch(() => {}));
    await tick();
    expect(wire.sent).toHaveLength(SQL_READ_LIMIT * 2);
    for (const id of oldIds) wire.deliver({ id, ok: true, result: "old opening" });
    await tick();
    expect(settled).not.toHaveBeenCalled();
    expect(wire.sent).toHaveLength(SQL_READ_LIMIT * 2);
    wire.deliver({ id: oldIds[0], ok: true, result: "new opening" }, "key-2");
    await tick();
    expect(settled).toHaveBeenCalledExactlyOnceWith("new opening");
    expect(wire.sent).toHaveLength(SQL_READ_LIMIT * 2 + 1);
  });

  it("retains SQL credit when the actual DataChannel carrier rejects after writing a chunk", async () => {
    const writes = [];
    const channel = {
      readyState: "open", bufferedAmount: 0,
      addEventListener: () => {}, close: () => {},
      send(text) {
        if (writes.length === 1) throw new Error("chunk enqueue failed");
        writes.push(text);
      },
    };
    const wire = openCarrier({ channel });
    const rpc = open(wire);
    const failed = ask(rpc, "tasks.get", {}, { task_id: "x".repeat(CHUNK_BYTES * 2) });
    await expect(failed).rejects.toMatchObject({ message: "chunk enqueue failed", uncertain: true, retryable: true });
    expect(writes).toHaveLength(1);
    const replacement = fakeCarrier();
    rpc.rideOn(replacement);
    for (let index = 0; index < SQL_READ_LIMIT; index += 1) ask(rpc);
    await tick();
    expect(replacement.sent).toHaveLength(SQL_READ_LIMIT - 1);
    wire.close();
    await tick();
    expect(replacement.sent).toHaveLength(SQL_READ_LIMIT);
  });
});

// A receipt is the device saying it has the request. It separates two
// questions one deadline was answering at once — did this reach the machine,
// and is the machine taking too long — and only the first is worth giving up
// on. A bridge with eleven agents on it can take a while to answer; a client
// that called that a failure is how a delivered message read as failed.
describe("a receipt on admission", () => {
  /** The id the rpc actually minted for the call it just sent. */
  const askedId = (carrier) => carrier.sent.at(-1).frameFields.payload.id;
  const receipt = (carrier) => carrier.deliver({ id: askedId(carrier), accepted: true });

  it("takes the call off the path's deadline, and waits for the answer", async () => {
    vi.useFakeTimers();
    try {
      const carrier = fakeCarrier();
      const rpc = rpcOn(carrier);
      const asked = rpc.call("thread.post", {}, { timeoutMs: 5000 });
      const observed = vi.fn();
      asked.catch(observed);
      await vi.advanceTimersByTimeAsync(0);

      receipt(carrier);
      await vi.advanceTimersByTimeAsync(30_000); // far past the path's deadline
      expect(observed, "a request the device has is not a request that failed").not.toHaveBeenCalled();

      carrier.deliver({ id: askedId(carrier), ok: true, result: { posted_sequence: 7 } });
      await expect(asked).resolves.toEqual({ posted_sequence: 7 });
    } finally {
      vi.useRealTimers();
    }
  });

  it("still gives up on a machine that took the request and never answered", async () => {
    vi.useFakeTimers();
    try {
      const carrier = fakeCarrier();
      const rpc = rpcOn(carrier);
      const asked = rpc.call("thread.attach", {}, { timeoutMs: 5000 });
      const observed = vi.fn();
      asked.catch(observed);
      await vi.advanceTimersByTimeAsync(0);
      receipt(carrier);

      await vi.advanceTimersByTimeAsync(119_000);
      expect(observed).not.toHaveBeenCalled();
      await vi.advanceTimersByTimeAsync(2_000);
      await expect(asked).rejects.toMatchObject({ timedOut: true });
    } finally {
      vi.useRealTimers();
    }
  });

  it("keeps the path's deadline for a call nothing has acknowledged", async () => {
    await expect(rpcOn(fakeCarrier()).call("board.list", {}, { timeoutMs: 5 }))
      .rejects.toMatchObject({ timedOut: true });
  });

  it("tells the caller its request landed, so a send can say queued rather than failed", async () => {
    const carrier = fakeCarrier();
    const rpc = rpcOn(carrier);
    const landed = vi.fn();
    const asked = rpc.call("thread.post", {}, { onReceipt: landed });
    // The send is a chain of awaits (the wire, then the encryption), so the
    // envelope is on the carrier a few turns later.
    for (let turn = 0; turn < 6; turn += 1) await Promise.resolve();

    // Decryption is async, so a delivered frame lands a turn later.
    const settle = async () => { for (let turn = 0; turn < 4; turn += 1) await Promise.resolve(); };
    receipt(carrier);
    await settle();
    expect(landed).toHaveBeenCalledTimes(1);

    receipt(carrier); // a second receipt for the same call is not a second landing
    await settle();
    expect(landed).toHaveBeenCalledTimes(1);

    carrier.deliver({ id: askedId(carrier), ok: true, result: {} });
    await asked;
  });
});
