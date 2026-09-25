// @vitest-environment jsdom
// A paged tracker sync (#85), with nothing stood in for between the bridge's
// answers and the cache: the fixture's real greeting announces
// `issues.listPaged`, the real sync pass pulls the project's list page by page
// into the real cache, the real change router carries the `issues` push, and
// the real Issues pane reads the list on that push.
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
  capabilities: PAGING_GREETING.capabilities.filter((name) => name !== "issues.listPaged"),
};

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
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

const ISSUES = 250;
const bridgeClock = { now: Date.parse("2026-09-25T08:00:00Z") };
const tick = () => (bridgeClock.now += 1000);
const stamp = () => new Date(bridgeClock.now).toISOString();

let tracker; // number → issue, as the bridge holds them now
let greeting;
let holdNext = null; // a predicate naming the one `issues.list` ask to hold back
let held = null; // that ask, once it has been asked: its answer and its release

const cursorFor = (number) => btoa(`v1:${number}:p1`);
const numberOf = (cursor) => Number(atob(cursor).split(":")[1]);

function listAnswer(params) {
  tick();
  const below = params.cursor ? numberOf(params.cursor) : Infinity;
  const rows = [...tracker.values()].filter((issue) => issue.number < below).sort((a, b) => b.number - a.number);
  const page = params.limit ? rows.slice(0, params.limit) : rows;
  const more = Boolean(params.limit) && rows.length > params.limit;
  return {
    project_id: "p1",
    issues: structuredClone(page),
    user_session: { session_started_ms: null, last_activity_ms: null, previous_session_ended_ms: null, gap_ms: 21600000, now_ms: bridgeClock.now },
    ...(more ? { next_cursor: cursorFor(page.at(-1).number) } : {}),
  };
}

function issuesList(params) {
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
  "issues.list": issuesList,
  "issues.columns": () => ({ project_id: "p1", columns: [{ id: "backlog", name: "Backlog" }] }),
};

const bridge = { call: vi.fn(async (method, params) => (ANSWERS[method] || (() => ({})))(params || {})) };
const lists = () => bridge.call.mock.calls.filter(([method]) => method === "issues.list").map(([, params]) => params);

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

const heldList = async () => (await trackerCache.readIssuesRecord("dev-1", "p1"))?.issues || [];
const numbers = (issues) => issues.map((issue) => issue.number);
const everyNumber = Array.from({ length: ISSUES }, (_, index) => ISSUES - index);

/** Every paint-worthy moment of the whole list: what it held at each write. */
function watchTheList() {
  const seen = [];
  cache.subscribeCache(trackerCache.issuesAddress("dev-1", "p1"), () => {
    void heldList().then((issues) => seen.push(issues));
  });
  return seen;
}

async function boot(hello) {
  greeting = hello;
  await changeEvents.greetBridge((method, params) => bridge.call(method, params), { deviceId: "dev-1", strict: true });
  sync.startCacheSync();
}

function mountPane() {
  document.body.innerHTML = '<div id="issues"></div>';
  return import("../src/core/trackerIssuesPane.js").then(({ mountIssuesPane }) => {
    pane = mountIssuesPane(document.querySelector("#issues"), {
      projectId: "p1",
      projectName: "build",
      deviceId: "dev-1",
      projectKey: "dev-1|p1",
      callRpc: (method, params) => bridge.call(method, params),
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => ({ workspaces: [], items: [], projects: [] }),
      defaultView: "dashboard",
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
    id: `issue-${number}`, project_id: "p1", number, title: `issue ${number}`,
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
  it("pulls the list page by page and lands every row in the cache, in order", async () => {
    const seen = watchTheList();
    await boot(PAGING_GREETING);
    await settle();

    expect(changeEvents.bridgeCapabilities("dev-1").issues.listPaged).toBe(true);
    expect(lists()).toEqual([
      { project_id: "p1", limit: 100 },
      { project_id: "p1", limit: 100, cursor: cursorFor(151) },
      { project_id: "p1", limit: 100, cursor: cursorFor(51) },
    ]);
    expect(numbers(await heldList())).toEqual(everyNumber);
    // Each page was written, announced and read back before the next landed:
    // the list grew a page at a time rather than arriving whole at the end.
    expect(seen.map((issues) => issues.length)).toEqual([100, 200, 250]);
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_ISSUES_PAGE_KIND)).toHaveLength(3);
  });

  it("keeps a row a push brought in over the older copy a page still out was carrying", async () => {
    // The sync pass's second page is held: asked now, answered later.
    holdNext = (params) => params.cursor === cursorFor(151);
    await boot(PAGING_GREETING);
    await vi.waitFor(() => expect(held).not.toBeNull());
    await settle();
    expect(numbers(await heldList())).toEqual(everyNumber.slice(0, 100));

    // The issue on that page moves on the bridge, and the bridge pushes it.
    // The Issues tab on screen reads the list again on the push and writes
    // the new row; the sync pass's own re-read waits behind its pull (#119).
    tracker.set(120, { ...tracker.get(120), title: "moved on", status: "in_progress", updated_at: (tick(), stamp()) });
    await mountPane();
    expect(changeEvents.dispatchChangeEvent({
      type: "changes",
      items: [{ entity_id: "p1", issues: { issue_ids: ["issue-120"], truncated: false } }],
    }, "dev-1")).toBe(true);
    await vi.waitFor(async () => expect((await heldList()).find((issue) => issue.number === 120)?.title).toBe("moved on"));

    // Now the older page lands. It must not put the old row back.
    const seen = watchTheList();
    held.release();
    held = null;
    await settle();

    const titlesOf120 = seen.map((issues) => issues.find((issue) => issue.number === 120)?.title);
    expect(titlesOf120.length).toBeGreaterThan(0);
    expect(titlesOf120.every((title) => title === "moved on")).toBe(true);
    const landed = await heldList();
    expect(numbers(landed)).toEqual(everyNumber);
    expect(landed.find((issue) => issue.number === 120)).toMatchObject({ title: "moved on", status: "in_progress" });
    // And the push was answered by a read begun after it, not folded away.
    expect(lists().filter((params) => !params.cursor).length).toBeGreaterThanOrEqual(3);
  });

  it("pulls the whole list in one read from a bridge that does not page", async () => {
    await boot(WHOLE_GREETING);
    await settle();

    expect(changeEvents.bridgeCapabilities("dev-1").issues.listPaged).toBe(false);
    expect(lists()).toEqual([{ project_id: "p1" }]);
    expect(numbers(await heldList())).toEqual(everyNumber);
    expect(await cache.cachedSubKeys("dev-1", "p1", trackerCache.TRACKER_ISSUES_PAGE_KIND)).toEqual([]);
  });
});
