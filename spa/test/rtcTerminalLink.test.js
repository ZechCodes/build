import { describe, expect, it, vi } from "vitest";
import * as transport from "@build/secure-transport";
import { createRendezvousLifecycle } from "../src/core/rendezvousLifecycle.js";
import { createBridgeEpoch } from "../src/core/bridgeEpoch.js";
import { SPA_API_RANGE } from "../src/core/bridgeApi/index.js";
import { openRtcTerminalLink } from "../src/core/rtcTerminalLink.js";

const deferred = () => { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = async () => { for (let n = 0; n < 12; n += 1) await Promise.resolve(); };
const greeting = () => ({ api_version: "3.24.0", capabilities: ["bridge.epoch"], bridge_epoch: "bridge-one", journal_incarnation: "journal-one" });

function fixture() {
  const signaling = deferred();
  const carrierListeners = new Set();
  const app = { onClose: (fn) => { carrierListeners.add(fn); return () => carrierListeners.delete(fn); } };
  const restored = new Set();
  let sessionOptions;
  let peerOptions;
  const rendezvous = { mint: vi.fn(async () => ({ sessionId: "term-one", sessionKeyB64: "term-key", deviceId: "device" })), close: vi.fn() };
  const lifecycle = createRendezvousLifecycle(() => rendezvous);
  const owner = lifecycle.forDevice("device");
  const session = {
    deviceId: "device", sessionId: "parent-one", bridgeEpoch: createBridgeEpoch(), peerBinding: { sessionId: "parent-one" },
    rawCall: vi.fn(async () => greeting()), onPush: vi.fn(), peer: vi.fn(), close: vi.fn(),
    watchRecovery: vi.fn(), confirmCarried: vi.fn(async () => true), reattachSignaling: vi.fn(async () => {}),
    resetSqlReads: vi.fn(), installAdapter: vi.fn(),
    acceptGreeting: (...args) => session.bridgeEpoch.acceptGreeting(...args),
  };
  const link = {
    app, term: { label: "term" }, recovery: { snapshot: () => ({ recovering: false }) }, close: vi.fn(),
    bindTerminal: vi.fn(async () => ({ label: "bound-term" })),
    whenInitialSignalingComplete: () => signaling.promise,
    onRestored: (fn) => { restored.add(fn); return () => restored.delete(fn); },
  };
  const options = {
    deviceId: "device", acquireRendezvous: () => owner.acquire(), transport, fetchIceServers: vi.fn(async () => []), clientId: () => "client",
    openSessionImpl: vi.fn(async (opts) => { sessionOptions = opts; return session; }),
    openPeerLinkImpl: vi.fn(async (opts) => { peerOptions = opts; return link; }),
  };
  return { options, signaling, app, session, link, rendezvous, owner, restored, get sessionOptions() { return sessionOptions; }, get peerOptions() { return peerOptions; } };
}

describe("independent RTC terminal owner", () => {
  it("mints and greets its own app session without adopting any HTTP context", async () => {
    const f = fixture();
    const http = { call: vi.fn(), close: vi.fn(), bridgeEpoch: createBridgeEpoch() };
    const rtc = await openRtcTerminalLink(f.options);
    expect(f.sessionOptions).toMatchObject({ deviceId: "device", rendezvous: f.rendezvous, transport });
    expect(f.peerOptions.binding).toBe(f.session.peerBinding);
    expect(f.peerOptions.signal).toBe(f.session.rawCall);
    expect(f.session.peer).toHaveBeenCalledWith(f.app);
    expect(f.session.rawCall).toHaveBeenCalledWith("session.hello", { api_range: SPA_API_RANGE });
    expect(f.session.installAdapter).toHaveBeenCalledOnce();
    expect(rtc.bridgeEpoch.current().bridge_epoch).toBe("bridge-one");
    expect(rtc.recovery).toBe(f.link.recovery);
    expect(rtc.term).toBe(f.link.term);
    expect(f.rendezvous.close).not.toHaveBeenCalled();
    f.signaling.resolve();
    await flush();
    expect(f.rendezvous.close).toHaveBeenCalledOnce();
    rtc.close();
    expect(http.close).not.toHaveBeenCalled();
    expect(http.bridgeEpoch.current()).toBe(null);
  });

  it("keeps the mint lease until the separately minted terminal binds", async () => {
    const f = fixture();
    const rtc = await openRtcTerminalLink(f.options);
    f.signaling.resolve(); await flush();
    const minted = await rtc.mintTerminal();
    expect(minted).toMatchObject({ sessionId: "term-one", sessionKeyB64: "term-key", bridgeEpoch: rtc.bridgeEpoch, release: expect.any(Function) });
    expect(f.rendezvous.close).toHaveBeenCalledTimes(1);
    const carrier = await rtc.bindTerminal(minted);
    expect(carrier).toEqual({ label: "bound-term" });
    expect(f.link.bindTerminal).toHaveBeenCalledWith(minted);
    expect(f.rendezvous.close).toHaveBeenCalledTimes(2);
    minted.release();
    expect(f.rendezvous.close).toHaveBeenCalledTimes(2);
    rtc.close();
  });

  it("reacquires signaling on restart and releases only after gathering settles", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    f.signaling.resolve(); await flush();
    await f.peerOptions.onFailed();
    expect(f.session.reattachSignaling).toHaveBeenCalledOnce();
    const restarted = deferred();
    f.peerOptions.onConnected(restarted.promise);
    expect(f.rendezvous.close).toHaveBeenCalledTimes(1);
    restarted.resolve(); await flush();
    expect(f.rendezvous.close).toHaveBeenCalledTimes(2);
    rtc.close();
    await expect(f.peerOptions.onFailed()).rejects.toThrow();
    expect(f.session.reattachSignaling).toHaveBeenCalledOnce();
  });

  it("cannot reacquire an invalidated owner generation", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    f.owner.forceClose();
    const replacement = f.owner.acquire();
    await expect(f.peerOptions.onFailed()).rejects.toThrow();
    rtc.close();
    expect(f.owner.state).toBe("negotiating");
    replacement.release();
  });

  it("re-greets its own epoch on restore and closes on re-greeting refusal", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    const ended = vi.fn(); rtc.onClose(ended);
    f.session.rawCall.mockResolvedValueOnce({ ...greeting(), bridge_epoch: "bridge-two" });
    for (const fn of [...f.restored]) fn();
    await flush();
    expect(f.session.resetSqlReads).toHaveBeenCalledOnce();
    expect(rtc.bridgeEpoch.current().bridge_epoch).toBe("bridge-two");
    f.session.rawCall.mockRejectedValueOnce(new Error("no greeting"));
    for (const fn of [...f.restored]) fn();
    await flush();
    expect(ended).toHaveBeenCalledOnce();
    expect(f.link.close).toHaveBeenCalledOnce();
    expect(f.restored.size).toBe(0);
  });

  it.each(["mint", "peer", "hello", "unsupported"])("cleans up its exact resources on %s failure", async (step) => {
    const f = fixture();
    if (step === "mint") f.options.openSessionImpl.mockRejectedValueOnce(new Error("mint failed"));
    if (step === "peer") f.options.openPeerLinkImpl.mockRejectedValueOnce(new Error("peer failed"));
    if (step === "hello") f.session.rawCall.mockRejectedValueOnce(new Error("hello failed"));
    if (step === "unsupported") f.session.rawCall.mockResolvedValueOnce({ api_version: "0.0.0" });
    await expect(openRtcTerminalLink(f.options)).rejects.toThrow();
    expect(f.owner.state).toBe("idle");
    expect(f.rendezvous.close).toHaveBeenCalledOnce();
    expect(f.session.close).toHaveBeenCalledTimes(step === "mint" ? 0 : 1);
    expect(f.link.close).toHaveBeenCalledTimes(["hello", "unsupported"].includes(step) ? 1 : 0);
  });

  it("releases a failed terminal bind without losing its healthy parent", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    f.signaling.resolve(); await flush();
    const minted = await rtc.mintTerminal();
    f.link.bindTerminal.mockRejectedValueOnce(new Error("bad terminal proof"));
    await expect(rtc.bindTerminal(minted)).rejects.toThrow();
    expect(f.session.close).not.toHaveBeenCalled();
    expect(f.link.close).not.toHaveBeenCalled();
    expect(f.rendezvous.close).toHaveBeenCalledTimes(2);
    rtc.close();
  });

  it("releases a failed mint and rejects foreign device terminal sessions", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    f.signaling.resolve(); await flush();
    f.rendezvous.mint.mockRejectedValueOnce(new Error("mint failed"));
    await expect(rtc.mintTerminal()).rejects.toThrow();
    expect(f.owner.state).toBe("idle");
    await expect(rtc.bindTerminal({ sessionId: "foreign", deviceId: "other" })).rejects.toThrow();
    expect(f.link.bindTerminal).not.toHaveBeenCalled();
    rtc.close();
  });

  it("aborts setup promptly and closes sessions that finish minting late", async () => {
    const f = fixture(); const mint = deferred(); const abort = new AbortController();
    f.options.openSessionImpl.mockImplementationOnce(() => mint.promise);
    const opening = openRtcTerminalLink({ ...f.options, signal: abort.signal });
    await flush(); abort.abort(new Error("cancelled"));
    await expect(opening).rejects.toThrow("cancelled");
    expect(f.owner.state).toBe("idle");
    mint.resolve(f.session); await flush();
    expect(f.session.close).toHaveBeenCalledOnce();
    expect(f.options.openPeerLinkImpl).not.toHaveBeenCalled();
  });

  it("closes a peer completing after cancellation and fences late callbacks", async () => {
    const f = fixture(); const peer = deferred(); const abort = new AbortController();
    f.options.openPeerLinkImpl.mockImplementationOnce((opts) => { f.captured = opts; return peer.promise; });
    const opening = openRtcTerminalLink({ ...f.options, signal: abort.signal });
    await flush(); abort.abort();
    await expect(opening).rejects.toThrow();
    peer.resolve(f.link); await flush();
    expect(f.link.close).toHaveBeenCalledOnce();
    expect(f.session.close).toHaveBeenCalledOnce();
    await expect(f.captured.onFailed()).rejects.toThrow();
    f.captured.onConnected(Promise.resolve());
    expect(f.owner.state).toBe("idle");
  });

  it("closes once when its parent reports loss and returns a closed mint refusal", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    const close = vi.fn(); const unsubscribe = rtc.onClose(close); unsubscribe();
    const notified = vi.fn(); rtc.onClose(notified);
    f.sessionOptions.onLost(); rtc.close();
    expect(close).not.toHaveBeenCalled(); expect(notified).toHaveBeenCalledOnce();
    expect(f.link.close).toHaveBeenCalledOnce(); expect(f.session.close).toHaveBeenCalledOnce();
    await expect(rtc.mintTerminal()).rejects.toThrow();
  });

  it("cancels an in-flight terminal mint and releases every owned lease", async () => {
    const f = fixture(); const rtc = await openRtcTerminalLink(f.options);
    const mint = deferred(); f.rendezvous.mint.mockReturnValueOnce(mint.promise);
    const pending = rtc.mintTerminal();
    rtc.close();
    await expect(pending).rejects.toThrow();
    mint.resolve({ sessionId: "late", deviceId: "device" }); await flush();
    expect(f.owner.state).toBe("idle");
    expect(f.link.bindTerminal).not.toHaveBeenCalled();
  });

  it("uses the production session and crypto wrappers for its private greeting", async () => {
    const keys = [];
    const received = [];
    let app;
    const makeCarrier = (minted) => {
      const envelopes = new Set(); const closed = new Set(); let ended = false;
      return {
        onEnvelope(fn) { envelopes.add(fn); return () => envelopes.delete(fn); },
        onClose(fn) { closed.add(fn); return () => closed.delete(fn); },
        close() { if (ended) return; ended = true; for (const fn of [...closed]) fn(); },
        async send(envelope) {
          const frame = await transport.decryptEnvelope({ sessionKeyB64: minted.sessionKeyB64, envelope });
          received.push(frame);
          const response = await transport.encryptFrame({
            sessionKeyB64: minted.sessionKeyB64, outerFields: { session_id: minted.sessionId, route_to: "client" },
            frameFields: { sender: "device", frame_type: "data", payload: { id: frame.payload.id, ok: true, result: greeting(), bridge_epoch: "bridge-one" } },
          });
          for (const fn of [...envelopes]) fn(response);
        },
      };
    };
    const rendezvous = {
      async mint() {
        const minted = { sessionId: `mint-${keys.length}`, sessionKeyB64: await transport.generateSessionKey(), deviceId: "device" };
        keys.push(minted); return minted;
      },
      signalCarrier: () => makeCarrier(keys[0]), close: vi.fn(),
    };
    const owner = createRendezvousLifecycle(() => rendezvous).forDevice("device");
    const rtc = await openRtcTerminalLink({
      deviceId: "device", transport, acquireRendezvous: () => owner.acquire(), fetchIceServers: async () => [],
      openPeerLinkImpl: async () => {
        app = makeCarrier(keys[0]);
        return { app, term: {}, recovery: { snapshot: () => ({ recovering: false }) }, whenInitialSignalingComplete: () => Promise.resolve(), onRestored: () => () => {}, close: () => app.close() };
      },
    });
    expect(received).toMatchObject([{ sender: "client", session_id: "mint-0", payload: { method: "session.hello", params: { api_range: SPA_API_RANGE } } }]);
    expect(rtc.bridgeEpoch.current().bridge_epoch).toBe("bridge-one");
    const terminal = await rtc.mintTerminal();
    expect(terminal.sessionId).toBe("mint-1");
    expect(terminal.sessionKeyB64).not.toBe(keys[0].sessionKeyB64);
    terminal.release(); rtc.close();
    expect(owner.state).toBe("idle");
  });
});
