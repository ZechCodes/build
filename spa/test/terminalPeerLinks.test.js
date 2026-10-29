import { afterEach, describe, expect, it, vi } from "vitest";
import { clearTerminalPeerLinks, configureTerminalPeerLinks, releaseTerminalPeerLink, terminalPeerLink } from "../src/core/terminalPeerLinks.js";

const deferred = () => { let resolve; let reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { promise, resolve, reject }; };
const flush = async () => { for (let n = 0; n < 12; n += 1) await Promise.resolve(); };

function fixture() {
  const context = { deviceId: "device", session: { transport: "http", close: vi.fn() }, peerLink: null };
  const listeners = new Set();
  const release = vi.fn();
  const minted = { sessionId: "terminal", deviceId: "device", sessionKeyB64: "key", release };
  const bound = { id: "bound" };
  const helper = {
    term: null, bridgeEpoch: {}, recovery: {}, mintTerminal: vi.fn(async () => minted),
    bindTerminal: vi.fn(async () => { helper.term = bound; return bound; }),
    close: vi.fn(), onClose: (fn) => { listeners.add(fn); return () => listeners.delete(fn); },
  };
  let current = true;
  const options = { open: vi.fn(async () => helper), isCurrent: vi.fn(() => current), onChanged: vi.fn() };
  configureTerminalPeerLinks(options);
  return { context, helper, options, minted, bound, release, listeners, stale: () => { current = false; } };
}

afterEach(() => clearTerminalPeerLinks());

