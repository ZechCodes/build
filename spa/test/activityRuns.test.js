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
    const runs = createActivityRuns({
      deviceId: "dev-1",
      entityId: "run-3",
      agentId: "ag-1",
      call: async (method, params) => {
        calls.push({ method, params });
        return new Promise((resolve) => {
          answer = resolve;
        });
      },
    });

    const first = runs.open(DIGEST);
    expect(await runs.open(DIGEST)).toBe(false);
    await new Promise((tick) => setTimeout(tick, 0)); // the local cache is asked first

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
