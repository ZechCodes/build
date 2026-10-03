// @vitest-environment jsdom
// A paged tracker sync (#85), with nothing stood in for between the bridge's
// answers and the cache: the fixture's real greeting announces
// `tasks.listPaged`, the real sync pass pulls the project's list page by page
// into the real cache, the real change router carries the `tasks` push, and
// the real Tasks pane reads the list on that push.
//
// The bridge here pages the way bridge/src/app/tracker/pages.rs does: number
// descending, `limit` rows, and an opaque cursor naming the last number a page
// answered. Each answer is taken when it is ASKED, so one held back is as old
// as the moment it was asked — the page a push overtakes.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const helloFixture = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8"));
const PAGING_GREETING = helloFixture.result;
const WHOLE_GREETING = {
  ...PAGING_GREETING,
  capabilities: PAGING_GREETING.capabilities.filter((name) => name !== "tasks.listPaged"),
};

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", async () => ({ App: (await import("../src/app.js")).App }));
vi.mock("../src/app.js", () => ({ App }));

const contexts = new Map();
// The device registry is the one seam: the sync pass finds this device's
// session here. Everything else the pane imports from it stays real.
vi.mock("../src/core/deviceContexts.js", async (importOriginal) => ({
  ...(await importOriginal()),
  contextFor: (deviceId) => contexts.get(deviceId) || null,
  liveContexts: () => [...contexts.values()],
  onDeviceStateChanged: () => () => {},
}));

// ─── the bridge ──────────────────────────────────────────────────────────────

const TASKS = 250;
const bridgeClock = { now: Date.parse("2026-09-25T08:00:00Z") };
const tick = () => (bridgeClock.now += 1000);
const stamp = () => new Date(bridgeClock.now).toISOString();

let tracker; // number → task, as the bridge holds them now
let greeting;
let holdNext = null; // a predicate naming the one `tasks.list` ask to hold back
let held = null; // that ask, once it has been asked: its answer and its release

const cursorFor = (number) => btoa(`v1:${number}:p1`);
const numberOf = (cursor) => Number(atob(cursor).split(":")[1]);

function listAnswer(params) {
  tick();
  const below = params.cursor ? numberOf(params.cursor) : Infinity;
  const rows = [...tracker.values()]
    .filter((task) => task.number < below && (!params.state || params.state === task.state))
    .sort((a, b) => b.number - a.number);
  const page = params.limit ? rows.slice(0, params.limit) : rows;
  const more = Boolean(params.limit) && rows.length > params.limit;
  return {
    project_id: "p1",
    tasks: structuredClone(page),
    user_session: { session_started_ms: null, last_activity_ms: null, previous_session_ended_ms: null, gap_ms: 21600000, now_ms: bridgeClock.now },
    ...(more ? { next_cursor: cursorFor(page.at(-1).number) } : {}),
  };
}

function tasksList(params) {
  const answer = listAnswer(params);
  if (!holdNext?.(params)) return answer;
  holdNext = null;
  return new Promise((resolve) => {
    held = { params, release: () => resolve(answer) };
  });
}

const ANSWERS = {
  "session.hello": () => greeting,
  "changes.subscribe": (params) => ({ subscription_id: params.subscription_id, watch: "live" }),
  "board.list": () => ({ items: [] }),
  "project.list": () => ({ projects: [{ project_id: "p1", name: "build" }] }),
  "workspace.list": () => ({ workspaces: [] }),
  "tasks.list": tasksList,
  "tasks.columns": () => ({ project_id: "p1", columns: [{ id: "backlog", name: "Backlog" }] }),
};

const bridge = { call: vi.fn(async (method, params) => (ANSWERS[method] || (() => ({})))(params || {})) };
const lists = () => bridge.call.mock.calls.filter(([method]) => method === "tasks.list").map(([, params]) => params);

// ─── the client ──────────────────────────────────────────────────────────────

let cache, changeEvents, sync, trackerCache, pane;

const registerDevice = (deviceId) => {
  const context = {
    deviceId,
    rpc: (...asked) => bridge.call(...asked),
    session: { device: deviceId },
    greeted: Promise.resolve(),
    cacheScope: { deviceId, active: () => true },
    active: () => contexts.get(deviceId) === context,
  };
  contexts.set(deviceId, context);
  return context;
};

