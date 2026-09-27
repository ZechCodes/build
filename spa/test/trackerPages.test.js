// `tasks.list` a page at a time (#85): each page lands under its own address,
// is read back from there, and is laid over the held list for the stretch of
// numbers it answers — never over a row something newer wrote.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const task = (number, over = {}) => ({
  id: `task-${number}`,
  number,
  title: `task ${number}`,
  updated_at: "2026-09-25T08:00:00Z",
  ...over,
});
const numbers = (tasks) => tasks.map((one) => one.number);
const AT = (text) => Date.parse(text);

let pages, cache, trackerCache;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  trackerCache = await import("../src/core/trackerCache.js");
  pages = await import("../src/core/trackerPages.js");
});

describe("laying a page over a held list", () => {
  const stretch = (tasks, over = {}) => ({ tasks, above: Infinity, through: -Infinity, readAt: null, ...over });

  it("is the page itself over nothing", () => {
    expect(numbers(pages.withTaskPage(null, stretch([task(3), task(2)])))).toEqual([3, 2]);
  });

  it("replaces only the numbers the page answers for", () => {
    const held = [task(9), task(8), task(7), task(6), task(5), task(4)];
    const laid = pages.withTaskPage(held, stretch([task(7, { title: "new" }), task(5)], { above: 8, through: 5 }));
    expect(numbers(laid)).toEqual([9, 8, 7, 5, 4]);
    expect(laid.find((one) => one.number === 7).title).toBe("new");
  });

  it("lets the last page answer for every number under the one before it", () => {
    const held = [task(4), task(3), task(2), task(1)];
    expect(numbers(pages.withTaskPage(held, stretch([task(3)], { above: 4 })))).toEqual([4, 3]);
  });

  it("keeps a held row written after the page's copy of it", () => {
    const newer = task(5, { title: "pushed", updated_at: "2026-09-25T08:05:00Z" });
    const laid = pages.withTaskPage([newer], stretch([task(5, { title: "paged" })]));
    expect(laid).toEqual([newer]);
  });

  it("takes the page's row over a held copy no newer than it", () => {
    const held = [task(5, { title: "held" })];
    expect(pages.withTaskPage(held, stretch([task(5, { title: "paged" })]))[0].title).toBe("paged");
  });

  it("keeps a row the page does not name when it was written after the page was read", () => {
    const filed = task(6, { updated_at: "2026-09-25T08:10:00Z" });
    const gone = task(4, { updated_at: "2026-09-25T07:00:00Z" });
    const laid = pages.withTaskPage([filed, gone], stretch([task(5)], { readAt: AT("2026-09-25T08:09:00Z") }));
    expect(numbers(laid)).toEqual([6, 5]);
  });
});

