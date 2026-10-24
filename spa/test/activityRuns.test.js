// The activity behind a folded run: which runs are open, and what each one
// holds.
//
// A run older than the newest message never changes, so it is fetched once,
// written through to the local cache and never asked for again. Only the tail
// run is live, and the window feeds that one.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let createActivityRuns, ACTIVITY_RECORD_KIND, cache;

const DIGEST = { from_sequence: 120, through_sequence: 870, tool_calls: 300 };

const item = (sequence) => ({ type: "event", data: { sequence, event: "tool_use", summary: `Read a${sequence}.js` } });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ createActivityRuns, ACTIVITY_RECORD_KIND } = await import("../src/core/activityRuns.js"));
});

/** One page per call, scripted oldest-first exactly as `thread.activity`
 *  answers, with every call recorded. */
const runsOver = (pages, calls, over = {}) =>
  createActivityRuns({
    deviceId: "dev-1",
    entityId: "run-3",
    agentId: "ag-1",
    call: async (method, params) => {
      calls.push({ method, params });
      return pages.shift();
    },
    ...over,
  });

const page = (sequences, hasMore = false) => ({
  items: sequences.map(item),
  oldest_sequence: sequences.length ? sequences[0] : null,
  has_more: hasMore,
});

describe("the runs a pane has open", () => {
  it("holds none open, and nothing fetched, before anything is pressed", () => {
    const runs = runsOver([], []);

    expect([...runs.openKeys()]).toEqual([]);
    expect(runs.isOpen("120")).toBe(false);
    expect(runs.itemsOf(120)).toBeUndefined();
  });

  it("opens and shuts a run on the key it is pressed with", () => {
    const runs = runsOver([], []);

    expect(runs.toggle("120")).toBe(true);
    expect(runs.isOpen("120")).toBe(true);
    expect([...runs.openKeys()]).toEqual(["120"]);
    expect(runs.toggle("120")).toBe(false);
    expect(runs.isOpen("120")).toBe(false);
  });
});

