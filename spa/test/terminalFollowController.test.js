import { describe, expect, it, vi } from "vitest";

import { createTerminalFollowController } from "../src/terminal/followController.js";

const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};

const settle = async () => {
  for (let index = 0; index < 6; index += 1) await Promise.resolve();
};

const target = (overrides = {}) => ({
  deviceId: "dev-a",
  context: {},
  carrier: { id: "term-a" },
  canAnswer: true,
  freshSession: false,
  ...overrides,
});

function harness(options = {}) {
  const minted = deferred();
  const adopted = deferred();
  const desired = [];
  const effects = {
    mint: vi.fn(() => minted.promise),
    adopt: vi.fn(() => adopted.promise),
    ride: vi.fn(),
    detach: vi.fn(),
    isDesired: vi.fn((candidate) => desired.some((value) => value.deviceId === candidate.deviceId
      && value.context === candidate.context && value.carrier === candidate.carrier)),
    ...options,
  };
  const controller = createTerminalFollowController(effects);
  return { controller, effects, minted, adopted, desire(value) { desired.push(value); return value; } };
}

describe("terminal follow controller", () => {
  it("owns one pending transition and promotes its complete identity only after confirmation", async () => {
    const h = harness();
    const request = h.desire(target());

    expect(h.controller.request(request)).toBe(true);
    expect(h.controller.snapshot()).toMatchObject({ phase: "minting", deviceId: "dev-a", sessionId: null });
    await Promise.resolve();
    const session = { sessionId: "terminal-a", sessionKeyB64: "secret", deviceId: "dev-a", release: vi.fn() };
    h.minted.resolve(session);
    await settle();
    expect(h.controller.snapshot()).toMatchObject({ phase: "confirming", deviceId: "dev-a", sessionId: "terminal-a" });
    expect(JSON.stringify(h.controller.snapshot())).not.toContain("secret");

    h.adopted.resolve();
    await settle();

    expect(h.controller.snapshot()).toEqual({ phase: "confirmed", deviceId: "dev-a", sessionId: "terminal-a", carrier: request.carrier });
    expect(session.release).toHaveBeenCalledOnce();
  });

  it("rejects a late mint after reset even when the replacement account has the same device id", async () => {
    const h = harness();
    const oldContext = {};
    const oldRequest = h.desire(target({ context: oldContext }));
    h.controller.request(oldRequest);
    await Promise.resolve();

    h.controller.reset();
    const replacement = h.desire(target({ context: {}, carrier: { id: "replacement" } }));
    h.controller.request(replacement);
    const stale = { sessionId: "stale", sessionKeyB64: "secret", deviceId: "dev-a", release: vi.fn() };
    h.minted.resolve(stale);
    await settle();

    expect(h.effects.adopt).not.toHaveBeenCalledWith(stale, oldRequest.carrier);
    expect(stale.release).toHaveBeenCalledOnce();
  });

  it("superseding confirmation detaches only the terminal session and stale acknowledgement cannot commit", async () => {
    const mintOne = deferred();
    const mintTwo = deferred();
    const confirmOne = deferred();
    const effects = {
      mint: vi.fn().mockReturnValueOnce(mintOne.promise).mockReturnValueOnce(mintTwo.promise),
      adopt: vi.fn().mockReturnValueOnce(confirmOne.promise).mockResolvedValueOnce(),
      ride: vi.fn(), detach: vi.fn(), isDesired: vi.fn(() => true),
    };
    const controller = createTerminalFollowController(effects);
    const first = target();
    controller.request(first);
    await Promise.resolve();
    const oldSession = { sessionId: "old", deviceId: "dev-a", release: vi.fn() };
    mintOne.resolve(oldSession);
    await Promise.resolve();
    await Promise.resolve();

    const replacement = target({ context: {}, carrier: { id: "new" }, freshSession: true });
    controller.request(replacement);
    expect(effects.detach).toHaveBeenCalled();
    expect(oldSession.release).toHaveBeenCalledOnce();

    confirmOne.resolve();
    await Promise.resolve();
    await Promise.resolve();
    expect(controller.snapshot()).not.toMatchObject({ phase: "confirmed", sessionId: "old" });
  });

  it("leaves a null-carrier session unconfirmed and releases its lease immediately", async () => {
    const first = { sessionId: "waiting", sessionKeyB64: "key", deviceId: "dev-a", release: vi.fn() };
    const second = { sessionId: "connected", sessionKeyB64: "key-2", deviceId: "dev-a", release: vi.fn() };
    const effects = {
      mint: vi.fn().mockResolvedValueOnce(first).mockResolvedValueOnce(second),
      adopt: vi.fn().mockResolvedValue(), ride: vi.fn(), detach: vi.fn(), isDesired: vi.fn(() => true),
    };
    const controller = createTerminalFollowController(effects);
    const context = {};
    controller.request(target({ context, carrier: null }));
    await settle();

    expect(controller.snapshot()).toMatchObject({ phase: "awaiting-carrier", sessionId: "waiting", carrier: null });
    expect(first.release).toHaveBeenCalledOnce();
    controller.request(target({ context, carrier: null }));
    await settle();
    expect(effects.mint).toHaveBeenCalledOnce();

    const carrier = { id: "ready" };
    controller.request(target({ context, carrier }));
    await settle();

    expect(effects.adopt).toHaveBeenCalledWith(second, carrier, expect.any(Function));
    expect(controller.snapshot()).toMatchObject({ phase: "confirmed", sessionId: "connected", carrier });
  });
});
