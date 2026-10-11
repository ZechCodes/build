import { describe, expect, it, vi } from "vitest";
import { drainBackgroundStreams } from "../src/core/syncBackgroundStreams.js";

const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};

describe("background stream drain", () => {
  it("refills a free stream and retains recovery debt from any checkout", async () => {
    const one = gate(), two = gate();
    const read = vi.fn((item) => item === 1 ? one.promise : item === 2 ? two.promise : true);
    const drain = drainBackgroundStreams({ items: [1, 2, 3, 4], active: () => true, read });
    expect(read.mock.calls).toEqual([[1], [2]]);
    one.release(false);
    await vi.waitFor(() => expect(read.mock.calls).toEqual([[1], [2], [3], [4]]));
    two.release(true);
    await expect(drain).resolves.toBe(false);
  });

  it("abandons queued checkouts when the captured pass becomes inactive", async () => {
    const held = gate();
    let active = true;
    const read = vi.fn(() => held.promise);
    const drain = drainBackgroundStreams({ items: [1, 2, 3], active: () => active, read });
    expect(read.mock.calls).toEqual([[1], [2]]);
    active = false;
    held.release(true);
    await expect(drain).resolves.toBe(false);
    expect(read.mock.calls).toEqual([[1], [2]]);
  });

  it("stops reserving checkouts on failure and waits for the sibling stream before rejecting", async () => {
    const failed = gate(), sibling = gate();
    const error = new Error("cache unavailable");
    const read = vi.fn((item) => item === 1 ? failed.promise.then(() => { throw error; }) : sibling.promise);
    const drain = drainBackgroundStreams({ items: [1, 2, 3], active: () => true, read });
    let settled = false;
    const outcome = drain.then(() => { settled = true; }, (failure) => { settled = true; return failure; });
    failed.release();
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    expect(settled).toBe(false);
    sibling.release(true);
    expect(await outcome).toBe(error);
    expect(read.mock.calls).toEqual([[1], [2]]);
  });
});