describe("yielding to a newer read", () => {
  const stretch = (tasks, over = {}) => ({ tasks, above: Infinity, through: -Infinity, readAt: null, read: 1, ...over });

  it("keeps a newer read's copy of a row though its updated_at is the same", () => {
    const newer = task(5, { title: "read later" });
    const laid = pages.withTaskPage([newer], stretch([task(5, { title: "read first" })]), () => 2);
    expect(laid).toEqual([newer]);
  });

  it("leaves out a row a newer read left out, though the cache never held it", async () => {
    const order = await import("../src/core/taskReadOrder.js");
    const older = await order.nextTaskRead();
    const newer = await order.nextTaskRead();
    const reads = order.withStretch(null, { above: 8, through: 4, read: newer });
    const laid = pages.withTaskPage([task(9)], stretch([task(6), task(5)], { above: 7, through: 5, read: older }), order.lastSayIn(reads));
    expect(numbers(laid)).toEqual([9]);
  });

  it("lays a row where only older reads had the say", async () => {
    const order = await import("../src/core/taskReadOrder.js");
    const older = await order.nextTaskRead();
    const newer = await order.nextTaskRead();
    const reads = order.withStretch(null, { above: 8, through: 4, read: older });
    const laid = pages.withTaskPage([], stretch([task(6)], { read: newer }), order.lastSayIn(reads));
    expect(numbers(laid)).toEqual([6]);
  });

  it("forgets every older say once a pull has had the say on every number", async () => {
    const order = await import("../src/core/taskReadOrder.js");
    const stale = await order.nextTaskRead();
    let reads = order.withStretch(null, { above: 8, through: 4, read: stale });
    reads = order.withWritten(reads, ["task-2"], await order.nextTaskRead());
    const first = await order.nextTaskRead();
    reads = order.withStretch(reads, { above: Infinity, through: 5, read: first });
    expect(order.lastSayIn(reads)(task(2))).toBeGreaterThan(stale);
    const last = await order.nextTaskRead();
    reads = order.withStretch(reads, { above: 5, through: -Infinity, read: last, pullRead: first });
    expect(order.lastSayIn(reads)(task(2))).toBe(last);
    expect(order.lastSayIn(reads)(task(6))).toBe(first);
    expect(reads.stretches).toHaveLength(2);
  });

  it("gives a card moved here the say over any read already out", async () => {
    const order = await import("../src/core/taskReadOrder.js");
    const ADDRESS = { deviceId: "dev-1", entityId: "p1", kind: "tracker-tasks" };
    const out = await order.nextTaskRead();
    const moved = task(5, { status: "done" });
    await order.noteWritten([ADDRESS], ["task-5"]);
    const reads = (await cache.readCached(order.readsAddress(ADDRESS)))?.value;
    const laid = pages.withTaskPage([moved], stretch([task(5, { status: "backlog" })], { read: out }), order.lastSayIn(reads));
    expect(laid).toEqual([moved]);
  });

  it("numbers reads in the order they are asked, across tabs sharing the cache", async () => {
    const order = await import("../src/core/taskReadOrder.js");
    const first = await order.nextTaskRead();
    vi.resetModules();
    const otherTab = await import("../src/core/taskReadOrder.js");
    const second = await otherTab.nextTaskRead();
    const third = await order.nextTaskRead();
    expect(first).toBeLessThan(second);
    expect(second).toBeLessThan(third);
  });
});