describe("fetching what a run holds", () => {
  it("refuses a held old generation body even while its offscreen reader remains live", async () => {
    const threadAddress = { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "canonical" };
    await cache.writeCached(threadAddress, { thread_id: "old-thread", items: [] });
    let answer;
    let asked = false;
    const runs = runsOver([], [], { threadId: "old-thread", conversationId: "canonical", call: async () => {
      asked = true; return new Promise((resolve) => { answer = resolve; });
    } });
    const opening = runs.open(DIGEST);
    await vi.waitFor(() => expect(asked).toBe(true));
    await cache.writeCached(threadAddress, { thread_id: "fresh-thread", items: [] });
    answer(page([121, 122]));
    await opening;
    expect(await cache.readCached({ ...threadAddress, kind: "activity", sub: "ag-1:old-thread:120" })).toBeUndefined();
    expect(runs.itemsOf(120)).toBeUndefined();
    runs.dispose();
  });

  it("asks thread.activity over the digest's span, and answers the items", async () => {
    const calls = [];
    const runs = runsOver([page([121, 122])], calls);

    expect(await runs.open(DIGEST)).toBe(true);

    expect(calls).toEqual([{
      method: "thread.activity",
      params: {
        entity_id: "run-3",
        agent_id: "ag-1",
        from_sequence: 120,
        through_sequence: 870,
        limit: 200,
      },
    }]);
    expect(runs.itemsOf(120).map((held) => held.data.sequence)).toEqual([121, 122]);
  });

  // A fold names its run by the oldest sequence the WINDOW holds of it, which
  // is where the run starts only when the page did not cut one. That key is the
  // only one this module is spoken to in: a pane translating between two key
  // spaces gets it wrong silently, since a wrong key is an empty answer.
  it("answers a run's items by the key its fold is pressed with", async () => {
    const runs = runsOver([page([121, 122])], []);

    await runs.open(DIGEST);

    expect(runs.itemsOf(500).map((held) => held.data.sequence)).toEqual([121, 122]);
    expect(runs.itemsOf(870).map((held) => held.data.sequence)).toEqual([121, 122]);
  });

  it("answers nothing for a key no run it holds covers", async () => {
    const runs = runsOver([page([121, 122])], []);

    await runs.open(DIGEST);

    expect(runs.itemsOf(2000)).toBeUndefined();
    expect(runs.itemsOf(119)).toBeUndefined();
  });

  it("pages back until the span is answered, oldest first", async () => {
    const calls = [];
    const runs = runsOver([page([500, 501], true), page([300, 301], false)], calls);

    await runs.open(DIGEST);

    expect(calls[1].params.before_sequence).toBe(500);
    expect(runs.itemsOf(120).map((held) => held.data.sequence)).toEqual([300, 301, 500, 501]);
  });

  it("names no agent when the conversation is the entity's own", async () => {
    const calls = [];
    const runs = runsOver([page([121])], calls, { agentId: "" });

    await runs.open(DIGEST);

    expect(calls[0].params.agent_id).toBeUndefined();
  });

  it("writes a run through to the local cache, under the sequence it starts at", async () => {
    const runs = runsOver([page([121, 122])], []);

    await runs.open(DIGEST);

    const record = await cache.readCached({
      deviceId: "dev-1",
      entityId: "run-3",
      kind: ACTIVITY_RECORD_KIND,
      sub: "ag-1:120",
    });
    expect(record.value.items.map((held) => held.data.sequence)).toEqual([121, 122]);
  });

  it("takes a run the local cache holds instead of the wire", async () => {
    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: ACTIVITY_RECORD_KIND, sub: "ag-1:120" },
      { items: [item(121)] },
    );
    const calls = [];
    const runs = runsOver([], calls);

    expect(await runs.open(DIGEST)).toBe(true);

    expect(calls).toEqual([]);
    expect(runs.itemsOf(120)).toHaveLength(1);
  });

  it("re-reads a run cache announcement and tells its view to redraw", async () => {
    const paints = [];
    let runs;
    runs = runsOver([page([121])], [], {
      onChange: () => paints.push((runs.itemsOf(120) || []).map((held) => held.data.sequence)),
    });
    await runs.open(DIGEST);

    await cache.writeCached(
      { deviceId: "dev-1", entityId: "run-3", kind: ACTIVITY_RECORD_KIND, sub: "ag-1:120" },
      { items: [item(122)] },
    );
    await vi.waitFor(() => expect(paints).toContainEqual([122]));

    expect(runs.itemsOf(120).map((held) => held.data.sequence)).toEqual([122]);
    runs.dispose();
  });

  it("never asks for a run it already holds", async () => {
    const calls = [];
    const runs = runsOver([page([121])], calls);

    await runs.open(DIGEST);
    expect(await runs.open(DIGEST)).toBe(false);

    expect(calls).toHaveLength(1);
  });

  it("asks once for a run pressed twice before the answer lands", async () => {
    const calls = [];
    let answer = null;
    let wireWasAsked = null;
    const asked = new Promise((resolve) => {
      wireWasAsked = resolve;
    });
    const runs = createActivityRuns({
      deviceId: "dev-1",
      entityId: "run-3",
      agentId: "ag-1",
      call: async (method, params) => {
        calls.push({ method, params });
        const held = new Promise((resolve) => {
          answer = resolve;
        });
        wireWasAsked();
        return held;
      },
    });

    const first = runs.open(DIGEST);
    expect(await runs.open(DIGEST)).toBe(false);
    await asked; // the local cache is asked before the wire is

    answer(page([121]));
    expect(await first).toBe(true);
    expect(calls).toHaveLength(1);
  });

  it("asks for nothing at all where there is no entity to ask about", async () => {
    const calls = [];
    const runs = runsOver([], calls, { entityId: "" });

    expect(await runs.open(DIGEST)).toBe(false);
    expect(calls).toEqual([]);
  });

  it("hands a refusal back to the pane that pressed", async () => {
    const runs = createActivityRuns({
      deviceId: "dev-1",
      entityId: "run-3",
      agentId: "ag-1",
      call: async () => {
        throw new Error("unknown method: thread.activity");
      },
    });

    await expect(runs.open(DIGEST)).rejects.toThrow("unknown method");
    expect(runs.itemsOf(120)).toBeUndefined();
  });

  it("stops paging on an answer that holds nothing, rather than asking forever", async () => {
    const calls = [];
    const runs = runsOver([page([], true)], calls);

    await runs.open(DIGEST);

    expect(calls).toHaveLength(1);
    expect(runs.itemsOf(120)).toEqual([]);
  });
});

