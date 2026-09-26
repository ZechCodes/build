// What `conversation.settings` answered, written into the cached row — against
// the real cache, its atomic transactions and its watch.
//
// The order these cases replay is the bridge's own: a push's row is built
// under the bridge's lock and sent after it lets go, so a row read before the
// change can reach the device after the answer to it; and the change is noted,
// so a push carrying the new limit follows. No row carries a revision, so the
// answer lands only where nothing has written the row since the verb went out.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { createRailWorkItem } = await import("../src/core/railWorkItem.js");
const { createCompactionChoice } = await import("../src/core/conversationCompaction.js");
const { readCached, subscribeCache, wipeCache, writeCached } = await import("../src/core/localCache.js");

// A second tab: its own instance of the cache module — its own write clock —
// over the same database.
vi.resetModules();
const otherTab = await import("../src/core/localCache.js");

/** Hold the clock both tabs stamp `order` from on one tick, ahead of anything
 *  written so far, so their next writes carry the same order. */
const oneTick = () => vi.spyOn(globalThis.performance, "now").mockReturnValue(performance.now() + 1e9);

/** A record as a page still running the code from before writes were named
 *  puts it: stamped with a time and an order, and no `write`. */
const writeUnnamed = (address, value, order) => new Promise((done, fail) => {
  const opening = indexedDB.open("build-cache");
  opening.onerror = () => fail(opening.error);
  opening.onsuccess = () => {
    const db = opening.result;
    const transaction = db.transaction("records", "readwrite");
    const key = [address.deviceId, address.entityId, address.kind, address.sub].map(encodeURIComponent).join("|");
    transaction.objectStore("records").put({ at: Date.now(), order, value }, key);
    transaction.oncomplete = () => {
      db.close();
      done();
    };
    transaction.onerror = () => fail(transaction.error);
  };
});

const DEVICE = "dev-1";
const ENTITY = "run-7";
const ROW_ADDRESS = { deviceId: DEVICE, entityId: ENTITY, kind: "row", sub: "" };

const cacheScope = {
  deviceId: DEVICE,
  active: () => true,
  address: (parts) => ({ ...parts, deviceId: DEVICE }),
};

const flush = async () => {
  for (let turn = 0; turn < 16; turn += 1) await new Promise((done) => setTimeout(done, 0));
};

/** The row as a push writes it: whole, with the agent's compaction as given. */
const pushRow = (max_context_tokens, compact_at_tokens = max_context_tokens ?? 200000) =>
  writeCached(ROW_ADDRESS, {
    kind: "branch",
    run_id: ENTITY,
    agents: [{ id: "agent-2", max_context_tokens, compact_at_tokens }],
  });

const cachedLimit = async () => (await readCached(ROW_ADDRESS))?.value.agents[0].max_context_tokens;

/** A gate a case opens by hand: the rail's write waits for the menu to finish
 *  shutting, and this is that wait held open. */
const gate = () => {
  let open;
  const opened = new Promise((resolve) => (open = resolve));
  return { opened, open };
};

/** One rail's reading of the row, and its choice wired the way the rail wires
 *  it — `write` waiting on `before` where a case holds it. */
const observer = ({ answer, before = null, kind = "branch" }) => {
  const records = createRailWorkItem({
    context: { kind, deviceId: DEVICE },
    railContext: { kind },
    cacheScope,
    callFor: () => async () => ({}),
    named: () => {},
    standOn: () => {},
    reread: async () => {},
    redrawConversation: () => {},
    alive: () => true,
  });
  const asked = [];
  const choice = createCompactionChoice({
    call: async (method, params) => {
      asked.push(params.max_context_tokens);
      return answer(params);
    },
    capture: (entityId) => records.rowWrite(entityId),
    write: async (captured, agentId, rewrite) => {
      if (before) await before.opened;
      return records.patchAgentIfUnwritten(captured, agentId, rewrite);
    },
  });
  const choose = (maxContextTokens, heldLimit = null) =>
    choice.choose({ entityId: ENTITY, agent: { id: "agent-2", max_context_tokens: heldLimit }, maxContextTokens });
  return { choose, asked };
};

const answering = ({ max_context_tokens }) => ({
  agent_id: "agent-2",
  max_context_tokens,
  compact_at_tokens: max_context_tokens ?? 200000,
});

/** Every value the row record took, in order. */
let rowWrites;
let unwatch;

beforeEach(async () => {
  await wipeCache();
  await pushRow(null);
  rowWrites = [];
  unwatch = subscribeCache(ROW_ADDRESS, async () => rowWrites.push(await cachedLimit()));
});