describe("pulling every page", () => {
  const LIST = Array.from({ length: 7 }, (_, index) => task(7 - index));
  /** A bridge that pages the way #85's does: number descending, and a cursor
   *  that is the last number a page answered. */
  const pagingBridge = (list = LIST) => vi.fn(async (params) => {
    const below = params.cursor ? Number(params.cursor.slice(1)) : Infinity;
    const rest = list.filter((one) => one.number < below);
    const page = rest.slice(0, params.limit);
    const more = rest.length > params.limit;
    return {
      project_id: "p1",
      tasks: page,
      user_session: { gap_ms: 1, now_ms: AT("2026-09-25T08:30:00Z") },
      ...(more ? { next_cursor: `c${page.at(-1).number}` } : {}),
    };
  });

  const pull = (ask, fold, extra = {}) => pages.pullTaskPages({
    ask, deviceId: "dev-1", projectId: "p1", params: { project_id: "p1" }, fold, limit: 3, ...extra,
  });

  it("asks page after page with the cursor each answered, and folds each in order", async () => {
    const ask = pagingBridge();
    const folded = [];
    expect(await pull(ask, async (stretch) => folded.push(stretch))).toBe(true);
    expect(ask.mock.calls.map(([params]) => params)).toEqual([
      { project_id: "p1", limit: 3 },
      { project_id: "p1", limit: 3, cursor: "c5" },
      { project_id: "p1", limit: 3, cursor: "c2" },
    ]);
    expect(folded.map((stretch) => [numbers(stretch.tasks), stretch.above, stretch.through])).toEqual([
      [[7, 6, 5], Infinity, 5],
      [[4, 3, 2], 5, 2],
      [[1], 2, -Infinity],
    ]);
    expect(folded[0].readAt).toBe(AT("2026-09-25T08:30:00Z"));
  });

  it("writes each page under its own address and folds what it reads back from there", async () => {
    const seen = [];
    await pull(pagingBridge(), async (stretch, page) => {
      const address = trackerCache.tasksPageAddress("dev-1", "p1", { project_id: "p1", limit: 3, ...(stretch.above === Infinity ? {} : { cursor: `c${stretch.above}` }), read_order: stretch.read });
      seen.push((await cache.readCached(address))?.value);
      expect(page).toEqual(seen.at(-1));
    });
    expect(seen.map((page) => numbers(page.tasks))).toEqual([[7, 6, 5], [4, 3, 2], [1]]);
  });

  it("forgets the pages an earlier pull of the same list left behind, and no other list's", async () => {
    const stale = trackerCache.tasksPageAddress("dev-1", "p1", { project_id: "p1", limit: 3, cursor: "c6" });
    const narrowed = trackerCache.tasksPageAddress("dev-1", "p1", { project_id: "p1", state: "open", limit: 3 });
    await cache.writeCached(stale, { tasks: [] });
    await cache.writeCached(narrowed, { tasks: [] });
    await pull(pagingBridge(), async () => {});
    expect(await cache.readCached(stale)).toBeUndefined();
    expect(await cache.readCached(narrowed)).toBeTruthy();
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_TASKS_PAGE_KIND)).toHaveLength(4);
  });

  it("keeps a newer read's raw page while an older walk cleans up", async () => {
    let release;
    let ready;
    const asked = new Promise((resolve) => { ready = resolve; });
    const old = pull(() => new Promise((resolve) => {
      release = () => resolve({ tasks: [task(20)] });
      ready();
    }), async () => {});
    await asked;
    const { nextTaskRead } = await import("../src/core/taskReadOrder.js");
    const newerRead = await nextTaskRead();
    const newer = trackerCache.tasksPageAddress("dev-1", "p1", { project_id: "p1", limit: 3, read_order: newerRead });
    await cache.writeCached(newer, { tasks: [task(30)], read_order: newerRead });
    release();
    expect(await old).toBe(true);
    expect((await cache.readCached(newer))?.value?.tasks).toEqual([task(30)]);
  });

  it("stops where the bridge stops answering, keeping what already landed", async () => {
    const ask = pagingBridge();
    ask.mockImplementationOnce(ask.getMockImplementation()).mockResolvedValueOnce(null);
    const folded = [];
    expect(await pull(ask, async (stretch) => folded.push(stretch))).toBe(false);
    expect(folded).toHaveLength(1);
  });

  it("stops when its caller stands down, without writing the answer that came back", async () => {
    let live = true;
    const ask = pagingBridge();
    const fold = vi.fn(async () => { live = false; });
    expect(await pull(ask, fold, { active: () => live })).toBe(false);
    expect(ask).toHaveBeenCalledTimes(1);
    expect(fold).toHaveBeenCalledTimes(1);
  });

  it("walks on past a short or empty page while the bridge names the next", async () => {
    // A label few tasks carry: the bridge reads a bounded stretch per page
    // and answers what it kept there, short or nothing, with where to go on.
    const answers = [
      { tasks: [task(9)], next_cursor: "c6" },
      { tasks: [], next_cursor: "c3" },
      { tasks: [task(2)] },
    ];
    const ask = vi.fn(async () => ({ project_id: "p1", ...answers.shift() }));
    const folded = [];
    expect(await pull(ask, async (stretch) => folded.push(stretch))).toBe(true);
    expect(ask.mock.calls.map(([params]) => params.cursor)).toEqual([undefined, "c6", "c3"]);
    // The empty page answers for no number: the next one starts where the
    // last row kept left off, and lays itself over everything below that.
    expect(folded.map((stretch) => [numbers(stretch.tasks), stretch.above, stretch.through])).toEqual([
      [[9], Infinity, 9],
      [[2], 9, -Infinity],
    ]);
    // Between #9 and #2 the first page, the empty one and the last may each
    // have read some of the numbers. The first page's read is the oldest, so
    // it has the say there; the last page's own read has it only from the
    // row it named down.
    expect(folded[1].spans).toEqual([
      { above: 9, through: 3, read: folded[0].read, readAt: null },
      { above: 3, through: -Infinity, read: folded[1].read, readAt: null },
    ]);
  });

  it("stops rather than loop on a cursor that does not move", async () => {
    const ask = vi.fn(async () => ({ tasks: [task(3)], next_cursor: "same" }));
    await pull(ask, async () => {});
    expect(ask).toHaveBeenCalledTimes(2);
  });

  it("stops if readback lost the page body's read provenance", async () => {
    const pagePrefix = { deviceId: "dev-1", entityId: "p1", kind: trackerCache.TRACKER_TASKS_PAGE_KIND };
    const folded = vi.fn();
    let stop;
    stop = cache.subscribeCache(pagePrefix, (address) => {
      stop();
      void cache.writeCached(address, { tasks: [task(3)], read_order: 0 });
    });
    expect(await pull(async () => ({ tasks: [task(3)] }), folded)).toBe(false);
    expect(folded).not.toHaveBeenCalled();
  });
});

