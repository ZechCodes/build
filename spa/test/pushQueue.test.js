import { describe, expect, it } from "vitest";
import { createPushQueue } from "../src/core/pushQueue.js";

const context = { active: () => true };
const tip = (sequence, over = {}) => ({ agent_id: "a", conversation_id: "c", thread_id: "t", since_sequence: sequence - 1,
  last_sequence: sequence, items: [{ type: "message", data: { id: `m${sequence}`, sequence, body: `${sequence}` } }], ...over });
const item = (entity, sequence, over = {}) => ({ entity_id: entity, thread: [tip(sequence)], ...over });
const gate = () => { let release; const done = new Promise((resolve) => { release = resolve; }); return { done, release }; };

describe("bounded push application", () => {
  it("coalesces multi-project bursts while a write stalls and promotes the open conversation", async () => {
    const stalled = gate();
    const applied = [];
    let running = 0, peak = 0;
    const queue = createPushQueue({ priority: (entry) => entry.entity_id === "open", apply: async (_, entry) => {
      running += 1; peak = Math.max(peak, running); applied.push(entry);
      if (applied.length === 1) await stalled.done;
      running -= 1;
    } });
    const done = queue.enqueue(context, [item("p1", 1)]);
    await Promise.resolve();
    for (let n = 2; n <= 100; n += 1) queue.enqueue(context, [item("p1", n), item("p2", n), item("open", n)]);
    expect(queue.snapshot()).toEqual({ running: 1, pending: 3 });
    stalled.release();
    await done;
    expect(peak).toBe(1);
    expect(applied[1].entity_id).toBe("open");
    expect(applied[1].thread[0].items.map((row) => row.data.sequence)).toEqual(Array.from({ length: 99 }, (_, n) => n + 2));
    expect(applied).toHaveLength(4);
  });

  it("retains gaps, resets, generation and removal boundaries in arrival order", async () => {
    const stalled = gate(), applied = [];
    const queue = createPushQueue({ priority: () => true, apply: async (_, entry) => {
      applied.push(entry); if (applied.length === 1) await stalled.done;
    } });
    const done = queue.enqueue(context, [item("a", 1)]);
    await Promise.resolve();
    queue.enqueue(context, [item("a", 2), item("a", 5), item("a", 6, { thread: [tip(6, { reset: true, thread_id: "new" })] }),
      { entity_id: "board", state: { removed: ["a"] } }, item("a", 7)]);
    stalled.release(); await done;
    expect(applied.map((entry) => entry.entity_id)).toEqual(["a", "a", "a", "board", "a"]);
    expect(applied[1].thread.map((entry) => entry.since_sequence)).toEqual([1, 4]);
    expect(applied[2].thread[0].reset).toBe(true);
  });

  it("keeps finished-state and roster generation transitions instead of replacing them", async () => {
    const stalled = gate(), applied = [];
    const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); if (applied.length === 1) await stalled.done; } });
    const state = (thread, status = "building") => ({ entity_id: "a", state: { kind: "branch", state: status, agents: [{ id: "a", thread_id: thread }] } });
    const done = queue.enqueue(context, [state("t")]);
    await Promise.resolve();
    queue.enqueue(context, [state("t"), state("new"), state("new", "archived"), state("new")]);
    stalled.release(); await done;
    expect(applied).toHaveLength(5);
  });

  it("stands down queued work with its captured lifetime", async () => {
    const stalled = gate(), applied = [];
    let active = true;
    const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); await stalled.done; } });
    const done = queue.enqueue({ active: () => active }, [item("a", 1), item("b", 2)]);
    await Promise.resolve();
    active = false; stalled.release(); await done;
    expect(applied).toHaveLength(1);
    expect(queue.snapshot()).toEqual({ running: 0, pending: 0 });
  });
});


it("does not turn capped empty tips into apparent coverage", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); if (applied.length === 1) await stalled.done; } });
  const done = queue.enqueue(context, [item("other", 1)]);
  await Promise.resolve();
  queue.enqueue(context, [item("a", 100, { thread: [tip(100, { since_sequence: 0, items: [] })] }), item("a", 101)]);
  stalled.release(); await done;
  expect(applied[1].thread).toHaveLength(2);
  expect(applied[1].thread[0].items).toEqual([]);
  expect(applied[1].thread[1].since_sequence).toBe(100);
});

it("can run live work after empty and inactive deliveries", async () => {
  const applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); } });
  await queue.enqueue(context, []);
  await queue.enqueue({ active: () => false }, [item("old", 1)]);
  await queue.enqueue(context, [item("new", 2)]);
  expect(applied.map((entry) => entry.entity_id)).toEqual(["new"]);
});


it("coalesces exact source terminal snapshots while retaining source membership boundaries", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); if (applied.length === 1) await stalled.done; } });
  const done = queue.enqueue(context, [item("other", 1)]); await Promise.resolve();
  const source = { project_id: "p", source_id: "s", source_path: "/repo" };
  for (let n = 0; n < 100; n += 1) queue.enqueue(context, [{ project_source: source, terminals: { tabs: [{ term_id: `${n}` }] } }]);
  queue.enqueue(context, [{ project_source: { ...source, source_path: "/moved" }, terminals: { tabs: [] } }]);
  expect(queue.snapshot().pending).toBe(2);
  stalled.release(); await done;
  expect(applied[1].terminals.tabs).toEqual([{ term_id: "99" }]);
  expect(applied[2].project_source.source_path).toBe("/moved");
});