afterEach(() => unwatch());

describe("the answer in the cached row", () => {
  it("lands on a row nothing has written since the ask", async () => {
    const rail = observer({ answer: answering });

    await rail.choose(0);
    await flush();

    expect(await cachedLimit()).toBe(0);
    expect((await readCached(ROW_ADDRESS)).value.agents[0].compact_at_tokens).toBe(0);
  });

  it("leaves a stale push that lands after it to the push the change itself causes", async () => {
    const rail = observer({ answer: answering });
    await rail.choose(0);
    await flush();

    // The bridge's sequence: the answer, then a row it read before the change
    // (sent after it let go of its lock), then the row its note of the change
    // flushes. Pushes write the row whole; nothing re-writes the answer.
    await pushRow(null);
    await flush();
    await pushRow(0);
    await flush();

    expect(rowWrites).toEqual([0, null, 0]);
    expect(await cachedLimit()).toBe(0);
  });

  it("stands down for a push that lands while the verb is in flight", async () => {
    const rail = observer({
      answer: async (params) => {
        // A row read before the change, sent after it: it lands between the
        // ask and the answer. Written since the ask, so the answer does not
        // land over it — the note's push settles it.
        await pushRow(null);
        return answering(params);
      },
    });

    await rail.choose(0);
    await flush();
    expect(await cachedLimit()).toBe(null);

    await pushRow(0);
    await flush();
    expect(await cachedLimit()).toBe(0);
    expect(rowWrites).toEqual([null, 0]);
  });

  it("keeps a change made elsewhere that lands after the answer and before its write", async () => {
    const shut = gate();
    const rail = observer({ answer: answering, before: shut });

    const choosing = rail.choose(0);
    await flush();
    // Another client sets 150k; its push lands while this rail's menu is
    // still shutting, with no push of the answer ahead of it.
    await pushRow(150000);
    shut.open();
    await choosing;
    await flush();

    expect(await cachedLimit()).toBe(150000);
    expect(rowWrites).toEqual([150000]);
  });

  it("keeps a return to the old value made elsewhere before its write", async () => {
    const shut = gate();
    const rail = observer({ answer: answering, before: shut });

    const choosing = rail.choose(0);
    await flush();
    await pushRow(null);
    shut.open();
    await choosing;
    await flush();

    expect(await cachedLimit()).toBe(null);
  });

  // Two tabs keep their own write clocks, and both can read the same tick: a
  // row another tab wrote since the ask can carry the very order this tab
  // captured. It is still a different write, and the answer stands down.
  it("stands down for another tab's write stamped on the same tick", async () => {
    const tick = oneTick();
    try {
      await pushRow(null);
      const { order } = await readCached(ROW_ADDRESS);
      let stamped;
      const rail = observer({
        answer: async (params) => {
          await otherTab.writeCached(ROW_ADDRESS, {
            kind: "branch",
            run_id: ENTITY,
            agents: [{ id: "agent-2", max_context_tokens: 150000, compact_at_tokens: 150000 }],
          });
          stamped = (await readCached(ROW_ADDRESS)).order;
          return answering(params);
        },
      });

      await rail.choose(0);
      await flush();
      expect(stamped).toBe(order);
    } finally {
      tick.mockRestore();
    }

    expect(await cachedLimit()).toBe(150000);
  });

  // Another tab still on the code from before writes were named: its records
  // carry an order and nothing else, and two of its writes can share a tick.
  // A row captured with no name can never be told unchanged, so the answer
  // is left to the push.
  it("stands down over a row an older page wrote, where another writes the same tick", async () => {
    const ORDER = 42;
    const agents = (max_context_tokens) => [{ id: "agent-2", max_context_tokens, compact_at_tokens: 200000 }];
    await writeUnnamed(ROW_ADDRESS, { kind: "branch", run_id: ENTITY, agents: agents(null) }, ORDER);
    const rail = observer({
      answer: async (params) => {
        await writeUnnamed(ROW_ADDRESS, { kind: "branch", run_id: ENTITY, agents: agents(150000) }, ORDER);
        return answering(params);
      },
    });

    await rail.choose(0);
    await flush();

    expect((await readCached(ROW_ADDRESS)).order).toBe(ORDER);
    expect(await cachedLimit()).toBe(150000);
  });

  it("writes each of two successive choices once, the later last", async () => {
    const rail = observer({ answer: answering });

    await rail.choose(0);
    await flush();
    await rail.choose(150000, 0);
    await flush();

    expect(rail.asked).toEqual([0, 150000]);
    expect(rowWrites).toEqual([0, 150000]);
    expect(await cachedLimit()).toBe(150000);
  });

  it("sends nothing for a second choice while the first is still writing", async () => {
    const shut = gate();
    const rail = observer({ answer: answering, before: shut });

    const first = rail.choose(0);
    await flush();
    await rail.choose(300000);
    shut.open();
    await first;
    await flush();

    expect(rail.asked).toEqual([0]);
    expect(await cachedLimit()).toBe(0);
  });

  // Two rails holding different answers on one cache — two tabs, or the two
  // sides of a swap — each write at most once, and only over the row they
  // asked against: the first to land moves the row, so the second stands
  // down instead of answering the first's write with its own.
  it("lets two observers with different answers write once between them", async () => {
    const shutOff = gate();
    const shutSized = gate();
    const off = observer({ answer: answering, before: shutOff });
    const sized = observer({ answer: answering, before: shutSized });

    const choosingOff = off.choose(0);
    const choosingSized = sized.choose(150000);
    await flush();
    shutOff.open();
    await choosingOff;
    shutSized.open();
    await choosingSized;
    await flush();

    expect(rowWrites).toEqual([0]);

    // The bridge ran both, in some order, and each change's note flushes the
    // row as it stands after both.
    await pushRow(150000);
    await flush();
    expect(rowWrites).toEqual([0, 150000]);
    expect(await cachedLimit()).toBe(150000);
  });
});

