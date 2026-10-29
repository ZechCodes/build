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
    expect(deliver).toHaveBeenCalledWith(candidate);
    for (let i = 0; i < 127; i++) queue.offer({ candidate: `${i}` });
    expect(() => queue.offer({ candidate: "overflow" })).toThrow(/ICE queue/);
    queue.delivered(candidate);
    expect(() => queue.offer({ candidate: "room" })).not.toThrow();
  });
  it("limits bytes, allows one end marker and drops canceled generations", () => {
    const deliver = vi.fn();
    const queue = createPeerIceQueue({ deliver });
    expect(() => queue.offer({ candidate: "x".repeat(65536) })).toThrow(/ICE queue/);
    queue.offer(null);
    queue.offer(null);
    queue.open();
    expect(deliver.mock.calls).toEqual([[null]]);
    queue.reset();
    queue.offer({ candidate: "stale" });
    queue.close();
    queue.open();
    expect(deliver).toHaveBeenCalledTimes(1);
  });
});