const settle = async () => {
  let before = -1;
  while (before !== bridge.call.mock.calls.length) {
    before = bridge.call.mock.calls.length;
    for (let turn = 0; turn < 20; turn += 1) await new Promise((done) => setTimeout(done, 0));
  }
};

const heldList = async () => (await trackerCache.readTasksRecord("dev-1", "p1"))?.tasks || [];
const numbers = (tasks) => tasks.map((task) => task.number);
const everyNumber = Array.from({ length: TASKS }, (_, index) => TASKS - index);

/** Every paint-worthy moment of the whole list: what it held at each write. */
function watchTheList() {
  const seen = [];
  cache.subscribeCache(trackerCache.tasksAddress("dev-1", "p1"), () => {
    void heldList().then((tasks) => seen.push(tasks));
  });
  return seen;
}

async function boot(hello) {
  greeting = hello;
  await changeEvents.greetBridge((method, params) => bridge.call(method, params), { deviceId: "dev-1", strict: true });
  sync.startCacheSync();
}

function mountPane(defaultView = "dashboard") {
  document.body.innerHTML = '<div id="tasks"></div>';
  return import("../src/core/trackerTasksPane.js").then(({ mountTasksPane }) => {
    pane = mountTasksPane(document.querySelector("#tasks"), {
      projectId: "p1",
      projectName: "build",
      deviceId: "dev-1",
      projectKey: "dev-1|p1",
      callRpc: (method, params) => bridge.call(method, params),
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ workspaces: [], items: [], projects: [] }),
      defaultView,
      navigate: () => {},
    });
  });
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete globalThis.navigator?.locks;
  contexts.clear();
  registerDevice("dev-1");
  bridge.call.mockClear();
  tracker = new Map(everyNumber.map((number) => [number, {
    id: `task-${number}`, project_id: "p1", number, title: `task ${number}`,
    state: "open", status: "backlog", labels: [], created_at: stamp(), updated_at: stamp(),
  }]));
  holdNext = null;
  held = null;
  cache = await import("../src/core/localCache.js");
  changeEvents = await import("../src/core/changeEvents.js");
  trackerCache = await import("../src/core/trackerCache.js");
  sync = await import("../src/core/cacheSync.js");
});

afterEach(() => {
  pane?.dispose();
  pane = null;
  held?.release();
  sync.stopCacheSync();
  changeEvents.resetChangeEvents();
});