describe("the numbers between one page's rows and the next's", () => {
  const params = { project_id: "p1", label: "bug" };
  const ADDRESS = { deviceId: "dev-1", entityId: "p1", kind: "tracker-tasks-query", sub: JSON.stringify(params) };
  const labelled = task(20, { labels: ["bug"] });
  const answer = (tasks, extra = {}) => ({ project_id: "p1", tasks, ...extra });
  const pull = (ask) => pages.pullTaskPages({
    ask, deviceId: "dev-1", projectId: "p1", params, limit: 1,
    fold: (stretch) => pages.foldTasksPage(ADDRESS, stretch, () => []),
  });
  const held = async () => numbers((await cache.readCached(ADDRESS))?.value?.tasks || []);

  // The walk's first page reads #20 down to #13 and keeps nothing; while its
  // answer is on the way, #20 takes the label and a newer read lays it.
  const scannedPastWhileANewerReadLaysIt = (rest) => {
    let call = 0;
    return async () => {
      if (++call === 1) {
        await pull(async () => answer([labelled]));
        expect(await held()).toEqual([20]);
        return answer([], { next_cursor: "below-13" });
      }
      return rest();
    };
  };

  it("does not let a page after an empty one take off a row it never read", async () => {
    await pull(scannedPastWhileANewerReadLaysIt(() => answer([])));
    expect(await held()).toEqual([20]);
  });

  it("does not let a page after an empty one take it off for the rows it did name", async () => {
    await pull(scannedPastWhileANewerReadLaysIt(() => answer([task(4, { labels: ["bug"] })])));
    expect(await held()).toEqual([20, 4]);
  });

  it("does not let a page after a short one take off a row the short one read past", async () => {
    let call = 0;
    await pull(async () => {
      if (++call === 1) {
        // Keeps #30 and reads on down to #13 before its answer comes back.
        await pull(async () => answer([task(30, { labels: ["bug"] }), labelled]));
        return answer([task(30, { labels: ["bug"] })], { next_cursor: "below-13" });
      }
      return answer([]);
    });
    expect(await held()).toEqual([30, 20]);
  });

  it("still takes off a row every page since it was laid read past", async () => {
    await cache.writeCached(ADDRESS, { tasks: [labelled], columns: [] });
    const answers = [answer([], { next_cursor: "below-13" }), answer([])];
    await pull(async () => answers.shift());
    expect(await held()).toEqual([]);
  });
});

