// One E2EE session's crypto and correlation, over one carrier at a time.

import { describe, it, expect, vi } from "vitest";
import { createSessionRpc } from "../src/core/sessionRpc.js";

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
function fakeCarrier({ sendFails = null } = {}) {
  const listeners = new Set();
  return {
    sent: [],
    send(envelope) {
      if (sendFails) return Promise.reject(new Error(sendFails));
      this.sent.push(envelope);
      return Promise.resolve();
    },
    onEnvelope: (fn) => {
      listeners.add(fn);
      return () => listeners.delete(fn);
    },
    onClose: () => () => {},
    close: () => {},
    deliver: (payload) => listeners.forEach((fn) => fn({ key: "key-1", frameFields: { payload } })),
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