it("retains disjoint out-of-order tip spans without inventing coverage", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); if (applied.length === 1) await stalled.done; } });
  const done = queue.enqueue(context, [item("other", 1)]); await Promise.resolve();
  queue.enqueue(context, [item("a", 101), item("a", 1)]);
  stalled.release(); await done;
  expect(applied[1].thread.map((entry) => [entry.since_sequence, entry.last_sequence])).toEqual([[100, 101], [0, 1]]);
});


it("settles an applied source delivery while unrelated traffic keeps the drain busy", async () => {
  const stalled = gate(), other = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => {
    applied.push(entry.entity_id);
    if (entry.entity_id === "first") await stalled.done;
    if (entry.entity_id === "later") await other.done;
  } });
  const draining = queue.enqueue(context, [item("first", 1)]); await Promise.resolve();
  const delivered = queue.enqueueApplied(context, [item("source", 1)]);
  queue.enqueue(context, [item("later", 1)]);
  stalled.release(); await delivered;
  expect(applied).toContain("source");
  expect(queue.snapshot().running).toBe(1);
  other.release(); await draining;
});


it("bounds repeated empty invalidations without implying that their span was delivered", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); if (applied.length === 1) await stalled.done; } });
  const done = queue.enqueue(context, [item("other", 1)]); await Promise.resolve();
  for (let n = 1; n <= 100; n += 1) queue.enqueue(context, [item("a", n, { thread: [tip(n, { items: [] })] })]);
  stalled.release(); await done;
  expect(applied[1].thread).toHaveLength(1);
  expect(applied[1].thread[0]).toMatchObject({ since_sequence: 0, last_sequence: 100, items: [] });
});

it("uses two enrichment workers without overlapping the same key", async () => {
  const first = gate(), second = gate(), started = [];
  const queue = createPushQueue({ concurrency: 2, apply: async (_, entry) => {
    started.push(entry.entity_id);
    if (started.length === 1) await first.done;
    if (entry.entity_id === "p2") await second.done;
  } });
  const done = queue.enqueue(context, [item("p1", 1), item("p2", 1)]);
  await Promise.resolve();
  queue.enqueue(context, [item("p1", 2), item("p1", 3), item("p3", 1)]);
  expect(started).toEqual(["p1", "p2"]);
  expect(queue.snapshot()).toEqual({ running: 2, pending: 2 });
  first.release();
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(started).toEqual(["p1", "p2", "p1", "p3"]);
  second.release(); await done;
});

it("keeps a removal barrier exclusive when enrichment has two workers", async () => {
  const removal = gate(), started = [];
  const queue = createPushQueue({ concurrency: 2, apply: async (_, entry) => {
    started.push(entry.entity_id);
    if (entry.entity_id === "board") await removal.done;
  } });
  const done = queue.enqueue(context, [{ entity_id: "board", state: { removed: ["p1"] } }, item("p1", 1)]);
  await new Promise((resolve) => setTimeout(resolve, 0));
  expect(started).toEqual(["board"]);
  removal.release(); await done;
  expect(started).toEqual(["board", "p1"]);
});

it("preserves full state before legacy count-only digests without folding unsupported shapes", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => { applied.push(entry); if (applied.length === 1) await stalled.done; } });
  const done = queue.enqueue(context, [item("other", 1)]); await Promise.resolve();
  const state = { kind: "branch", state: "building", agents: [] };
  try {
    queue.enqueue(context, [{ entity_id: "a", state }, { entity_id: "a", state: { state: "building", agents: 2 } },
      { entity_id: "a", state: { state: "building", agents: 0 } }]);
  } finally { stalled.release(); }
  await done;
  expect(applied.slice(1).map((entry) => entry.state)).toEqual([state, { state: "building", agents: 2 }, { state: "building", agents: 0 }]);
});


it("bounds revision-only board demand without dividing mounted work into segments", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ priority: (entry) => entry.entity_id === "open", apply: async (_, entry) => {
    applied.push(entry); if (applied.length === 1) await stalled.done;
  } });
  const done = queue.enqueue(context, [item("other", 1)]); await Promise.resolve();
  for (let n = 1; n <= 1200; n += 1) {
    queue.enqueue(context, [{ entity_id: "board", state: { revision: n } }, item("other", n), item("open", n)]);
  }
  const pending = queue.snapshot().pending;
  stalled.release(); await done;
  expect(pending).toBe(3);
  expect(applied[1].entity_id).toBe("open");
  expect(applied.find((entry) => entry.entity_id === "board").state.revision).toBe(1200);
  expect(applied[1].thread[0].items).toHaveLength(1200);
});

it("keeps board revision high-water and treats unknown board fields as causal barriers", async () => {
  const stalled = gate(), applied = [];
  const queue = createPushQueue({ apply: async (_, entry) => {
    applied.push(entry); if (applied.length === 1) await stalled.done;
  } });
  const done = queue.enqueue(context, [item("other", 1)]); await Promise.resolve();
  queue.enqueue(context, [{ entity_id: "board", state: { revision: 9 } },
    { entity_id: "board", state: { revision: 3 } },
    { entity_id: "board", state: { revision: 10, future_transition: true } },
    { entity_id: "board", state: { revision: 11 } }]);
  stalled.release(); await done;
  expect(applied.slice(1).map((entry) => entry.state)).toEqual([
    { revision: 9 }, { revision: 10, future_transition: true }, { revision: 11 },
  ]);
});