describe("across tabs sharing the cache", () => {
  // Each tab is its own copy of the modules over the one IndexedDB: the
  // boundary between two browser tabs.
  const ADDRESS = { deviceId: "dev-1", entityId: "p1", kind: "tracker-tasks", sub: "" };
  const answer = (tasks) => ({ project_id: "p1", tasks });
  const pull = (api, ask) => api.pullTaskPages({
    ask, deviceId: "dev-1", projectId: "p1", params: { project_id: "p1" }, limit: 100,
    fold: (stretch) => api.foldTasksPage(ADDRESS, stretch, () => []),
  });
  const held = async () => (await cache.readCached(ADDRESS))?.value?.tasks || [];
  const anotherTab = async () => {
    vi.resetModules();
    return import("../src/core/trackerPages.js");
  };
  /** Start a pull in this tab whose one page is held until answered. */
  const heldPull = () => {
    let answerIt;
    const asked = new Promise((resolve) => {
      answerIt = resolve;
    });
    let answered;
    const pulled = pull(pages, () => new Promise((resolve) => {
      answered = resolve;
      answerIt();
    }));
    return { asked, answer: async (page) => { answered(page); await pulled; } };
  };

  it("does not put back a task another tab's newer read took off the list", async () => {
    const old = heldPull();
    await old.asked;
    await pull(await anotherTab(), async () => answer([]));
    await old.answer(answer([task(20)]));
    expect(await held()).toEqual([]);
  });

  it("keeps another tab's newer copy of a row over an older page's with the same updated_at", async () => {
    const old = heldPull();
    await old.asked;
    await pull(await anotherTab(), async () => answer([task(20, { title: "read later" })]));
    await old.answer(answer([task(20, { title: "read first" })]));
    expect((await held()).map((one) => one.title)).toEqual(["read later"]);
  });

  it("does not put back a card another tab moved while a page was out", async () => {
    await cache.writeCached(ADDRESS, { tasks: [task(20, { status: "backlog" })], columns: [] });
    const old = heldPull();
    await old.asked;
    vi.resetModules();
    const otherOrder = await import("../src/core/taskReadOrder.js");
    await otherOrder.noteWritten([ADDRESS], ["task-20"]);
    await cache.writeCached(ADDRESS, { tasks: [task(20, { status: "done" })], columns: [] });
    await old.answer(answer([task(20, { status: "backlog" })]));
    expect((await held()).map((one) => one.status)).toEqual(["done"]);
  });

  // #129: the list says how new it is by when its newest pull was asked, so
  // an older pull's page landing last does not make it look older.
  it("stays stamped with the newer pull when an older one's page lands after it", async () => {
    const old = heldPull();
    await old.asked;
    await pull(await anotherTab(), async () => answer([task(20)]));
    const newer = (await cache.readCached(ADDRESS)).value.read_order;
    await old.answer(answer([task(19)]));
    expect(Number.isFinite(newer)).toBe(true);
    expect((await cache.readCached(ADDRESS)).value.read_order).toBe(newer);
  });

  it("stamps the list with when the pull that laid it was asked", async () => {
    const old = heldPull();
    await old.asked;
    const landed = Date.now();
    await new Promise((resolve) => setTimeout(resolve, 30));
    await old.answer(answer([task(20)]));
    expect((await cache.readCached(ADDRESS)).value.read_order).toBeLessThanOrEqual(landed);
  });

  // A list written here, or by a build from before #129, has no read of its
  // own: it is as new as the cache's stamp on it. A page from an older pull
  // lands under it and must not make the list look older than that.
  describe("over a list with no read of its own", () => {
    const clock = (at) => vi.spyOn(Date, "now").mockReturnValue(at);
    const heldStamp = async () => trackerCache.listAskedAt(await cache.readCached(ADDRESS));
    afterEach(() => vi.restoreAllMocks());

    it("keeps its stamp when a page from an older pull lands after it", async () => {
      clock(1_000_100);
      const old = heldPull();
      await old.asked;
      clock(1_000_120);
      await cache.writeCached(ADDRESS, { tasks: [task(20, { status: "done" })], columns: [] });
      clock(1_000_130);
      await old.answer(answer([task(20)]));
      expect(await heldStamp()).toBe(1_000_120);
    });

    it("takes the stamp of a pull asked after it", async () => {
      clock(1_000_120);
      await cache.writeCached(ADDRESS, { tasks: [task(20)], columns: [] });
      clock(1_000_130);
      await pull(pages, async () => answer([task(20)]));
      expect(await heldStamp()).toBe(1_000_130);
    });
  });

  // The count is the cache's, not the tab's: a write here in another tab, or
  // after a reload, still orders after a read asked in the same millisecond.
  it("numbers a write in another tab after a read on a clock that does not move", async () => {
    vi.spyOn(Date, "now").mockReturnValue(1_000_000);
    const order = await import("../src/core/taskReadOrder.js");
    const asked = await order.nextTaskRead();
    vi.resetModules();
    const otherTab = await import("../src/core/taskReadOrder.js");
    expect(await otherTab.noteWritten([ADDRESS], ["task-20"])).toBeGreaterThan(asked);
    vi.restoreAllMocks();
  });

  it("still lays a page asked after another tab's read", async () => {
    await pull(await anotherTab(), async () => answer([task(20, { title: "read first" })]));
    await pull(pages, async () => answer([task(20, { title: "read later" }), task(19)]));
    expect((await held()).map((one) => one.title)).toEqual(["read later", "task 19"]);
  });

  it("keeps the numbers between different tabs' first-page cursors", async () => {
    let releaseOld;
    let oldAsked;
    const asked = new Promise((resolve) => { oldAsked = resolve; });
    const old = pull(pages, (params) => {
      if (params.cursor) return answer([task(10)]);
      return new Promise((resolve) => {
        releaseOld = () => resolve({ ...answer([task(20)]), next_cursor: "c20" });
        oldAsked();
      });
    });
    await asked;

    const other = await anotherTab();
    let live = true;
    expect(await other.pullTaskPages({
      ask: async () => ({ ...answer([task(30)]), next_cursor: "c30" }),
      deviceId: "dev-1", projectId: "p1", params: { project_id: "p1" }, limit: 100,
      active: () => live,
      fold: async (stretch) => {
        await other.foldTasksPage(ADDRESS, stretch, () => []);
        live = false;
      },
    })).toBe(false);

    releaseOld();
    await old;
    expect(numbers(await held())).toEqual([30, 20, 10]);
  });
});
