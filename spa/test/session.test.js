// One E2EE session with one device: minted over a rendezvous, carried by the
// peer connection and by nothing else (spec rules 1 and 2).

import { describe, it, expect, vi } from "vitest";
import { DEFAULT_RPC_TIMEOUT_MS, openSession, replyOrNothing } from "../src/core/session.js";
import { ApiError, selectAdapter } from "../src/core/bridgeApi/index.js";

// ---- fakes -------------------------------------------------------------------

// A transparent "transport": envelopes are plain objects, no real crypto.
const fakeTransport = {
  ready: async () => {},
  createSessionInit: async ({ sessionId, deviceId }) => ({
    sessionKeyB64: `key-${deviceId}`,
    sessionInit: { session_id: sessionId, device_id: deviceId },
  }),
  openSessionAccept: async () => {},
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

/** A carrier a test drives: what it was handed, and what it hands back. Its
 *  listener slots are subscriptions, as the real carrier's are. */
function fakeCarrier({ sendFails = null } = {}) {
  const envelopeListeners = new Set();
  const closeListeners = new Set();
  const subscribe = (listeners) => (fn) => {
    listeners.add(fn);
    return () => listeners.delete(fn);
  };
  const carrier = {
    sent: [],
    closed: false,
    send: async (envelope) => {
      if (sendFails) throw new Error(sendFails);
      carrier.sent.push(envelope);
    },
    onEnvelope: subscribe(envelopeListeners),
    onClose: subscribe(closeListeners),
    close: () => {
      carrier.closed = true;
      closeListeners.forEach((fn) => fn());
    },
    reply: (payload) => envelopeListeners.forEach((fn) => fn({ frameFields: { payload } })),
  };
  return carrier;
}

const replyTo = (carrier, index = -1) => carrier.sent.at(index).frameFields.payload;
const answer = (carrier, result) => carrier.reply({ id: replyTo(carrier).id, ok: true, result });

/** The device's rendezvous, as this module holds it: it mints sessions and
 *  hands out a signaling wire per session. Whether it is a relay socket or a
 *  direct listener is not this file's subject (test/rendezvous.test.js). */
function fakeRendezvous({ deviceId = "dev-a", mintFails = null } = {}) {
  const rendezvous = {
    minted: [],
    wires: [],
    open: vi.fn(async () => {}),
    mint: vi.fn(async (args = {}) => {
      rendezvous.minted.push(args);
      if (mintFails) throw new Error(mintFails);
      return {
        sessionId: args.sessionId || "sess-1",
        sessionKeyB64: args.sessionKeyB64 || `key-${deviceId}`,
        deviceId,
      };
    }),
    signalCarrier: vi.fn((sessionId) => {
      const wire = fakeCarrier();
      wire.sessionId = sessionId;
      rendezvous.wires.push(wire);
      return wire;
    }),
    isOpen: () => true,
    onClosed: () => () => {},
    close: vi.fn(),
  };
  return rendezvous;
}

/** A session, its rendezvous, and what it reported. */
async function opened(overrides = {}) {
  const events = { lost: 0, pushes: [] };
  const rendezvous = overrides.rendezvous || fakeRendezvous();
  const session = await openSession({
    rendezvous,
    transport: fakeTransport,
    deviceId: "dev-a",
    onLost: () => events.lost++,
    onPush: (payload) => events.pushes.push(payload),
    ...overrides,
  });
  return { session, rendezvous, events, signaling: () => rendezvous.wires.at(-1) };
}

/** A session that is live: the two channels opened and the app one is carrying. */
async function carrying(overrides = {}) {
  const stood = await opened(overrides);
  const peer = fakeCarrier();
  await stood.session.peer(peer);
  return { ...stood, peer };
}

// ---- minting -----------------------------------------------------------------

describe("openSession", () => {
  it("mints one session over the rendezvous and takes its signaling wire", async () => {
    const { session, rendezvous } = await opened();

    expect(rendezvous.mint).toHaveBeenCalledTimes(1);
    expect(rendezvous.signalCarrier).toHaveBeenCalledWith("sess-1");
    expect(session.deviceId).toBe("dev-a");
  });

  it("fails to open when the device does not answer the rendezvous", async () => {
    await expect(
      opened({ rendezvous: fakeRendezvous({ mintFails: "device did not answer" }) }),
    ).rejects.toThrow("device did not answer");
  });
});

// ---- which wire a call rides -------------------------------------------------

describe("a session that is being upgraded", () => {
  it("holds the user's calls for the channel rather than putting them on the relay", async () => {
    const { session, signaling } = await opened();

    const pending = session.call("board.list", {});
    await tick();
    expect(signaling().sent).toHaveLength(0); // rule 1: the relay is not a data plane

    const peer = fakeCarrier();
    await session.peer(peer);
    await tick();
    expect(peer.sent).toHaveLength(1);
    answer(peer, { tasks: [] });
    await expect(pending).resolves.toEqual({ tasks: [] });
  });

  it("puts rtc.* on the rendezvous, never on the channel it is negotiating", async () => {
    const { session, signaling, peer } = await carrying();

    const offered = session.call("rtc.offer", { sdp: "v=0" });
    await tick();
    expect(peer.sent).toHaveLength(0);
    expect(replyTo(signaling()).method).toBe("rtc.offer");
    answer(signaling(), { sdp: "v=0 a" });
    await expect(offered).resolves.toEqual({ sdp: "v=0 a" });
  });

  it("sends the user's calls over the channel once it carries, and answers from it", async () => {
    const { session, peer, signaling } = await carrying();

    const reply = session.call("board.list", {});
    await tick();
    expect(peer.sent).toHaveLength(1);
    expect(signaling().sent).toHaveLength(0);
    answer(peer, { tasks: [] });
    await expect(reply).resolves.toEqual({ tasks: [] });
  });

  it("signals even while the app is paused — the pause holds user actions, not the upgrade", async () => {
    let paused = false;
    const { session, signaling } = await carrying({ isPaused: () => paused });
    paused = true;

    session.call("rtc.offer", { sdp: "v=0" }).catch(() => {});
    await tick();
    expect(replyTo(signaling()).method).toBe("rtc.offer");
    await expect(session.call("board.list", {})).rejects.toThrow(/offline/);
  });

  it("refuses what was waiting for a wire when the caller says the device is blocked", async () => {
    const { session } = await opened();
    const pending = session.call("board.list", {});

    session.fail(new Error("this device is blocked: timeout"));

    await expect(pending).rejects.toThrow("this device is blocked: timeout");
  });
});

// ---- the rendezvous comes and goes under a live session ----------------------

describe("a session whose rendezvous has closed", () => {
  it("is not lost: the rendezvous is not a carrier", async () => {
    const { session, events, signaling } = await carrying();

    signaling().close();
    await tick();

    expect(events.lost).toBe(0);
    const reply = session.call("board.list", {});
    await tick();
    reply.catch(() => {});
    expect(reply).toBeInstanceOf(Promise);
  });

  it("answers an ICE restart asked for while it was closed, once it is reopened", async () => {
    const { session, rendezvous, signaling } = await carrying();
    const first = signaling();
    first.close(); // closed once the channels opened (rule 4)

    const restart = session.call("rtc.offer", { sdp: "v=0 restart" });
    await tick();
    expect(first.sent).toHaveLength(0);

    await session.reattachSignaling();
    await tick();

    // The same session, re-presented: a carrier re-attach, not a second session.
    expect(rendezvous.mint).toHaveBeenLastCalledWith({ sessionId: "sess-1", sessionKeyB64: "key-dev-a" });
    const wire = signaling();
    expect(replyTo(wire).method).toBe("rtc.offer");
    answer(wire, { sdp: "v=0 answer" });
    await expect(restart).resolves.toEqual({ sdp: "v=0 answer" });
  });
});

// ---- what ends a session -----------------------------------------------------

describe("the end of a session", () => {
  it("is the channel going, once and for all", async () => {
    const { session, events, peer } = await carrying();
    const pending = session.call("board.list", {});
    await tick();
    expect(peer.sent).toHaveLength(1);

    await session.peer(null);

    await expect(pending).rejects.toThrow(/offline/);
    expect(events.lost).toBe(1);
    await expect(session.call("board.list", {})).rejects.toThrow(/offline/);
  });

  it("is reported once, whatever else drops after it", async () => {
    const { session, events, signaling } = await carrying();
    session.peer(null);
    signaling().close();
    session.peer(null);
    expect(events.lost).toBe(1);
  });

  it("says nothing about carriers dropping after a deliberate close", async () => {
    const { session, events, peer, signaling } = await carrying();
    session.close();
    peer.close();
    signaling().close();
    expect(events.lost).toBe(0);
  });

  it("lets its lease on the rendezvous go when it closes", async () => {
    const { session, signaling } = await carrying();
    session.close();
    expect(signaling().closed).toBe(true);
  });

  it("re-establishes on every carrier it takes", async () => {
    const { session } = await opened();
    const carrierChanges = [];
    session.onCarrier(() => carrierChanges.push("changed"));

    await session.peer(fakeCarrier());
    expect(carrierChanges).toHaveLength(1);
    await session.peer(fakeCarrier()); // an ICE restart on fresh channels
    expect(carrierChanges).toHaveLength(2);
  });
});

// ---- the rpc through the session --------------------------------------------

describe("what a session's calls carry", () => {
  it("rejects RPC errors from the device", async () => {
    const { session, peer } = await carrying();
    const reply = session.call("run.get", { run_id: "x" });
    await tick();
    peer.reply({ id: replyTo(peer).id, ok: false, error: "no such run" });
    await expect(reply).rejects.toThrow("no such run");
  });

  it("keeps a coded refusal's wire fields on the rejection", async () => {
    const { session, peer } = await carrying();
    const reply = session.call("run.get", { run_id: "x" });
    await tick();
    peer.reply({
      id: replyTo(peer).id,
      ok: false,
      error: "no such run",
      error_code: "not_found",
      details: { run_id: "x" },
    });
    await expect(reply).rejects.toMatchObject({ error_code: "not_found", details: { run_id: "x" } });
  });

  it("takes an options object as the third argument and stamps a background priority", async () => {
    const { session, peer } = await carrying();
    session.call("git.status", { run_id: "x" }, { priority: "background" }).catch(() => {});
    session.call("board.list", {}, 5000).catch(() => {});
    session.call("run.get", {}, { timeoutMs: 5000 }).catch(() => {});
    await tick();
    const payloads = peer.sent.slice(-3).map((envelope) => envelope.frameFields.payload);
    expect(payloads[0].priority).toBe("background");
    expect(payloads[1]).not.toHaveProperty("priority");
    expect(payloads[2]).not.toHaveProperty("priority");
  });

  it("routes calls and their refusals through the installed adapter", async () => {
    const { session, peer } = await carrying();
    expect(session.adapter()).toBe(null);

    const installed = session.installAdapter(selectAdapter({ api_version: "1.1.0" }));
    expect(session.adapter()).toBe(installed);
    expect(installed.capabilities.errors.codes).toBe(true);

    const reply = session.call("run.get", { run_id: "x" });
    await tick();
    expect(replyTo(peer).method).toBe("run.get");
    peer.reply({ id: replyTo(peer).id, ok: false, error: "no such run", error_code: "not_found" });
    const error = await reply.catch((thrown) => thrown);
    expect(error).toBeInstanceOf(ApiError);
    expect(error.code).toBe("not_found");

    // A bridge nobody speaks to leaves the session with no adapter at all.
    expect(session.installAdapter({ unsupported: "app", version: "2.0.0" })).toBe(null);
    expect(session.adapter()).toBe(null);
  });

  it("hands the bridge's unsolicited pushes to onPush, and its replies to nobody else", async () => {
    const { session, peer, events } = await carrying();
    const reply = session.call("ping", {});
    await tick();
    answer(peer, { pong: true });
    await expect(reply).resolves.toEqual({ pong: true });

    peer.reply({ type: "entity.changed", id: "run-7" });
    await tick();
    expect(events.pushes).toEqual([{ type: "entity.changed", id: "run-7" }]);
  });

  it("hands one frame up once, however many wires it has ridden", async () => {
    const { session, peer, events } = await carrying();
    const second = fakeCarrier();
    await session.peer(second);

    second.reply({ type: "board.changed" });
    await tick();
    expect(events.pushes).toEqual([{ type: "board.changed" }]);
  });

  it("fails a call whose envelope never crossed the wire, rather than waiting out its timeout", async () => {
    const { session } = await opened();
    await session.peer(fakeCarrier({ sendFails: "the channel closed" }));

    await expect(session.call("board.list", {}, 60000)).rejects.toThrow(/the channel closed/);
  });

  it("gives workspace detail no default deadline while other calls retain one", async () => {
    vi.useFakeTimers();
    try {
      const { session, peer } = await carrying();
      const workspace = session.call("workspace.get", { workspace_id: "ws-1" });
      const ordinary = session.call("project.list", {});
      workspace.catch(() => {});
      ordinary.catch(() => {});
      await vi.advanceTimersByTimeAsync(DEFAULT_RPC_TIMEOUT_MS + 1);
      await expect(ordinary).rejects.toThrow("project.list timed out");

      const request = peer.sent
        .map((envelope) => envelope.frameFields.payload)
        .find((payload) => payload.method === "workspace.get");
      peer.reply({ id: request.id, ok: true, result: { id: "ws-1" } });
      await expect(workspace).resolves.toEqual({ id: "ws-1" });
      session.close();
    } finally {
      vi.useRealTimers();
    }
  });

  it("times out RPCs that never get a reply", async () => {
    vi.useFakeTimers();
    try {
      const { session } = await carrying();
      const reply = session.call("slow.method", {}, 1000);
      reply.catch(() => {});
      await vi.advanceTimersByTimeAsync(1500);
      await expect(reply).rejects.toThrow("timed out");
      // A reply that never came is not a refusal: the daemon answers a mutation
      // when its own state change is durable, and the work behind it can outlast
      // the timer. The rejection carries which of the two this is, and
      // `replyOrNothing` is the one place that reads it.
      await expect(replyOrNothing(reply)).resolves.toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });

  it("says the timer is the browser's own, at twelve seconds", () => {
    expect(DEFAULT_RPC_TIMEOUT_MS).toBe(12000);
  });
});

// ---- carrying on without a reply ---------------------------------------------
// The one rule every mutation that outlives the timer follows, in the module
// that owns the timer: a reply the browser stopped waiting for is not a refusal.

describe("a reply the browser stopped waiting for", () => {
  const timedOut = (uncertain = true) =>
    Object.assign(new Error("worktree.create timed out"), { timedOut: true, ...(uncertain ? { uncertain: true } : {}) });

  it("answers nothing, so the caller carries on with what the board already has", async () => {
    await expect(replyOrNothing(Promise.reject(timedOut()))).resolves.toBeNull();
  });

  it("raises a timeout that happened before carrier handoff as a definite failure", async () => {
    await expect(replyOrNothing(Promise.reject(timedOut(false)))).rejects.toMatchObject({ timedOut: true });
  });

  it("still raises a refusal, which is the daemon saying no", async () => {
    await expect(replyOrNothing(Promise.reject(new Error("branch exists")))).rejects.toThrow("branch exists");
  });

  it("hands a reply that did arrive straight through", async () => {
    await expect(replyOrNothing(Promise.resolve({ branch: "build/x" }))).resolves.toEqual({ branch: "build/x" });
  });
});