describe("activity history retention", () => {
  const threadAddress = { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "canonical" };
  const bodyAddress = { ...threadAddress, kind: "activity", sub: "ag-1:thread:120" };
  const scopedRuns = (pages) => runsOver(pages, [], { threadId: "thread", conversationId: "canonical" });

  it("filters hydrated children and trims their stored body when the parent floor advances", async () => {
    await cache.writeCached(threadAddress, { thread_id: "thread", items: [], historyStartSequence: 1 });
    await cache.writeCached(bodyAddress, { items: [item(121), item(800)] });
    const runs = scopedRuns([]);
    await runs.open(DIGEST);
    expect(runs.itemsOf(120).map((one) => one.data.sequence)).toEqual([121, 800]);
    await cache.writeCached(threadAddress, { thread_id: "thread", items: [], historyStartSequence: 700 });
    await vi.waitFor(() => expect(runs.itemsOf(120).map((one) => one.data.sequence)).toEqual([800]));
    await vi.waitFor(async () => expect((await cache.readCached(bodyAddress)).value.items.map((one) => one.data.sequence)).toEqual([800]));
    runs.dispose();
  });

  it("publishes an activity response floor even when the requested run was completely removed", async () => {
    await cache.writeCached(threadAddress, { thread_id: "thread", items: [item(121)], deliveredSequence: 121 });
    const runs = scopedRuns([{ items: [], history_start_sequence: 900, has_more: false }]);
    await runs.open(DIGEST);
    expect((await cache.readCached(threadAddress)).value.historyStartSequence).toBe(900);
    expect((await cache.readCached(threadAddress)).value.items).toEqual([]);
    expect(runs.itemsOf(120)).toEqual([]);
    runs.dispose();
  });

  it("hides an old generation's hydrated body after reset without applying its response floor", async () => {
    await cache.writeCached(threadAddress, { thread_id: "thread", items: [] });
    await cache.writeCached(bodyAddress, { items: [item(121)] });
    const runs = scopedRuns([]);
    await runs.open(DIGEST);
    await cache.writeCached(threadAddress, { thread_id: "replacement", historyStartSequence: 1, items: [] });
    await vi.waitFor(() => expect(runs.itemsOf(120)).toEqual([]));
    expect((await cache.readCached(threadAddress)).value.historyStartSequence).toBe(1);
    runs.dispose();
  });
});

it("fences a stale activity response against a concurrently advanced parent floor", async () => {
  const at = { deviceId: "dev-1", entityId: "run-3", kind: "thread", sub: "canonical" };
  await cache.writeCached(at, { thread_id: "thread", items: [], historyStartSequence: 1 });
  let answer;
  const runs = runsOver([], [], { threadId: "thread", conversationId: "canonical", call: () => new Promise((resolve) => { answer = resolve; }) });
  const opening = runs.open(DIGEST);
  await vi.waitFor(() => expect(answer).toBeTypeOf("function"));
  await cache.writeCached(at, { thread_id: "thread", items: [], historyStartSequence: 900 });
  answer(page([121, 800]));
  await opening;
  expect(runs.itemsOf(120) || []).toEqual([]);
  const body = await cache.readCached({ ...at, kind: "activity", sub: "ag-1:thread:120" });
  expect(body?.value.items || []).toEqual([]);
  runs.dispose();
});