describe("terminal peer link registry", () => {
  it("returns normal RTC peers without creating any HTTP terminal owner", () => {
    const f = fixture(); const peer = { term: {} };
    expect(terminalPeerLink({ ...f.context, session: {}, peerLink: peer })).toBe(peer);
    expect(terminalPeerLink(null)).toBe(null);
    expect(f.options.open).not.toHaveBeenCalled();
  });

  it("keeps one lazy owner per exact HTTP session and binds before exposing its minted carrier", async () => {
    const f = fixture(); const peer = terminalPeerLink(f.context);
    expect(terminalPeerLink(f.context)).toBe(peer);
    expect(peer.term).toBe(null); expect(peer.recovery).toBe(null);
    expect(f.options.open).not.toHaveBeenCalled();
    const minted = await peer.mintTerminal();
    expect(f.options.open).toHaveBeenCalledWith({ deviceId: "device", signal: expect.any(AbortSignal) });
    expect(minted).toMatchObject({ peerLink: peer, carrier: f.bound, bridgeEpoch: f.helper.bridgeEpoch, release: f.release });
    expect(peer.term).toBe(f.bound); expect(peer.recovery).toBe(f.helper.recovery);
    expect(f.helper.bindTerminal).toHaveBeenCalledWith(f.minted);
    expect(f.context.peerLink).toBe(null); expect(f.context.session.close).not.toHaveBeenCalled();
    expect(f.options.onChanged).toHaveBeenCalledWith(f.context);
  });

  it("single-flights the hidden parent while concurrent callers await open", async () => {
    const f = fixture(); const opened = deferred(); f.options.open.mockReturnValueOnce(opened.promise);
    const peer = terminalPeerLink(f.context);
    const one = peer.mintTerminal(); const two = peer.mintTerminal();
    await flush(); expect(f.options.open).toHaveBeenCalledOnce();
    opened.resolve(f.helper); await Promise.all([one, two]);
    expect(f.options.open).toHaveBeenCalledOnce();
  });

  it("closes only the retired session's helper and permits a successor session", async () => {
    const f = fixture(); const oldSession = f.context.session;
    const oldPeer = terminalPeerLink(f.context); await oldPeer.mintTerminal();
    releaseTerminalPeerLink(oldSession);
    expect(f.helper.close).toHaveBeenCalledOnce();
    expect(terminalPeerLink(f.context)).toBe(null);
    f.context.session = { transport: "http", close: vi.fn() };
    const next = terminalPeerLink(f.context);
    expect(next).not.toBe(oldPeer);
    await expect(oldPeer.mintTerminal()).rejects.toThrow();
    expect(f.context.session.close).not.toHaveBeenCalled();
  });

  it("aborts a lazy open promptly and closes its late helper", async () => {
    const f = fixture(); const opening = deferred(); f.options.open.mockReturnValueOnce(opening.promise);
    const peer = terminalPeerLink(f.context); const minted = peer.mintTerminal();
    await flush(); const signal = f.options.open.mock.calls[0][0].signal;
    releaseTerminalPeerLink(f.context.session);
    expect(signal.aborted).toBe(true);
    await expect(minted).rejects.toThrow();
    opening.resolve(f.helper); await flush();
    expect(f.helper.close).toHaveBeenCalledOnce();
    expect(f.helper.mintTerminal).not.toHaveBeenCalled();
  });

  it("rejects a helper opened after its exact context session was replaced", async () => {
    const f = fixture(); const opening = deferred(); f.options.open.mockReturnValueOnce(opening.promise);
    const peer = terminalPeerLink(f.context); const minted = peer.mintTerminal();
    await flush(); f.context.session = { transport: "http" }; opening.resolve(f.helper);
    await expect(minted).rejects.toThrow();
    expect(f.helper.close).toHaveBeenCalledOnce();
  });

  it("releases a late mint without binding it after the owner becomes stale", async () => {
    const f = fixture(); const mint = deferred(); f.helper.mintTerminal.mockReturnValueOnce(mint.promise);
    const peer = terminalPeerLink(f.context); const result = peer.mintTerminal();
    await flush(); f.stale(); mint.resolve(f.minted);
    await expect(result).rejects.toThrow();
    expect(f.release).toHaveBeenCalledOnce();
    expect(f.helper.bindTerminal).not.toHaveBeenCalled();
    expect(f.helper.close).toHaveBeenCalledOnce();
  });

  it("keeps the HTTP app available after terminal failure and retries on the same lazy owner", async () => {
    const f = fixture(); f.options.open.mockRejectedValueOnce(new Error("no RTC path"));
    const peer = terminalPeerLink(f.context);
    await expect(peer.mintTerminal()).rejects.toThrow("no RTC path");
    expect(f.context.session.close).not.toHaveBeenCalled();
    expect(terminalPeerLink(f.context)).toBe(peer);
    await peer.mintTerminal(); expect(f.options.open).toHaveBeenCalledTimes(2);
  });

  it("forgets only a lost helper and fences its stale close callback against replacement", async () => {
    const f = fixture(); const peer = terminalPeerLink(f.context);
    await peer.mintTerminal(); const oldClose = [...f.listeners][0]; oldClose();
    expect(peer.term).toBe(null); expect(peer.recovery).toBe(null);
    const next = { ...f.helper, term: null, close: vi.fn(), onClose: () => () => {} };
    f.options.open.mockResolvedValueOnce(next); await peer.mintTerminal();
    oldClose();
    expect(peer.recovery).toBe(next.recovery); expect(next.close).not.toHaveBeenCalled();
  });

  it("releases a refused terminal binding and preserves its healthy parent", async () => {
    const f = fixture(); f.helper.bindTerminal.mockRejectedValueOnce(new Error("bad proof"));
    const peer = terminalPeerLink(f.context);
    await expect(peer.mintTerminal()).rejects.toThrow("bad proof");
    expect(f.release).toHaveBeenCalledOnce(); expect(f.helper.close).not.toHaveBeenCalled();
    expect(f.context.session.close).not.toHaveBeenCalled();
  });

  it("clears all owned helpers without closing app sessions", async () => {
    const f = fixture(); await terminalPeerLink(f.context).mintTerminal();
    clearTerminalPeerLinks(); clearTerminalPeerLinks();
    expect(f.helper.close).toHaveBeenCalledOnce(); expect(f.context.session.close).not.toHaveBeenCalled();
    expect(terminalPeerLink(f.context)).toBe(null);
  });

  it("releases the exact owner when the HTTP session reports close or expiry", async () => {
    const f = fixture(); let close;
    const unsubscribe = vi.fn(); f.context.session.onClose = (fn) => { close = fn; return unsubscribe; };
    await terminalPeerLink(f.context).mintTerminal(); close();
    expect(f.helper.close).toHaveBeenCalledOnce(); expect(unsubscribe).toHaveBeenCalledOnce();
    expect(terminalPeerLink(f.context)).toBe(null);
  });

  it("does not create a lazy owner for an HTTP session already closed", () => {
    const f = fixture(); const unsubscribe = vi.fn();
    f.context.session.onClose = (fn) => { fn(); return unsubscribe; };
    expect(terminalPeerLink(f.context)).toBe(null);
    expect(f.options.open).not.toHaveBeenCalled(); expect(unsubscribe).toHaveBeenCalledOnce();
  });
});