// A workspace whose conversation is hidden from the inbox has no row record;
// the rail reads its agents off the board's `runs`, and the answer goes there.
describe("the answer on the board's runs", () => {
  const FEED_ADDRESS = { deviceId: DEVICE, entityId: "", kind: "feed", sub: "" };
  const OTHER = { run_id: "run-9", agents: [{ id: "agent-9", max_context_tokens: null }] };

  const pushFeed = (max_context_tokens) => writeCached(FEED_ADDRESS, {
    items: [],
    runs: [OTHER, { run_id: ENTITY, agents: [{ id: "agent-2", max_context_tokens, compact_at_tokens: 200000 }] }],
  });
  const runsLimit = async () => (await readCached(FEED_ADDRESS))?.value.runs
    .find((run) => run.run_id === ENTITY).agents[0].max_context_tokens;

  beforeEach(async () => {
    await wipeCache();
    await pushFeed(null);
  });

  it("lands on the run nothing has written since the ask, and on no other", async () => {
    const rail = observer({ answer: answering, kind: "workspace" });

    await rail.choose(0);

    expect(await runsLimit()).toBe(0);
    expect((await readCached(FEED_ADDRESS)).value.runs[0].agents[0].max_context_tokens).toBe(null);
  });

  it("stands down where the board was written after the ask", async () => {
    const shut = gate();
    const rail = observer({ answer: answering, kind: "workspace", before: shut });

    const choosing = rail.choose(0);
    await flush();
    await pushFeed(150000);
    shut.open();
    await choosing;

    expect(await runsLimit()).toBe(150000);
  });

  it("stands down for another tab's board stamped on the same tick", async () => {
    const tick = oneTick();
    try {
      await pushFeed(null);
      const { order } = await readCached(FEED_ADDRESS);
      let stamped;
      const rail = observer({
        kind: "workspace",
        answer: async (params) => {
          await otherTab.writeCached(FEED_ADDRESS, {
            items: [],
            runs: [OTHER, { run_id: ENTITY, agents: [{ id: "agent-2", max_context_tokens: 150000, compact_at_tokens: 150000 }] }],
          });
          stamped = (await readCached(FEED_ADDRESS)).order;
          return answering(params);
        },
      });

      await rail.choose(0);
      expect(stamped).toBe(order);
    } finally {
      tick.mockRestore();
    }

    expect(await runsLimit()).toBe(150000);
  });

  it("stands down over a board an older page wrote, where another writes the same tick", async () => {
    const ORDER = 42;
    const board = (max_context_tokens) => ({
      items: [],
      runs: [OTHER, { run_id: ENTITY, agents: [{ id: "agent-2", max_context_tokens, compact_at_tokens: 200000 }] }],
    });
    await writeUnnamed(FEED_ADDRESS, board(null), ORDER);
    const rail = observer({
      kind: "workspace",
      answer: async (params) => {
        await writeUnnamed(FEED_ADDRESS, board(150000), ORDER);
        return answering(params);
      },
    });

    await rail.choose(0);

    expect((await readCached(FEED_ADDRESS)).order).toBe(ORDER);
    expect(await runsLimit()).toBe(150000);
  });
});