describe("a tracker synced from a bridge that pages", () => {
  async function pageAddressRace(changeBridge) {
    // One answer covers the whole Open list. A stale first page can therefore
    // survive as the final list state, with no later page masking the race.
    tracker = new Map([[250, tracker.get(250)]]);
    await boot(PAGING_GREETING);
    await settle();
    sync.stopCacheSync();
    await mountPane("list");
    await settle();
    const newerPane = pane;
    const newerHost = document.querySelector("#tasks");
    const params = { project_id: "p1", state: "open" };
    const openList = trackerCache.tasksQueryAddress("dev-1", "p1", params);

    // Another tab reserves its read and holds the bridge's old answer. It
    // uses the real page pull and list fold over the same IndexedDB as the
    // mounted pane, whose change watcher will start the newer read.
    vi.resetModules();
    const otherPages = await import("../src/core/trackerPages.js");
    const otherCache = await import("../src/core/localCache.js");
    let release;
    let ready;
    const asked = new Promise((resolve) => { ready = resolve; });
    const oldDone = otherPages.pullTaskPages({
      deviceId: "dev-1", projectId: "p1", params, limit: 100,
      ask: (pageParams) => {
        const answer = listAnswer(pageParams);
        return new Promise((resolve) => {
          release = () => resolve(answer);
          ready();
        });
      },
      fold: (stretch) => otherPages.foldTasksPage(openList, stretch, () => []),
    });
    await asked;
    let pageAddress;
    changeBridge();
    let released = false;
    const pagePrefix = { deviceId: "dev-1", entityId: "p1", kind: trackerCache.TRACKER_TASKS_PAGE_KIND };
    const stopWatching = cache.subscribeCache(pagePrefix, (address) => {
      const pageParams = JSON.parse(address.sub);
      if (pageParams.state !== "open" || pageParams.limit !== 100) return;
      if (released) return;
      released = true;
      pageAddress = address;
      release();
    });
    expect(changeEvents.dispatchChangeEvent({
      type: "changes",
      items: [{ entity_id: "p1", tasks: { task_ids: ["task-250"], truncated: false } }],
    }, "dev-1")).toBe(true);
    await vi.waitFor(() => expect(released).toBe(true));
    await oldDone;
    await settle();
    stopWatching();
    newerPane.dispose();
    return { pageAddress, otherCache, newerHost };
  }

  it("keeps a newer pane's page when an older tab answers the same page parameters", async () => {
    const newer = { ...tracker.get(250), title: "new from push", updated_at: new Date(tick()).toISOString() };
    const { pageAddress, otherCache, newerHost } = await pageAddressRace(() => tracker.set(250, newer));
    const openList = trackerCache.tasksQueryAddress("dev-1", "p1", { project_id: "p1", state: "open" });
    expect((await cache.readCached(openList))?.value?.tasks.find((row) => row.number === 250)).toMatchObject(newer);
    expect(newerHost.querySelector('[data-task="task-250"] .task-title')?.textContent).toBe("new from push");
    expect((await otherCache.readCached(pageAddress))?.value?.tasks.find((row) => row.number === 250)).toMatchObject(newer);
  });

  it("keeps a newer empty page's absence when an older tab answers the same page parameters", async () => {
    const { pageAddress, otherCache, newerHost } = await pageAddressRace(() => {
      for (const [number, task] of tracker) tracker.set(number, { ...task, state: "closed" });
    });
    const openList = trackerCache.tasksQueryAddress("dev-1", "p1", { project_id: "p1", state: "open" });
    expect((await cache.readCached(openList))?.value?.tasks).toEqual([]);
    expect(newerHost.querySelector('[data-task="task-250"]')).toBeNull();
    expect((await otherCache.readCached(pageAddress))?.value?.tasks).toEqual([]);
  });

  it("pulls the list page by page and lands every row in the cache, in order", async () => {
    const seen = watchTheList();
    await boot(PAGING_GREETING);
    await settle();

    expect(changeEvents.bridgeCapabilities("dev-1").tasks.listPaged).toBe(true);
    expect(lists()).toEqual([
      { project_id: "p1", limit: 100 },
      { project_id: "p1", limit: 100, cursor: cursorFor(151) },
      { project_id: "p1", limit: 100, cursor: cursorFor(51) },
    ]);
    expect(numbers(await heldList())).toEqual(everyNumber);
    // Each page was written, announced and read back before the next landed:
    // the list grew a page at a time rather than arriving whole at the end.
    expect(seen.map((tasks) => tasks.length)).toEqual([100, 200, 250]);
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_TASKS_PAGE_KIND)).toHaveLength(3);
  });

  it("keeps a row a push brought in over the older copy a page still out was carrying", async () => {
    // The sync pass's second page is held: asked now, answered later.
    holdNext = (params) => params.cursor === cursorFor(151);
    await boot(PAGING_GREETING);
    await vi.waitFor(() => expect(held).not.toBeNull());
    await settle();
    expect(numbers(await heldList())).toEqual(everyNumber.slice(0, 100));

    // The task on that page moves on the bridge, and the bridge pushes it.
    // The Tasks tab on screen reads the list again on the push and writes
    // the new row; the sync pass's own re-read waits behind its pull (#119).
    tracker.set(120, { ...tracker.get(120), title: "moved on", status: "in_progress", updated_at: (tick(), stamp()) });
    await mountPane();
    expect(changeEvents.dispatchChangeEvent({
      type: "changes",
      items: [{ entity_id: "p1", tasks: { task_ids: ["task-120"], truncated: false } }],
    }, "dev-1")).toBe(true);
    await vi.waitFor(async () => expect((await heldList()).find((task) => task.number === 120)?.title).toBe("moved on"));

    // Now the older page lands. It must not put the old row back.
    const seen = watchTheList();
    held.release();
    held = null;
    await settle();

    const titlesOf120 = seen.map((tasks) => tasks.find((task) => task.number === 120)?.title);
    expect(titlesOf120.length).toBeGreaterThan(0);
    expect(titlesOf120.every((title) => title === "moved on")).toBe(true);
    const landed = await heldList();
    expect(numbers(landed)).toEqual(everyNumber);
    expect(landed.find((task) => task.number === 120)).toMatchObject({ title: "moved on", status: "in_progress" });
    // And the push was answered by a read begun after it, not folded away.
    expect(lists().filter((params) => !params.cursor).length).toBeGreaterThanOrEqual(3);
  });

  it("does not put back a task a newer read took off the list while an older page was out", async () => {
    await boot(PAGING_GREETING);
    await settle();
    // The Open list's second page is held: asked now, answered later.
    holdNext = (params) => params.state === "open" && params.cursor === cursorFor(151);
    await mountPane("list");
    await vi.waitFor(() => expect(held).not.toBeNull());
    const olderPane = pane;

    // #120 is closed and pushed. A second Tasks tab reads the Open list
    // after the push, and #120 is not on it.
    tracker.set(120, { ...tracker.get(120), state: "closed", updated_at: (tick(), stamp()) });
    changeEvents.dispatchChangeEvent({
      type: "changes",
      items: [{ entity_id: "p1", tasks: { task_ids: ["task-120"], truncated: false } }],
    }, "dev-1");
    await mountPane("list");
    await settle();
    const openList = trackerCache.tasksQueryAddress("dev-1", "p1", { project_id: "p1", state: "open" });
    const openRows = async () => (await cache.readCached(openList))?.value?.tasks || [];
    expect(numbers(await openRows())).toEqual(everyNumber.filter((number) => number !== 120));

    // Now the older page lands. It must not bring #120 back as open.
    const seen = [];
    const stopWatching = cache.subscribeCache(openList, () => void openRows().then((rows) => seen.push(rows)));
    held.release();
    held = null;
    await settle();
    stopWatching();
    olderPane.dispose();

    expect(seen.some((rows) => rows.some((row) => row.number === 120))).toBe(false);
    expect(numbers(await openRows())).toEqual(everyNumber.filter((number) => number !== 120));
  });

  it("keeps a newer read's row over an older page's copy with the same updated_at", async () => {
    // The bridge fills in who a task's agent is when it lists the task,
    // without touching the task's updated_at: two copies of one row with one
    // timestamp can differ, and only the order they were read in says which
    // is newer.
    holdNext = (params) => params.cursor === cursorFor(151);
    await boot(PAGING_GREETING);
    await vi.waitFor(() => expect(held).not.toBeNull());
    tracker.set(120, { ...tracker.get(120), identities: { agent: { name: "New name", available: true } } });
    await mountPane();
    changeEvents.dispatchChangeEvent({
      type: "changes",
      items: [{ entity_id: "p1", tasks: { task_ids: ["task-120"], truncated: false } }],
    }, "dev-1");
    const nameOf120 = (rows) => rows.find((row) => row.number === 120)?.identities?.agent?.name;
    await vi.waitFor(async () => expect(nameOf120(await heldList())).toBe("New name"));

    const seen = watchTheList();
    held.release();
    held = null;
    await settle();

    expect(seen.length).toBeGreaterThan(0);
    expect(seen.every((rows) => nameOf120(rows) === "New name")).toBe(true);
    expect(numbers(await heldList())).toEqual(everyNumber);
  });

  it("stops the Tasks tab's page walk when the device's session is replaced", async () => {
    await boot(PAGING_GREETING);
    await settle();
    // The tab's second page is held: asked on this session, answered later.
    holdNext = (params) => params.cursor === cursorFor(151);
    await mountPane();
    await vi.waitFor(() => expect(held).not.toBeNull());
    const pagesBefore = await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_TASKS_PAGE_KIND);

    // The device reconnects: a new session replaces the one that asked.
    registerDevice("dev-1");
    const olderAnswer = held;
    held = null;
    bridge.call.mockClear();
    olderAnswer.release();
    await settle();

    // The old session's answer is not written, and no page after it asked.
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_TASKS_PAGE_KIND)).toEqual(pagesBefore);
    expect(lists()).toEqual([]);
  });

  it("pulls the whole list in one read from a bridge that does not page", async () => {
    await boot(WHOLE_GREETING);
    await settle();

    expect(changeEvents.bridgeCapabilities("dev-1").tasks.listPaged).toBe(false);
    expect(lists()).toEqual([{ project_id: "p1" }]);
    expect(numbers(await heldList())).toEqual(everyNumber);
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_TASKS_PAGE_KIND)).toEqual([]);
  });
});
