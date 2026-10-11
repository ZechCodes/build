import { describe, expect, it, vi } from "vitest";
import { createBackgroundStreams, drainBackgroundStreams } from "../src/core/syncBackgroundStreams.js";

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

const generation = () => ({ deviceId: "dev-1", session: {}, requestScope: {}, lifetime: 1 });
const reading = (load, priority = () => ({ priority: "background" })) => ({ active: () => true, priority, load });

describe("shared background wire admission", () => {
  it("releases stale reservations before waiting on a barrier owned by a push continuation", async () => {
    const streams = createBackgroundStreams().forContext(generation());
    const held = gate(), foreground = gate(), passChecks = gate(), changedBarrier = gate();
    const reads = [streams.read(reading(() => held.promise)), streams.read(reading(() => held.promise))];
    let unchanged = true, checked = 0, waiting = 0;
    const passRead = () => streams.read({
      ...reading(() => true),
      before: async () => {
        if (unchanged) {
          if (++checked === 2) passChecks.release();
          return () => unchanged;
        }
        if (++waiting === 2) changedBarrier.release();
        await foreground.promise;
        return () => true;
      },
    });
    reads.push(passRead(), passRead());
    await passChecks.promise;
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    unchanged = false;
    held.release(true);
    await changedBarrier.promise;
    const continuation = vi.fn(() => { foreground.release(); return true; });
    reads.push(streams.read(reading(continuation)));
    try {
      await vi.waitFor(() => expect(continuation).toHaveBeenCalledTimes(1));
    } finally {
      foreground.release();
      await Promise.all(reads);
    }
  });

  it("shares two slots across contexts on one device and keeps another device independent", async () => {
    const streams = createBackgroundStreams(), owner = generation();
    const pass = streams.forContext(owner), push = streams.forContext({ ...owner });
    const other = streams.forContext({ ...owner, deviceId: "dev-2" });
    const one = gate(), two = gate(), third = vi.fn(() => true);
    const reads = [pass.read(reading(() => one.promise)), push.read(reading(() => two.promise)), push.read(reading(third))];
    await other.read(reading(() => true));
    expect(third).not.toHaveBeenCalled();
    one.release(true);
    await vi.waitFor(() => expect(third).toHaveBeenCalledTimes(1));
    two.release(true);
    await expect(Promise.all(reads)).resolves.toEqual([true, true, true]);
  });

  it("promotes queued work before its barrier and gives foreground no background slot", async () => {
    const streams = createBackgroundStreams(), owner = generation(), queue = streams.forContext(owner);
    const held = gate(), foreground = gate();
    const reads = [queue.read(reading(() => held.promise)), queue.read(reading(() => held.promise))];
    let selected = false;
    const load = vi.fn(() => foreground.promise), before = vi.fn(() => Promise.resolve());
    reads.push(queue.read({ ...reading(load, () => selected ? {} : { priority: "background" }), before }));
    await vi.waitFor(() => expect(before).toHaveBeenCalledTimes(1));
    expect(load).not.toHaveBeenCalled();
    selected = true;
    queue.wake();
    await vi.waitFor(() => expect(load).toHaveBeenCalledWith({}));
    expect(before).toHaveBeenCalledTimes(1);
    const next = vi.fn(() => true);
    reads.push(queue.read(reading(next)));
    held.release(true);
    await vi.waitFor(() => expect(next).toHaveBeenCalledTimes(1));
    foreground.release(true);
    await Promise.all(reads);
  });

  it.each(["session", "requestScope", "lifetime"])("replaces %s without inheriting old slots or queued work", async (field) => {
    const streams = createBackgroundStreams(), owner = generation(), old = streams.forContext(owner);
    const abandoned = gate(), current = gate(), stale = vi.fn(), next = vi.fn(() => true);
    const reads = [old.read(reading(() => abandoned.promise)), old.read(reading(() => abandoned.promise))];
    const queued = old.read(reading(stale));
    // Let the old reads acquire both slots before replacing their generation.
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    const fresh = streams.forContext({ ...owner, [field]: field === "lifetime" ? 2 : {} });
    await expect(queued).resolves.toBeNull();
    reads.push(fresh.read(reading(() => current.promise)), fresh.read(reading(() => current.promise)));
    const following = fresh.read(reading(next));
    abandoned.release(true);
    await Promise.all(reads.slice(0, 2));
    expect(stale).not.toHaveBeenCalled();
    expect(next).not.toHaveBeenCalled();
    current.release(true);
    await Promise.all([...reads, following]);
    expect(next).toHaveBeenCalledTimes(1);
  });

  it("reacquires a slot if navigation demotes a read after waking it as foreground", async () => {
    const streams = createBackgroundStreams(), owner = generation(), queue = streams.forContext(owner), held = gate();
    const reads = [queue.read(reading(() => held.promise)), queue.read(reading(() => held.promise))];
    let selected = false;
    const load = vi.fn(() => true);
    reads.push(queue.read(reading(load, () => selected ? {} : { priority: "background" })));
    try {
      for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
      selected = true;
      queue.wake();
      selected = false;
      for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
      expect(load).not.toHaveBeenCalled();
    } finally {
      held.release(true);
      await Promise.all(reads);
    }
    expect(load).toHaveBeenCalledWith({ priority: "background" });
  });

  it("discards queued reads on stop even while both RPCs remain unanswered", async () => {
    const streams = createBackgroundStreams(), queue = streams.forContext(generation()), held = gate();
    const reads = [queue.read(reading(() => held.promise)), queue.read(reading(() => held.promise))];
    const load = vi.fn(), queued = queue.read(reading(load));
    for (let turn = 0; turn < 20; turn += 1) await Promise.resolve();
    streams.clear();
    await expect(queued).resolves.toBeNull();
    expect(load).not.toHaveBeenCalled();
    held.release(true);
    await Promise.all(reads);
  });
});
