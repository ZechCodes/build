import { describe, expect, it, vi } from "vitest";
import { createTerminalFollowController } from "../src/terminal/followController.js";

const settle = async () => {
  for (let step = 0; step < 8; step += 1) await Promise.resolve();
};

function harness() {
  let wanted = true;
  let acknowledge;
  const target = { deviceId: "dev-a", context: {}, carrier: {}, canAnswer: true };
  const session = { deviceId: "dev-a", sessionId: "terminal-a", release: vi.fn() };
  const effects = {
    mint: vi.fn(async () => session),
    adopt: vi.fn(() => new Promise((resolve) => { acknowledge = resolve; })),
    ride: vi.fn(),
    detach: vi.fn(),
    isDesired: () => wanted,
  };
  return {
    controller: createTerminalFollowController(effects), target, session, effects,
    want: (value) => { wanted = value; },
    acknowledge: () => acknowledge(),
  };
}

describe("terminal follow authority at asynchronous boundaries", () => {
  it("still starts one mint when detachment synchronously repeats the same request", async () => {
    const h = harness();
    const fresh = { ...h.target, freshSession: true };
    h.effects.detach.mockImplementationOnce(() => h.controller.request(fresh));
    h.controller.request(fresh);
    await settle();

    expect(h.effects.mint).toHaveBeenCalledOnce();
    h.acknowledge();
    await settle();
    h.controller.reset();
  });

  it("cancels confirmation immediately when the same device loses its carrier", async () => {
    const h = harness();
    h.controller.request(h.target);
    await settle();
    h.want(false);
    h.controller.request({ ...h.target, carrier: null, canAnswer: false });

    expect(h.session.release).toHaveBeenCalledOnce();
    h.acknowledge();
    await settle();
    h.controller.reset();
  });

  it("cancels unconfirmed work when the newly selected device is unavailable", async () => {
    const h = harness();
    h.controller.request(h.target);
    await settle();
    h.want(false);
    h.controller.request({ deviceId: "dev-b", context: {}, carrier: null, canAnswer: false });

    expect(h.effects.detach).toHaveBeenCalledOnce();
    expect(h.session.release).toHaveBeenCalledOnce();
    h.acknowledge();
    await settle();
    h.controller.reset();
  });

  it("allows a new request after the desired target changes before minting starts", async () => {
    const h = harness();
    h.controller.request(h.target);
    h.want(false);
    await settle();
    expect(h.effects.mint).not.toHaveBeenCalled();

    h.want(true);
    h.controller.request(h.target);
    await settle();
    expect(h.effects.mint).toHaveBeenCalledOnce();
    h.acknowledge();
    await settle();
    h.controller.reset();
  });

  it("detaches stale confirmation and permits a retry without retaining pending authority", async () => {
    const h = harness();
    h.controller.request(h.target);
    await settle();
    h.want(false);
    h.acknowledge();
    await settle();

    expect(h.effects.detach).toHaveBeenCalledOnce();
    expect(h.session.release).toHaveBeenCalledOnce();
    const replacement = { deviceId: "dev-a", sessionId: "terminal-new", release: vi.fn() };
    h.effects.mint.mockResolvedValue(replacement);
    h.want(true);
    h.controller.request(h.target);
    await settle();
    expect(h.effects.mint).toHaveBeenCalledTimes(2);
    h.acknowledge();
    await settle();
    expect(h.controller.snapshot().sessionId).toBe("terminal-new");
    h.controller.reset();
  });

  it("finishes reset cleanup even when the confirming session's lease release throws", async () => {
    const h = harness();
    h.session.release.mockImplementation(() => { throw new Error("release failed"); });
    h.controller.request(h.target);
    await settle();

    expect(() => h.controller.reset()).not.toThrow();
    expect(h.effects.detach).toHaveBeenCalledOnce();
    h.acknowledge();
    await settle();
    expect(h.session.release).toHaveBeenCalledOnce();
    expect(h.controller.snapshot().sessionId).toBe(null);
  });
});
