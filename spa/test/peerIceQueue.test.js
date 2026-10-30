import { describe, expect, it, vi } from "vitest";
import { createPeerIceQueue } from "../src/core/peerIceQueue.js";

describe("bounded negotiation ICE queue", () => {
  it("holds candidates until the authenticated answer and accounts for relay holds", () => {
    const deliver = vi.fn();
    const queue = createPeerIceQueue({ deliver });
    const candidate = { candidate: "candidate:1 typ relay" };
    queue.offer(candidate);
    expect(deliver).not.toHaveBeenCalled();
    queue.open();
    expect(deliver).toHaveBeenCalledWith(candidate, expect.any(Function), candidate);
    for (let i = 0; i < 127; i++) queue.offer({ candidate: `${i}` });
    expect(() => queue.offer({ candidate: "overflow" })).toThrow(/ICE queue/);
    deliver.mock.calls[0][1]();
    expect(() => queue.offer({ candidate: "room" })).not.toThrow();
  });
  it("limits bytes, allows one end marker and drops canceled generations", () => {
    const deliver = vi.fn();
    const queue = createPeerIceQueue({ deliver });
    expect(() => queue.offer({ candidate: "x".repeat(65536) })).toThrow(/ICE queue/);
    queue.offer(null);
    queue.offer(null);
    queue.open();
    expect(deliver.mock.calls.map(([value]) => value)).toEqual([null]);
    queue.reset();
    queue.offer({ candidate: "stale" });
    queue.close();
    queue.open();
    expect(deliver).toHaveBeenCalledTimes(1);
  });
});

describe("ICE completion leases", () => {
  it("does not let an old generation release a reused successor object", () => {
    const leases = [];
    const queue = createPeerIceQueue({ deliver: (_value, release) => leases.push(release) });
    const reused = { candidate: "candidate:reused typ host" };
    queue.offer(reused); queue.open(); const oldRelease = leases[0];
    queue.reset(); queue.offer(reused); queue.open();
    oldRelease();
    expect(queue.hasPending()).toBe(true);
    leases[1](); expect(queue.hasPending()).toBe(false);
  });
  it("reserves one normalized marker behind all held and in-flight candidates", () => {
    const calls = [];
    const queue = createPeerIceQueue({ deliver: (value, release) => calls.push({ value, release }) });
    queue.offer({ candidate: "host" }); queue.offer({ candidate: "relay" });
    queue.offer({ candidate: "", usernameFragment: "round-1" }); queue.offer(null); queue.offer({ candidate: "" });
    queue.open(); expect(calls.map((call) => call.value?.candidate)).toEqual(["host", "relay"]);
    calls[1].release(); expect(calls).toHaveLength(2);
    calls[0].release(); expect(calls.map((call) => call.value)).toEqual([{ candidate: "host" }, { candidate: "relay" }, null]);
    calls[2].release(); expect(queue.hasPending()).toBe(false);
  });
});

describe("ICE byte and marker audit regressions", () => {
  it("counts escaped UTF-8 JSON while asynchronous deliveries remain pending", () => {
    const queue = createPeerIceQueue({ deliver() {} }); queue.open();
    const candidate = { candidate: '漢"\\'.repeat(2000) };
    for (let i = 0; i < 4; i++) queue.offer({ ...candidate });
    expect(() => queue.offer({ ...candidate })).toThrow(/ICE queue/);
  });
  it("deduplicates empty and null markers even at the complete object budget", () => {
    const calls = [];
    const queue = createPeerIceQueue({ deliver: (value, release) => calls.push({ value, release }) });
    for (let i = 0; i < 127; i++) queue.offer({ candidate: `${i}` });
    queue.offer(null); queue.open();
    expect(() => queue.offer({ candidate: "" })).not.toThrow();
    expect(() => queue.offer(null)).not.toThrow(); expect(calls).toHaveLength(127);
    queue.reset(); queue.offer({ candidate: "new" }); queue.open();
    calls.slice(0, 127).forEach((call) => call.release());
    expect(queue.hasPending()).toBe(true); expect(calls).toHaveLength(128);
    queue.close(); calls[127].release(); expect(queue.hasPending()).toBe(false);
  });
});

describe("one pre-answer end reservation", () => {
  it.each([false, true])("merges both native marker forms at128 objects (nullFirst=%s)", (nullFirst) => {
    const calls = [];
    const queue = createPeerIceQueue({ deliver: (value, release, original) => calls.push({ value, release, original }) });
    for (let i = 0; i < 127; i++) queue.offer({ candidate: `candidate:${i}` });
    const empty = { candidate: "", sdpMid: "0", usernameFragment: "round-1" };
    expect(() => { queue.offer(nullFirst ? null : empty); queue.offer(nullFirst ? empty : null); }).not.toThrow();
    queue.open(); expect(calls).toHaveLength(127);
    calls.slice().forEach((call) => call.release()); expect(calls).toHaveLength(128);
    expect(calls[127].value).toBeNull(); expect(calls[127].original).toBe(empty);
    calls[127].release(); expect(queue.hasPending()).toBe(false);
  });

  it("charges a preferred empty marker upgrade against the byte budget", () => {
    const queue = createPeerIceQueue({ deliver() {} });
    queue.offer({ candidate: "x".repeat(65480) }); queue.offer(null);
    expect(() => queue.offer({ candidate: "", usernameFragment: "x".repeat(100) })).toThrow(/ICE queue/);
  });
});
