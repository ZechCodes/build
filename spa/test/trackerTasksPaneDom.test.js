/** @vitest-environment jsdom */
// The Tasks tab, mounted: the list, the board, the filters, and the two moves
// that change something.
//
// Cache-first — the project's whole list is on disk, so the tab paints before
// the bridge is asked — and filter-as-param: every control is an `tasks.list`
// param rather than a pass over what happens to be in hand.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, task } from "./trackerWireFixture.js";

let watchers = [];
// The real module refuses a registration that names a cadence — nothing in
// this client polls — so this stand-in refuses one too. A poll creeping back
// into a surface fails here rather than only in a browser.
const RETIRED = ["intervalMs", "keepPolling", "catchUpOnVisible"];
vi.mock("../src/core/changeEvents.js", () => ({
  // The greeting says which kinds a bridge carries; a stand-in that
  // answers none would have the sync layer ask for none of the new ones.
  bridgeCapabilities: () => ({
    changes: { subscriptions: true, kinds: carriedKinds },
    // #57: whether this bridge can carry files on a task. A case that sets
    // it false is an older bridge answering, not another machine.
    tasks: { attachments: carriesAttachments },
  }),
  watchChanges: (registration) => {
    const named = RETIRED.filter((option) => option in registration);
    if (named.length) throw new TypeError(`watchChanges does not poll: remove ${named.join(", ")}`);
    const watcher = { ...registration, disposed: false };
    watchers.push(watcher);
    return { dispose: () => { watcher.disposed = true; } };
  },
}));

/** What this device's greeting says its subscriptions carry. A case naming a
 *  shorter list is an older bridge answering, not another machine. */
const EVERY_KIND = ["state", "thread", "git", "files", "terminals", "tasks"];
let carriedKinds = EVERY_KIND;
/** Whether the bridge under test has `tasks.attach` (1.8). */
let carriesAttachments = true;

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

const openAssigneePicker = vi.fn(() => ({ close: vi.fn(), setCatalog: vi.fn() }));
vi.mock("../src/core/trackerAssigneePicker.js", () => ({
  openAssigneePicker: (...args) => openAssigneePicker(...args),
}));

/** The machine this tab is reading, as core/transientRead.js asks about it.
 *  A case moves it; `reconnect()` is the session coming back. */
let away = true;
let reconnecting = true;
let movedListeners = new Set();
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => null,
  deviceWatch: () => ({
    away: () => away,
    reconnecting: () => reconnecting,
    moved: (fn) => {
      movedListeners.add(fn);
      return () => movedListeners.delete(fn);
    },
  }),
}));

const reconnect = async () => {
  away = false;
  reconnecting = false;
  [...movedListeners].forEach((fn) => fn());
  await flush();
};

const PROJECT_KEY = "dev-1|proj-1";

const feed = {
  workspaces: [{ id: "ws-1", workspace_id: "ws-1", name: "wire-facade", projectKey: PROJECT_KEY, entity_id: "run-1" }],
  items: [{ kind: "branch", projectKey: PROJECT_KEY, run_id: "run-1", agents: [{ id: "agent-1", ordinal: 1 }] }],
};

let host, call, pane, cache, trackerCache, mountTasksPane;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const mount = async (over = {}) => {
  pane = mountTasksPane(host, {
    projectId: "proj-1",
    projectName: "Build",
    deviceId: "dev-1",
    projectKey: PROJECT_KEY,
    callRpc: call,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed: () => feed,
    defaultView: "list",
    navigate: vi.fn(),
    ...over,
  });
  await flush();
  return pane;
};

const listed = (method) => call.mock.calls.filter(([name]) => name === method);
/** The Clear press, when it is being offered. It is mounted once like every
 *  other control on the bar (#43) and hidden rather than removed — taking a
 *  button out from under the reader takes whatever focus was on it too — so
 *  "is it offered" is a question about `hidden` and not about the DOM. */
const clearPress = () => {
  const press = host.querySelector("[data-task-filter-clear]");
  return press && !press.hidden ? press : null;
};
/** One filter menu, by the filter it writes (#44). The native selects went;
 *  every filter is now the same custom control. */
/** The inline composer (#57), and the press that opens it. */
const newPress = () => host.querySelector("[data-task-new]");
const composer = () => host.querySelector(".task-compose");
const openComposer = async () => {
  newPress().click();
  await flush();
  return composer();
};
const typeIn = (selector, value) => {
  const field = host.querySelector(selector);
  field.value = value;
  field.dispatchEvent(new Event("input"));
  return field;
};
/** Choose an assignee in the composer's own control. */
const chooseAssignee = async (optionId) => {
  const select = host.querySelector("#task-new-assignee");
  select.value = optionId;
  select.dispatchEvent(new Event("change"));
  await flush();
};
const fileIt = async () => {
  host.querySelector("[data-compose-file]").click();
  await flush();
};

const menu = (name) => host.querySelector(`[data-filter-menu="${name}"]`);
const menuPress = (name) => menu(name).querySelector(".fmenu-press");
const openMenu = async (name) => {
  const press = menuPress(name);
  if (press.getAttribute("aria-expanded") !== "true") press.click();
  await flush();
  return menu(name);
};
const menuRows = (name) => [...menu(name).querySelectorAll(".fmenu-row")];
const menuLabels = (name) => menuRows(name).map((row) => row.textContent.trim());

/** Choose one filter, the way a reader does: open the menu and press a row. */
const chooseFilter = async (name, value) => {
  await openMenu(name);
  const row = menuRows(name).find((one) => one.dataset.value === value);
  if (!row) throw new Error(`no "${value}" row in the ${name} menu: ${menuLabels(name).join(", ")}`);
  row.click();
  await flush();
};

/** Clear one menu from inside it — the press in its own footer. */
const clearMenu = async (name) => {
  await openMenu(name);
  menu(name).querySelector(".fmenu-clear").click();
  await flush();
};
const titles = () => [...host.querySelectorAll(".task-title")].map((one) => one.textContent);
const columnNames = () => [...host.querySelectorAll(".task-column-head h3")].map((one) => one.textContent);
const cardsIn = (columnId) =>
  [...host.querySelectorAll(`[data-column="${columnId}"] .task-card`)].map((card) => card.dataset.task);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  watchers = [];
  carriedKinds = EVERY_KIND;
  carriesAttachments = true;
  away = true;
  reconnecting = true;
  movedListeners = new Set();
  notifyError.mockClear();
  openAssigneePicker.mockClear();
  document.body.innerHTML = '<div id="pane"></div>';
  host = document.querySelector("#pane");
  cache = await import("../src/core/localCache.js");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountTasksPane } = await import("../src/core/trackerTasksPane.js"));
  call = vi.fn(async (method) => {
    if (method === "tasks.list") {
      return {
        tasks: [
          task({ number: 12, id: "task-12", status: "in_progress", labels: ["bug"], assignee: { kind: "user" } }),
          task({ number: 11, id: "task-11", title: "Board is unreadable on a phone", state: "closed", status: "done" }),
        ],
      };
    }
    return {};
  });
});

afterEach(() => {
  pane?.dispose();
});

describe("painting before the bridge is asked", () => {
  it("paints the cached list first, then the live one", async () => {
    await trackerCache.writeTasksRecord("dev-1", "proj-1", {
      tasks: [task({ number: 9, id: "task-9", title: "From the cache" })],
      columns: columns(),
    });
    // The cached paint lands before the RPC settles: the call is held open.
    let answer;
    call = vi.fn((method) => (method === "tasks.list" ? new Promise((resolve) => { answer = resolve; }) : Promise.resolve({})));
    await mount();
    expect(titles()).toEqual(["From the cache"]);
    answer({ tasks: [task({ number: 12, id: "task-12", title: "From the bridge" })] });
    await flush();
    expect(titles()).toEqual(["From the bridge"]);
  });

  it("says nothing at all for a project this device has never read", async () => {
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("repaints from a query-cache write while the pulled payload is absent", async () => {
    call = vi.fn(() => new Promise(() => {}));
    await mount();
    expect(titles()).toEqual([]);

    await trackerCache.writeTasksQueryRecord(
      "dev-1",
      "proj-1",
      { project_id: "proj-1", state: "open" },
      trackerCache.tasksRecord([task({ id: "task-cache", number: 30, title: "Announced from cache" })], columns()),
    );
    await flush();

    expect(titles()).toEqual(["Announced from cache"]);
  });
});

describe("the Dashboard", () => {
  const dashboardRows = (section) => [...host.querySelectorAll(`[data-dashboard-section="${section}"] .task-dashboard-row`)]
    .map((row) => row.dataset.task);
  const tabs = () => [...host.querySelectorAll('[role="tablist"] [role="tab"]')];
  const chooseTab = async (id) => {
    host.querySelector(`[data-dashboard-tab="${id}"]`).click();
    await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]').dataset.dashboardSection).toBe(id));
  };
  const mountDashboard = () => {
    pane = mountTasksPane(host, {
      projectId: "proj-1",
      projectName: "Build",
      deviceId: "dev-1",
      projectKey: PROJECT_KEY,
      callRpc: call,
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => feed,
      defaultView: "dashboard",
      navigate: vi.fn(),
    });
  };

  it("defaults to Needs you with four counted tabs and switches the sole list from cached records", async () => {
    const working = task({ id: "working", number: 4, title: "Write release notes", assignee: { kind: "agent", agent_id: "agent-1" } });
    const review = task({ id: "review", number: 3, title: "Review patch", status: "in_review" });
    const done = task({ id: "done", number: 2, title: "Shipped fix", status: "done", state: "closed", links: { commits: ["abc123def456"] } });
    const movedAt = new Date(Date.now() - 60_000).toISOString();
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [working, review, done], columns: columns() });
    await trackerCache.writeTaskRecord("dev-1", "proj-1", done.id, trackerCache.taskRecord(done, [
      { type: "event", kind: "moved", at: movedAt, payload: { to: "done" } },
    ]));
    const { threadCacheAddress } = await import("../src/core/conversationCache.js");
    const thread = threadCacheAddress({ deviceId: "dev-1", entityId: "run-1", agentId: "agent-1", conversationId: "conv-1" });
    await cache.writeCached(thread, { items: [{ type: "message", data: { role: "agent", body: "Writing summary\nNext line" } }] });
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true, conversation_id: "conv-1", name: "Writer" }] }] };
    call = vi.fn(() => new Promise(() => {}));
    await mount({ feed: () => activeFeed, defaultView: undefined });

    expect(host.querySelector('[data-task-view="dashboard"]').getAttribute("aria-pressed")).toBe("true");
    expect(clearPress()).toBeNull();
    expect(tabs().map((tab) => [tab.dataset.dashboardTab, tab.textContent.trim(), tab.getAttribute("aria-selected")]))
      .toEqual([
        ["needsYou", "Needs you1", "true"], ["active", "Active1", "false"],
        ["backlog", "Backlog1", "false"], ["done", "Done1", "false"],
      ]);
    expect(dashboardRows("needsYou")).toEqual(["review"]);
    expect(host.querySelectorAll('[role="tabpanel"]')).toHaveLength(1);
    await chooseTab("active");
    expect(dashboardRows("active")).toEqual(["working"]);
    expect(dashboardRows("needsYou")).toEqual([]);
    expect(host.querySelector('[role="tab"][aria-selected="true"]').dataset.dashboardTab).toBe("active");
    expect(host.querySelector('[data-dashboard-section="active"] [data-active-group="working"] .task-dashboard-detail').textContent)
      .toMatch(/ · working now · Writing summary$/);
    await chooseTab("done");
    expect(dashboardRows("done")).toEqual(["done"]);
    expect(host.querySelector('[data-dashboard-section="done"] .task-dashboard-detail').textContent)
      .toContain("abc123def456");
  });

  it("splits Done under plain age titles, newest first, with each row drawn as before", async () => {
    const minutes = (count) => new Date(Date.now() - count * 60_000).toISOString();
    const finished = [
      ["older", 2, "Older fix", minutes(130)],
      ["recent", 3, "Recent fix", minutes(1)],
      ["half", 4, "Half hour fix", minutes(35)],
      ["fresh", 5, "Fresh fix", minutes(5)],
    ].map(([id, number, title, movedAt]) => [task({ id, number, title, status: "done", state: "closed" }), movedAt]);
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: finished.map(([done]) => done), columns: columns() });
    for (const [done, movedAt] of finished) {
      await trackerCache.writeTaskRecord("dev-1", "proj-1", done.id, trackerCache.taskRecord(done, [
        { type: "event", kind: "moved", at: movedAt, payload: { to: "done" } },
      ]));
    }
    call = vi.fn(() => new Promise(() => {}));
    await mount({ defaultView: undefined });
    await chooseTab("done");
    await vi.waitFor(() => expect(dashboardRows("done")).toHaveLength(4));

    const panel = host.querySelector('[data-dashboard-section="done"]');
    expect(panel.classList.contains("is-grouped")).toBe(true);
    const blocks = [...panel.querySelectorAll(".task-dashboard-group")];
    // Each group: its title above and outside its own panel, then the panel
    // holding that group's own list of rows.
    expect(blocks.map((block) => [
      block.children[0].tagName, block.children[0].textContent,
      block.children[1].className,
      [...block.querySelectorAll(".task-dashboard-group-panel > .task-dashboard-list > li")].map((row) => row.dataset.task),
    ])).toEqual([
      ["H3", "Last 15 minutes", "task-dashboard-group-panel", ["fresh", "recent"]],
      ["H3", "30 minutes ago", "task-dashboard-group-panel", ["half"]],
      ["H3", "2 hours ago", "task-dashboard-group-panel", ["older"]],
    ]);
    expect(panel.querySelectorAll(".task-dashboard-group-panel .task-dashboard-group-title")).toHaveLength(0);
    expect(host.querySelector(".task-dashboard-group-title").children).toHaveLength(0);
    expect(host.querySelector('[data-dashboard-tab="done"] .task-dashboard-count').textContent).toBe("4");
    const row = host.querySelector('[data-dashboard-section="done"] [data-task="recent"]');
    expect(row.className).toBe("task-dashboard-row");
    expect([...row.querySelector(".task-dashboard-link").children].map((part) => [part.className, part.textContent]))
      .toEqual([["task-dashboard-number", "#3"], ["task-dashboard-title", "Recent fix"], ["task-dashboard-detail", "Moved to Done"]]);
    expect(row.querySelector(".task-dashboard-link").getAttribute("href")).toContain("recent");

    // A repaint keeps each row's element while it stays in its group.
    const renamed = finished.map(([done]) => (done.id === "older" ? { ...done, title: "Older fix, renamed" } : done));
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: renamed, columns: columns() });
    await vi.waitFor(() => expect(host.querySelector('[data-task="older"] .task-dashboard-title').textContent)
      .toBe("Older fix, renamed"));
    expect(host.querySelector('[data-dashboard-section="done"] [data-task="recent"]')).toBe(row);
  });

  it("groups agents' tasks in Active, leaves the user's to Needs you, and Backlog a flat list of unassigned tasks", async () => {
    const identities = { "agent-7": { agent_id: "agent-7", name: "Still review", ordinal: 1, workspace_name: "Composer", available: true } };
    const open = [
      task({ id: "loose", number: 9, title: "Loose end", status: "ready" }),
      task({ id: "held", number: 8, title: "Held by an agent", assignee: { kind: "agent", agent_id: "agent-7" }, identities }),
      task({ id: "started", number: 7, title: "Started", status: "in_progress" }),
      task({ id: "mine", number: 6, title: "Mine", status: "ready", priority: "high", assignee: { kind: "user" } }),
      task({ id: "filed", number: 5, title: "Filed" }),
    ];
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: open, columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    await mount({ defaultView: undefined });
    await chooseTab("active");
    await vi.waitFor(() => expect(dashboardRows("active")).toEqual(["held"]));

    const active = host.querySelector('[data-dashboard-section="active"]');
    expect(active.classList.contains("is-grouped")).toBe(true);
    expect(host.querySelector('[data-dashboard-tab="active"] .task-dashboard-count').textContent).toBe("1");
    expect(host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count').textContent).toBe("1");
    const detailOf = (row) => row.querySelector(".task-dashboard-detail").textContent;
    expect([...active.querySelectorAll(".task-dashboard-group")].map((block) => [
      block.dataset.activeGroup, block.children[0].tagName, block.children[0].textContent, block.children[1].className,
      [...block.querySelectorAll(".task-dashboard-group-panel > .task-dashboard-list > li")].map((row) => [row.dataset.task, detailOf(row)]),
    ])).toEqual([
      ["assigned", "H3", "Assigned", "task-dashboard-group-panel",
        [["held", "With Composer · Still review · Backlog"]]],
    ]);
    expect(active.querySelectorAll(".task-dashboard-group-panel .task-dashboard-group-title")).toHaveLength(0);
    await chooseTab("backlog");
    await vi.waitFor(() => expect(dashboardRows("backlog")).toEqual(["loose", "started", "filed"]));
    const backlog = host.querySelector('[data-dashboard-section="backlog"]');
    expect(backlog.classList.contains("is-grouped")).toBe(false);
    expect(backlog.querySelector(".task-dashboard-groups")).toBeNull();
    expect(host.querySelector('[data-dashboard-tab="backlog"] .task-dashboard-count').textContent).toBe("3");
    expect([...backlog.querySelectorAll(".task-dashboard-list > li")].map((row) => [row.dataset.task, detailOf(row)]))
      .toEqual([["loose", "Ready"], ["started", "In progress"], ["filed", "Backlog"]]);
    const row = backlog.querySelector('[data-task="filed"]');
    expect(row.className).toBe("task-dashboard-row");
    expect([...row.querySelector(".task-dashboard-link").children].map((part) => [part.className, part.textContent]))
      .toEqual([["task-dashboard-number", "#5"], ["task-dashboard-title", "Filed"], ["task-dashboard-detail", "Backlog"]]);
    expect(row.querySelector(".task-dashboard-link").getAttribute("href")).toContain("filed");

    // A repaint keeps each row's element while it stays in Backlog.
    const renamed = open.map((one) => (one.id === "loose" ? { ...one, title: "Loose end, renamed" } : one));
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: renamed, columns: columns() });
    await vi.waitFor(() => expect(host.querySelector('[data-task="loose"] .task-dashboard-title').textContent)
      .toBe("Loose end, renamed"));
    expect(host.querySelector('[data-dashboard-section="backlog"] [data-task="filed"]')).toBe(row);

    // Assigning the remaining tasks to the user moves them out of Backlog and
    // into Needs you, never into Active.
    const allHeld = renamed.map((one) => (one.assignee ? one : { ...one, assignee: { kind: "user" } }));
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: allHeld, columns: columns() });
    await vi.waitFor(() => expect(dashboardRows("backlog")).toEqual([]));
    expect(host.querySelector('[data-dashboard-tab="backlog"] .task-dashboard-count').textContent).toBe("0");
    expect(host.querySelector(".task-dashboard-empty").textContent).toBe("No unassigned tasks.");
    await chooseTab("active");
    expect(dashboardRows("active")).toEqual(["held"]);
    expect([...host.querySelectorAll('[data-dashboard-section="active"] .task-dashboard-group-title')]
      .map((one) => one.textContent)).toEqual(["Assigned"]);
    await chooseTab("needsYou");
    expect(dashboardRows("needsYou").sort()).toEqual(["filed", "loose", "mine", "started"]);
    expect(host.querySelector('[data-dashboard-tab="needsYou"] .task-dashboard-count').textContent).toBe("4");
  });

  // The activity line reads a reference as its words, off the shared index
  // (#231), and the index can learn a name after the row is drawn — here
  // another project's workspace — with nothing else on the Dashboard moving.
  it("redraws the activity line when the reference index learns a name it wrote", async () => {
    const index = await import("../src/core/referenceIndex.js");
    const working = task({ id: "working", number: 4, title: "Work", assignee: { kind: "agent", agent_id: "agent-1" } });
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [working], columns: columns() });
    const { threadCacheAddress } = await import("../src/core/conversationCache.js");
    await cache.writeCached(threadCacheAddress({ deviceId: "dev-1", entityId: "run-1", agentId: "agent-1", conversationId: "conv-1" }), {
      items: [{ type: "message", data: { role: "agent", body: "Porting @workspace:Elsewhere" } }],
    });
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true, conversation_id: "conv-1" }] }] };
    call = vi.fn(() => new Promise(() => {}));
    try {
      await mount({ feed: () => activeFeed, defaultView: undefined });
      await chooseTab("active");
      await vi.waitFor(() => expect(host.querySelector('[data-dashboard-section="active"] .task-dashboard-detail').textContent)
        .toMatch(/Porting @workspace:Elsewhere$/));

      index.holdReferenceSources({
        feed: { workspaces: [{ projectKey: "dev-2/proj-9", workspace_id: "ws-9", name: "Elsewhere" }], projects: [], items: [] },
        tasks: {},
      });
      await flush();

      expect(host.querySelector('[data-dashboard-section="active"] .task-dashboard-detail').textContent).toMatch(/Porting Elsewhere$/);
    } finally {
      index.holdReferenceSources({});
    }
  });

  it("redraws from real conversation and detail cache writes, with no bridge answer", async () => {
    const working = task({ id: "working", number: 4, title: "Work", assignee: { kind: "agent", agent_id: "agent-1" } });
    const done = task({ id: "done", number: 3, title: "Done", status: "done", state: "closed" });
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [working, done], columns: columns() });
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true, conversation_id: "conv-1" }] }] };
    call = vi.fn(() => new Promise(() => {}));
    await mount({ feed: () => activeFeed, defaultView: undefined });
    await chooseTab("active");
    expect(dashboardRows("active")).toEqual(["working"]);

    const { threadCacheAddress } = await import("../src/core/conversationCache.js");
    await cache.writeCached(threadCacheAddress({ deviceId: "dev-1", entityId: "run-1", agentId: "agent-1", conversationId: "conv-1" }), {
      items: [{ type: "message", data: { role: "agent", body: "Cached new activity" } }],
    });
    await trackerCache.writeTaskRecord("dev-1", "proj-1", done.id, trackerCache.taskRecord(done, [
      { type: "event", kind: "moved", at: new Date().toISOString(), payload: { to: "done" } },
    ]));
    await flush();

    expect(host.querySelector('[data-dashboard-section="active"] .task-dashboard-detail').textContent)
      .toContain("Cached new activity");
    await chooseTab("done");
    expect(dashboardRows("done")).toEqual(["done"]);
    expect(host.querySelector('[data-dashboard-section="done"] .task-dashboard-link').getAttribute("href"))
      .toContain("done");
  });

  it("derives Needs you from cached unread comments and excludes questions, Done, and closed tasks", async () => {
    const questioned = task({ id: "questioned", number: 4, title: "Question only" });
    const unread = task({ id: "unread", number: 3, title: "Unread comment", watched: true });
    const done = task({ id: "done", number: 2, title: "Done", status: "done", assignee: { kind: "user" } });
    const closed = task({ id: "closed", number: 1, title: "Closed", state: "closed", status: "in_review" });
    const tasks = [questioned, unread, done, closed];
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks, columns: columns() });
    for (const one of tasks) {
      await trackerCache.writeTaskRecord("dev-1", "proj-1", one.id, trackerCache.taskRecord(one, [{
        type: "comment", id: "tc-02", author: { kind: "agent", agent_id: "agent-1" }, body: "Could you review this?",
      }]));
    }
    const inboxFeed = { ...feed, items: [...feed.items, ...[unread, done, closed].map((one) => ({
      kind: "tracker_task", projectKey: PROJECT_KEY, task_id: one.id, unread: 1,
    }))] };
    call = vi.fn(() => new Promise(() => {}));
    pane = mountTasksPane(host, {
      projectId: "proj-1",
      projectName: "Build",
      deviceId: "dev-1",
      projectKey: PROJECT_KEY,
      callRpc: call,
      catalog: () => ({ providers: [] }),
      refreshCatalog: async () => ({ providers: [] }),
      feed: () => inboxFeed,
      defaultView: undefined,
      navigate: vi.fn(),
    });

    await vi.waitFor(() => expect(dashboardRows("needsYou")).toEqual(["unread"]));

    const read = { ...unread, read_through: "tc-02" };
    await trackerCache.writeTaskRecord("dev-1", "proj-1", unread.id, trackerCache.taskRecord(read, [{
      type: "comment", id: "tc-02", author: { kind: "agent", agent_id: "agent-1" }, body: "Could you review this?",
    }]));
    await vi.waitFor(() => expect(dashboardRows("needsYou")).toEqual([]));
  });

  it("keeps the chosen tab across a remount and moves it with arrow keys", async () => {
    call = vi.fn(() => new Promise(() => {}));
    mountDashboard();
    await vi.waitFor(() => {
      expect(tabs().map((tab) => tab.querySelector(".task-dashboard-count").textContent)).toEqual(["0", "0", "0", "0"]);
      expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("needsYou");
      expect(host.querySelector(".task-dashboard-empty")?.textContent).toBe("Nothing needs your look right now.");
    });
    const first = host.querySelector('[data-dashboard-tab="needsYou"]');
    first.focus();
    first.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await vi.waitFor(() => expect(host.querySelector('[role="tabpanel"]').dataset.dashboardSection).toBe("active"));
    expect(document.activeElement.dataset.dashboardTab).toBe("active");
    pane.dispose();
    host.innerHTML = "";
    mountDashboard();
    await vi.waitFor(() => {
      expect(tabs().map((tab) => tab.querySelector(".task-dashboard-count").textContent)).toEqual(["0", "0", "0", "0"]);
      expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("active");
      expect(host.querySelector(".task-dashboard-empty")?.textContent).toBe("No one holds a task right now.");
    });
    expect(host.querySelector('[data-dashboard-tab="active"]').getAttribute("tabindex")).toBe("0");
  });

  it("shows each section's existing empty text under its tab", async () => {
    call = vi.fn(() => new Promise(() => {}));
    mountDashboard();
    await vi.waitFor(() => {
      expect(tabs().map((tab) => tab.querySelector(".task-dashboard-count").textContent)).toEqual(["0", "0", "0", "0"]);
      expect(host.querySelector('[role="tabpanel"]')?.dataset.dashboardSection).toBe("needsYou");
      expect(host.querySelector(".task-dashboard-empty")?.textContent).toBe("Nothing needs your look right now.");
    });
    expect(host.querySelector(".task-dashboard-empty").textContent).toBe("Nothing needs your look right now.");
    await chooseTab("active");
    expect(host.querySelector(".task-dashboard-empty").textContent).toBe("No one holds a task right now.");
    await chooseTab("backlog");
    const backlog = host.querySelector('[data-dashboard-section="backlog"]');
    expect(backlog.classList.contains("is-grouped")).toBe(false);
    expect(backlog.querySelector(".task-dashboard-groups")).toBeNull();
    expect(host.querySelector(".task-dashboard-empty").textContent).toBe("No unassigned tasks.");
    await chooseTab("done");
    expect(host.querySelector(".task-dashboard-empty").textContent).toBe("Nothing moved to Done in the last 24 hours.");
  });

  it("updates tab counts and the selected list after a whole-list cache write", async () => {
    const first = task({ id: "first", number: 5, status: "in_review" });
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [first], columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    mountDashboard();
    await vi.waitFor(() => {
      expect(tabs().map((tab) => tab.querySelector(".task-dashboard-count").textContent)).toEqual(["1", "0", "1", "0"]);
      expect(dashboardRows("needsYou")).toEqual(["first"]);
    });
    const second = task({ id: "second", number: 6, status: "in_review" });
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [second, first], columns: columns() });
    await vi.waitFor(() => expect(tabs()[0].textContent.trim()).toBe("Needs you2"));
    expect(dashboardRows("needsYou")).toEqual(["second", "first"]);
    expect(host.querySelector('[data-dashboard-tab="backlog"] .task-dashboard-count').textContent).toBe("2");
  });
});

describe("the list", () => {
  it("groups cached working and review tasks ahead of the remaining rows and collapses each group", async () => {
    const tasks = [
      task({ id: "rest", number: 4, title: "Other work" }),
      task({ id: "review", number: 3, title: "Review this", status: "in_review" }),
      task({ id: "working", number: 2, title: "Agent is working", assignee: { kind: "agent", agent_id: "agent-1" } }),
      task({ id: "mine", number: 1, title: "Assigned to me", assignee: { kind: "user" } }),
    ];
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks, columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true }] }] };
    await mount({ feed: () => activeFeed });

    expect(titles()).toEqual(["Agent is working", "Review this", "Assigned to me", "Other work"]);
    expect([...host.querySelectorAll(".task-group-heading")].map((one) => one.textContent.trim()))
      .toEqual(["In progress with an agent1", "Needs you2", "Other tasks1"]);
    host.querySelector('[data-task-group-toggle="working"]').click();
    await vi.waitFor(() => expect(host.querySelector('[data-task-group="working"] .task-rows').hidden).toBe(true));
    expect(host.querySelector('[data-task-group-toggle="working"]').getAttribute("aria-expanded")).toBe("false");
  });

  it("moves a row into Needs you after a real task-detail cache announcement", async () => {
    const one = task({ id: "watched", number: 8, title: "Watched work", watched: true });
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [one], columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    const inboxFeed = { ...feed, items: [...feed.items, {
      kind: "tracker_task", projectKey: PROJECT_KEY, task_id: one.id, unread: 1,
    }] };
    await mount({ feed: () => inboxFeed });
    expect(host.querySelector('[data-task-group="rest"] [data-task="watched"]')).not.toBeNull();

    await trackerCache.writeTaskRecord("dev-1", "proj-1", one.id, trackerCache.taskRecord(one, [{
      type: "comment", id: "tc-02", author: { kind: "agent", agent_id: "agent-1" }, body: "Please take a look",
    }]));
    await vi.waitFor(() => expect(host.querySelector('[data-task-group="needsYou"] [data-task="watched"]')).not.toBeNull());
  });

  it("shows cached tasks a page at a time and appends the next page without a bridge answer", async () => {
    const tasks = Array.from({ length: 54 }, (_, index) => task({
      id: `task-${54 - index}`, number: 54 - index, title: `Cached ${54 - index}`,
    }));
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks, columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    await mount();

    expect(titles()).toHaveLength(25);
    expect(titles()[0]).toBe("Cached 54");
    expect(host.querySelector(".task-paging").textContent).toContain("25 of 54");
    host.querySelector("[data-task-more]").click();
    await vi.waitFor(() => expect(titles()).toHaveLength(50));
    expect(titles()[25]).toBe("Cached 29");
    host.querySelector("[data-task-more]").click();
    await vi.waitFor(() => expect(titles()).toHaveLength(54));
    expect(host.querySelector("[data-task-more]")).toBeNull();
  });

  it("keeps an expanded page after a late cache write", async () => {
    const tasks = Array.from({ length: 30 }, (_, index) => task({
      id: `task-${30 - index}`, number: 30 - index, title: `Cached ${30 - index}`,
    }));
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks, columns: columns() });
    let answer;
    call = vi.fn((method) => method === "tasks.list" ? new Promise((resolve) => { answer = resolve; }) : Promise.resolve({}));
    await mount();
    host.querySelector("[data-task-more]").click();
    answer({ tasks: [...tasks, task({ id: "task-31", number: 31, title: "New task" })] });
    await flush();
    expect(titles()).toHaveLength(31);
    expect(titles()[0]).toBe("New task");
  });

  it("draws a row per task, newest number first", async () => {
    await mount();
    expect([...host.querySelectorAll(".task-number")].map((one) => one.textContent)).toEqual(["#12", "#11"]);
  });

  // #28: the number and the title have line one to themselves, and everything
  // else is on line two in one order.
  it("puts the number and the title on the first line, alone", async () => {
    await mount();
    const line = host.querySelector(".task-row .task-row-open");
    expect([...line.children].map((one) => one.classList[0])).toEqual(["task-number", "task-title"]);
    expect(line.querySelector(".task-title").textContent).toBe("Kanban drag does not persist");
  });

  it("puts the column, the age, the labels and the holder on the second", async () => {
    await mount();
    const facts = host.querySelector(".task-row .task-row-facts");
    // The labels are one group of muted words rather than a pill each (#45).
    expect([...facts.children].map((one) => one.classList[0]))
      .toEqual(["task-status", "task-age", "task-labels", "task-assign"]);
    expect(facts.querySelector(".task-status").textContent).toBe("In progress");
    expect(facts.querySelector(".task-labels .task-label").textContent).toBe("bug");
    expect(facts.querySelector(".task-assign").textContent.trim()).toBe("You");
  });

  // The coloured state dot went with the dots (#28). The board card, the
  // agent's entry and the task's own page still carry it.
  it("draws no state dot and no separators on a row", async () => {
    await mount();
    const rows = host.querySelectorAll(".task-row");
    expect([...rows].some((one) => one.querySelector(".task-state"))).toBe(false);
    expect([...rows].some((one) => one.querySelector(".task-sep"))).toBe(false);
  });

  // An unheld row says nothing rather than saying "Unassigned" — but the press
  // is still there, offering the word it would act on.
  it("says nothing about an assignee nobody is, and still offers the press", async () => {
    await mount();
    const unheld = host.querySelectorAll(".task-row")[1];
    expect(unheld.textContent).not.toContain("Unassigned");
    expect(unheld.querySelector(".task-assignee")).toBeNull();
    expect(unheld.querySelector(".task-assign").textContent.trim()).toBe("Assign");
  });

  it("opens each row on that task's page, on the machine the project is on", async () => {
    await mount();
    expect(host.querySelector(".task-row-open").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/tasks/task-12");
  });

  it("says nothing is here yet when the project has no tasks", async () => {
    call = vi.fn(async () => ({ tasks: [] }));
    await mount();
    expect(host.querySelector(".task-empty h2").textContent).toBe("No tasks yet");
  });
});

describe("the filters", () => {
  // A filter is a param, not a client-side pass over everything.
  it("sends each one to tasks.list", async () => {
    await mount();
    call.mockClear();
    await chooseFilter("state", "open");
    expect(listed("tasks.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
  });

  it("offers every column, including the ones nothing stands in", async () => {
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [], columns: columns() });
    await mount();
    await openMenu("status");
    expect(menuRows("status").map((one) => one.dataset.value))
      .toEqual(["", "backlog", "ready", "in_progress", "in_review", "done"]);
  });

  it("offers a clear only once something is narrowed", async () => {
    await mount();
    expect(clearPress()).toBeNull();
    await chooseFilter("label", "bug");
    expect(clearPress()).not.toBeNull();
  });

  it("says the filter is why the list is empty, not the project", async () => {
    call = vi.fn(async () => ({ tasks: [] }));
    await mount();
    await chooseFilter("state", "closed");
    expect(host.querySelector(".task-empty").textContent).toContain("No task matches these filters");
  });
});

describe("the board", () => {
  const board = async () => {
    await mount({ view: "board" });
  };

  it("draws one column per column the project has, in their order", async () => {
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [], columns: columns() });
    await board();
    expect(columnNames()).toEqual(["Backlog", "Ready", "In progress", "In review", "Done"]);
  });

  // A column is what `status` says, not a filter somebody typed.
  it("draws a column nothing stands in", async () => {
    await board();
    expect(cardsIn("ready")).toEqual([]);
    expect(cardsIn("in_progress")).toEqual(["task-12"]);
  });

  // The columns ARE the statuses, so narrowing by one would empty the rest.
  // The state filter still rides — the board opens on open tasks like the
  // list does (#33) — but the column does not: the board's columns ARE the
  // statuses, so narrowing by one would empty every other column.
  it("does not send the column filter while the board is open", async () => {
    await board();
    call.mockClear();
    await chooseFilter("status", "done");
    expect(listed("tasks.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
  });

  // Hover for a pointer, an info glyph for everything without one — the same
  // words both ways, so a phone is not told less than a laptop.
  it("says what each column means, on hover and behind a glyph", async () => {
    await board();
    const head = host.querySelector('[data-column="in_review"] .task-column-head');
    expect(head.getAttribute("title")).toContain("ready to be looked at, not that it is accepted");
    const why = head.querySelector(".task-column-why");
    expect(why.open).toBe(false);
    why.querySelector("summary").click();
    expect(why.open).toBe(true);
    expect(why.querySelector(".task-column-note").textContent)
      .toContain("ready to be looked at, not that it is accepted");
  });

  it("puts the glyph on every column, empty ones included", async () => {
    await board();
    const columns = [...host.querySelectorAll(".task-column")];
    expect(columns.length).toBeGreaterThan(1);
    expect(columns.every((one) => one.querySelector(".task-column-why"))).toBe(true);
  });

  // The other half of the complaint: five columns and an open/closed mark with
  // no legend. The list row dropped its mark with the dots (#28), so the board
  // — where the mark still is — is where this is now asked.
  it("says on the open/closed mark that it moves independently of the column", async () => {
    await mount();
    host.querySelector('[data-task-view="board"]').click();
    await flush();
    expect(host.querySelector(".task-card .task-state").getAttribute("title"))
      .toContain("The two move independently");
  });

  it("switches between the two views", async () => {
    await mount();
    expect(host.querySelector(".task-board")).toBeNull();
    host.querySelector('[data-task-view="board"]').click();
    await flush();
    expect(host.querySelector(".task-board")).not.toBeNull();
    expect(host.querySelector(".task-rows")).toBeNull();
  });
});

describe("moving a card", () => {
  const drop = (taskId, columnId) => {
    const data = new Map([["text/plain", taskId]]);
    host.querySelector(`[data-column-drop="${columnId}"]`).dispatchEvent(
      Object.assign(new Event("drop", { bubbles: true }), {
        preventDefault() {},
        dataTransfer: { getData: (key) => data.get(key) },
      }),
    );
  };

  it("calls tasks.update with the new status, and only that", async () => {
    await mount({ view: "board" });
    call.mockClear();
    drop("task-12", "in_review");
    await vi.waitFor(() => expect(listed("tasks.update")).toHaveLength(1));
    expect(listed("tasks.update")[0][1]).toEqual({ task_id: "task-12", status: "in_review" });
  });

  // The card moves now and the column repaints from the push.
  it("moves the card before the bridge has answered", async () => {
    let settle;
    const listAnswer = { tasks: [task({ number: 12, id: "task-12", status: "backlog" })] };
    call = vi.fn((method) => {
      if (method === "tasks.list") return Promise.resolve(listAnswer);
      return new Promise((resolve) => { settle = resolve; });
    });
    await mount({ view: "board" });
    drop("task-12", "done");
    await vi.waitFor(() => expect(cardsIn("done")).toEqual(["task-12"]));
    await vi.waitFor(() => expect(listed("tasks.update")).toHaveLength(1));
    settle({});
    const updateIndex = call.mock.calls.findIndex(([method]) => method === "tasks.update");
    await call.mock.results[updateIndex].value;
  });

  // A card that stayed put with no word is a card the reader will drag again.
  it("puts a refused move back and says why", async () => {
    const listAnswer = { tasks: [task({ number: 12, id: "task-12", status: "backlog" })] };
    call = vi.fn(async (method) => {
      if (method === "tasks.list") return listAnswer;
      throw new Error("task is closed");
    });
    await mount({ view: "board" });
    drop("task-12", "done");
    await vi.waitFor(() => expect(notifyError).toHaveBeenCalledWith("Could not move this task", "task is closed"));
    expect(cardsIn("backlog")).toEqual(["task-12"]);
  });

  // Dragging is one way to move a card; the arrow keys are the other.
  it("moves a focused card with the arrow keys", async () => {
    await mount({ view: "board" });
    call.mockClear();
    const card = host.querySelector('.task-card[data-task="task-12"]');
    card.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await vi.waitFor(() => expect(listed("tasks.update")).toHaveLength(1));
    expect(listed("tasks.update")[0][1]).toEqual({ task_id: "task-12", status: "in_review" });
  });

  it("stops at the ends rather than wrapping", async () => {
    const listAnswer = { tasks: [task({ number: 12, id: "task-12", status: "backlog" })] };
    call = vi.fn(async (method) => (method === "tasks.list" ? listAnswer : {}));
    await mount({ view: "board" });
    call.mockClear();
    host.querySelector(".task-card").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    await flush();
    expect(listed("tasks.update")).toEqual([]);
  });
});

describe("the two presses", () => {
  it("opens the picker on a row, standing on that task's current assignee", async () => {
    await mount();
    host.querySelector("[data-task-assign]").click();
    await flush();
    const [given] = openAssigneePicker.mock.calls[0];
    expect([given.task.id, given.current]).toEqual(["task-12", "user"]);
  });

  it("offers the project's agents to the picker, grouped by workspace", async () => {
    await mount();
    host.querySelector("[data-task-assign]").click();
    await flush();
    const [given] = openAssigneePicker.mock.calls[0];
    expect(given.options.map((one) => one.id)).toEqual([
      "none", "user", "project_agent", "agent:agent-1", "new_agent:ws-1", "new_workspace",
    ]);
  });

  it("opens the picker from a card too", async () => {
    await mount({ view: "board" });
    host.querySelector(".task-card [data-task-assign]").click();
    await flush();
    expect(openAssigneePicker).toHaveBeenCalled();
  });

  it.each(["list", "board"])("keeps the %s assignment press working when its cached destination disappears and returns", async (view) => {
    const workspace = { id: "ws-history", workspace_id: "ws-history", name: "History", projectKey: PROJECT_KEY };
    const localFeed = { workspaces: [workspace], items: [] };
    const held = task({
      assignee: { kind: "agent", agent_id: "agent-history" },
      identities: { "agent-history": { agent_id: "agent-history", name: "Finisher", ordinal: 1,
        workspace_id: workspace.id, workspace_name: workspace.name, provider: "codex_app_server", available: true } },
    });
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: [held], columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    await mount({ view, feed: () => localFeed });

    for (const available of [true, false, true]) {
      localFeed.workspaces = available ? [workspace] : [];
      pane.feedMoved();
      await flush();
      const link = host.querySelector(".task-assignee-link");
      expect(Boolean(link)).toBe(available);
      const priorCalls = openAssigneePicker.mock.calls.length;
      if (link) {
        const navigates = link.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true }));
        expect(navigates).toBe(true);
        expect(openAssigneePicker).toHaveBeenCalledTimes(priorCalls);
      }
      const press = host.querySelector("[data-task-assign]");
      // The deleted agent's name and harness are nested inside this button.
      // Clicking the label must work just like clicking the button itself.
      (press.querySelector(".task-assignee") || press).click();
      expect(openAssigneePicker).toHaveBeenCalledTimes(priorCalls + 1);
      expect(openAssigneePicker.mock.lastCall[0].task.id).toBe(held.id);
    }
  });

  // #57: filing happens IN the tab. No dialog, no navigation — the list the
  // task is being filed against stays on screen while it is written.
  it("opens the composer in place, above the list and over nothing", async () => {
    await mount();
    expect(composer()).toBeNull();
    await openComposer();
    expect(composer()).not.toBeNull();
    expect(document.querySelector(".modal, #task-new-scrim")).toBeNull();
    expect(titles()).toEqual(["Kanban drag does not persist", "Board is unreadable on a phone"]);
    expect(document.activeElement).toBe(host.querySelector(".task-compose-title"));
  });

  it("files what was typed, as tasks.create params", async () => {
    await mount();
    await openComposer();
    typeIn(".task-compose-title", "Kanban drag");
    typeIn("#task-new-body", "It does not persist.");
    call.mockClear();
    await fileIt();
    expect(listed("tasks.create")[0][1]).toMatchObject({
      project_id: "proj-1",
      title: "Kanban drag",
      body: "It does not persist.",
    });
    expect(listed("tasks.create")[0][1]).not.toHaveProperty("made_by_agent");
  });

  it("shuts once it has filed, and leaves the list behind it", async () => {
    await mount();
    await openComposer();
    typeIn(".task-compose-title", "Kanban drag");
    await fileIt();
    expect(composer()).toBeNull();
  });

  it("refuses an untitled task without asking the bridge", async () => {
    await mount();
    await openComposer();
    call.mockClear();
    await fileIt();
    expect(listed("tasks.create")).toHaveLength(0);
    expect(host.querySelector(".task-compose-error").hidden).toBe(false);
  });

  // A bridge that predates `tasks.create`'s assignee drops it at the facade
  // and answers ok, so the task is filed and nobody holds it. Assignment is
  // dispatch, so that is work the reader believes has started and has not.
  it("says when a filed task's assignee went nowhere", async () => {
    await mount();
    call.mockImplementation(async (method) =>
      method === "tasks.create" ? { task: task({ id: "task-1", number: 12, assignee: null }) } : { tasks: [] });
    await openComposer();
    typeIn(".task-compose-title", "Kanban drag");
    await chooseAssignee("project_agent");
    await fileIt();
    expect(notifyError).toHaveBeenCalledWith(
      "Filed #12 — but nobody was assigned",
      expect.stringContaining("created unassigned and nothing was started"),
    );
  });

  // #57: the composer is in the slot between the bar and the rows, outside the
  // body — so a push that repaints the list cannot take away a form somebody
  // is typing into.
  it("keeps the composer, its text and its focus through a push", async () => {
    await mount();
    await openComposer();
    const title = typeIn(".task-compose-title", "Half written");
    title.focus();
    call.mockImplementation(async (method) =>
      method === "tasks.list" ? { tasks: [task({ id: "task-9", number: 9, title: "Fresh" })], columns: columns() } : {});
    watchers[0].refresh();
    await flush();
    expect(titles()).toEqual(["Fresh"]);
    expect(host.querySelector(".task-compose-title")).toBe(title);
    expect(title.value).toBe("Half written");
    expect(document.activeElement).toBe(title);
  });

  // The list is keyed, so a filed task is one row INSERTED rather than a
  // repaint — which is the whole point of filing in place: you watch the thing
  // you just wrote appear in the list you wrote it against.
  //
  // The re-read behind it is held open here, so what is asserted is the
  // OPTIMISTIC row and not the one a refresh would have painted anyway.
  const fileWithTheReadHeldOpen = async () => {
    await openComposer();
    typeIn(".task-compose-title", "Just filed");
    call.mockImplementation((method) =>
      method === "tasks.create"
        ? Promise.resolve({ task: task({ id: "task-20", number: 20, title: "Just filed" }) })
        : new Promise(() => {}), // the list never answers
    );
    await fileIt();
  };

  it("puts the new row in the list without rebuilding the rows around it", async () => {
    await mount();
    const kept = host.querySelector('[data-task="task-12"]');
    await fileWithTheReadHeldOpen();
    expect(host.querySelector('[data-task-group="rest"] .task-title').textContent).toBe("Just filed");
    expect(host.querySelector('[data-task="task-12"]')).toBe(kept);
  });

  it("puts the focus on the row it just made", async () => {
    await mount();
    await fileWithTheReadHeldOpen();
    expect(document.activeElement.closest(".task-row")?.dataset.task).toBe("task-20");
  });

  // Cancelling gives the keyboard back to the press that opened the form —
  // closing a focused subtree otherwise leaves the focus nowhere.
  it("gives the focus back to New task when nothing was filed", async () => {
    await mount();
    await openComposer();
    host.querySelector("[data-compose-cancel]").click();
    await flush();
    expect(composer()).toBeNull();
    expect(document.activeElement).toBe(newPress());
  });

  it("opens one composer, however many times the press is pressed", async () => {
    await mount();
    await openComposer();
    typeIn(".task-compose-title", "Half written");
    await openComposer();
    expect(host.querySelectorAll(".task-compose")).toHaveLength(1);
    expect(host.querySelector(".task-compose-title").value).toBe("Half written");
  });

  // #57: the bytes go up BEFORE the task does, the way a chat attachment
  // does — and against the PROJECT, because a task being created has no
  // conversation to attach to (`thread.attach` would answer "unknown
  // conversation owner").
  const drop = (file) => {
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: { files: [file] } });
    host.querySelector(".task-compose").dispatchEvent(event);
  };

  it("sends a dropped file to tasks.attach for this project, then files it with the task", async () => {
    await mount();
    await openComposer();
    call.mockImplementation(async (method) => {
      if (method === "tasks.attach") return { name: "shot.png", path: ".build/attachments/abc-shot.png", mime: "image/png", size: 3 };
      if (method === "tasks.create") return { task: task({ id: "task-20", number: 20, attachments: [{ path: ".build/attachments/abc-shot.png" }] }) };
      return { tasks: [] };
    });
    drop(new File(["png"], "shot.png", { type: "image/png" }));
    await flush();
    expect(listed("tasks.attach")[0][1]).toMatchObject({ project_id: "proj-1", filename: "shot.png" });
    expect(host.querySelector(".composer-tray").hidden).toBe(false);

    typeIn(".task-compose-title", "Kanban drag");
    await fileIt();
    expect(listed("tasks.create")[0][1].attachments).toEqual([
      { name: "shot.png", path: ".build/attachments/abc-shot.png", mime: "image/png", size: 3 },
    ]);
    expect(notifyError).not.toHaveBeenCalled();
  });

  // #57, the other half of being honest about a bridge that cannot carry
  // files: do not offer the press at all. The apology below is for a bridge
  // that claimed it could and then did not.
  it("offers the paperclip on a bridge that carries files, and none on one that does not", async () => {
    await mount();
    await openComposer();
    expect(host.querySelector(".composer-attach")).not.toBeNull();

    pane.dispose();
    carriesAttachments = false;
    document.body.innerHTML = '<div id="pane"></div>';
    host = document.querySelector("#pane");
    await mount();
    await openComposer();
    expect(host.querySelector(".composer-attach")).toBeNull();
    expect(host.querySelector(".composer-tray")).toBeNull();
    // The composer is otherwise the same composer.
    expect(host.querySelector(".task-compose-title")).not.toBeNull();
    expect(host.querySelector("[data-compose-file]")).not.toBeNull();
  });

  // The v1 facade drops a field the bridge predates rather than refusing it,
  // so filing with files on today's bridge answers ok with none. The
  // screenshot was usually the reason for filing, so that is said out loud.
  it("says when the files a bridge cannot carry went nowhere", async () => {
    await mount();
    await openComposer();
    call.mockImplementation(async (method) => {
      if (method === "tasks.attach") return { name: "shot.png", path: ".build/attachments/abc-shot.png", mime: "image/png", size: 3 };
      if (method === "tasks.create") return { task: task({ id: "task-20", number: 20 }) }; // no attachments came back
      return { tasks: [] };
    });
    drop(new File(["png"], "shot.png", { type: "image/png" }));
    await flush();
    typeIn(".task-compose-title", "Kanban drag");
    await fileIt();
    expect(notifyError).toHaveBeenCalledWith(
      "Filed #20 — but the file did not go with it",
      expect.stringContaining("cannot carry files on a task"),
    );
  });

  it("says nothing of the sort when the assignee landed", async () => {
    await mount();
    call.mockImplementation(async (method) =>
      method === "tasks.create"
        ? { task: task({ id: "task-1", number: 12, assignee: { kind: "project_agent" } }) }
        : { tasks: [] });
    await openComposer();
    typeIn(".task-compose-title", "Kanban drag");
    await chooseAssignee("project_agent");
    await fileIt();
    expect(notifyError).not.toHaveBeenCalled();
  });
});

describe("the filter bar, mounted once", () => {
  // #43. The bar is not redrawn — ever. Not on a push, not on a feed move, not
  // on a re-read that changes the list, not on a filter change, and not when
  // switching to the board and back. The maintainer: "The inputs/selects
  // really shouldn't be redrawing ever."
  // Every control on the bar: the four menus' presses and the Clear beside
  // them. The search boxes and rows inside a menu are mounted with it.
  const controls = () => [
    ...host.querySelectorAll(".task-filters .fmenu-press"),
    host.querySelector("[data-task-filter-clear]"),
  ];

  it("is the same DOM nodes through ten paints and a push", async () => {
    await mount();
    const before = controls();
    expect(before).toHaveLength(5); // four filters and the Clear press
    for (let i = 0; i < 5; i++) pane.feedMoved();
    call.mockImplementation(async (method) =>
      method === "tasks.list" ? { tasks: [task({ id: "task-9", number: 9, title: "Fresh" })], columns: columns() } : {});
    for (let i = 0; i < 5; i++) {
      watchers[0].refresh();
      await flush();
    }
    expect(titles()).toEqual(["Fresh"]);
    expect(controls()).toEqual(before);
  });

  it("keeps a focused control's focus and value through a body repaint", async () => {
    await mount();
    await chooseFilter("label", "bug");
    const press = menuPress("label");
    press.focus();
    call.mockImplementation(async (method) =>
      method === "tasks.list" ? { tasks: [task({ id: "task-9", number: 9, title: "Fresh", labels: ["bug"] })], columns: columns() } : {});
    watchers[0].refresh();
    await flush();
    expect(titles()).toEqual(["Fresh"]);
    expect(menuPress("label")).toBe(press);
    expect(document.activeElement).toBe(press);
    expect(press.textContent.trim()).toBe("bug");
  });

  // The whole reason the bar is mounted once: a menu the reader has OPEN, with
  // a query half typed into it and a row walked to, is state that lives in the
  // DOM — and a push about a task must not touch any of it (#43, #44).
  it("keeps an open menu open, searched and walked through a body repaint", async () => {
    await mount();
    await openMenu("label");
    const search = menu("label").querySelector(".fmenu-search");
    search.value = "bu";
    search.dispatchEvent(new Event("input"));
    await flush();
    search.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowDown", bubbles: true }));
    const walked = menu("label").querySelector(".fmenu-row.is-active");
    call.mockImplementation(async (method) =>
      method === "tasks.list" ? { tasks: [task({ id: "task-9", number: 9, title: "Fresh", labels: ["bug"] })], columns: columns() } : {});
    watchers[0].refresh();
    await flush();
    expect(titles()).toEqual(["Fresh"]);
    expect(menuPress("label").getAttribute("aria-expanded")).toBe("true");
    expect(search.value).toBe("bu");
    expect(document.activeElement).toBe(search);
    expect(menu("label").querySelector(".fmenu-row.is-active")).toBe(walked);
  });

  // The options are the one thing about a control that a read may change. They
  // are patched by value: a label nobody had filed before is an inserted
  // `<option>`, and the select around it is the select it already was.
  it("adds a newly filed label to the options without re-creating the menu", async () => {
    await mount();
    const press = menuPress("label");
    await openMenu("label");
    // A multi menu offers no "Any label" row: an empty selection already is
    // one, so the rows are the labels themselves.
    expect(menuRows("label").map((one) => one.dataset.value)).toEqual(["bug"]);
    const kept = menuRows("label")[0];
    await trackerCache.writeTasksRecord("dev-1", "proj-1", {
      tasks: [task({ number: 13, id: "task-13", labels: ["bug", "spa"] })],
      columns: columns(),
    });
    // A narrowed read is what sends the tab back to the cache for the whole
    // list, which is where the menus are built from.
    await chooseFilter("state", "closed");
    await openMenu("label");
    expect(menuRows("label").map((one) => one.dataset.value)).toEqual(["bug", "spa"]);
    expect(menuPress("label")).toBe(press);
    expect(menuRows("label")[0]).toBe(kept);
  });

  // The Clear press is a control on the bar like the other four, so it is
  // hidden rather than taken away: removing a button takes the focus on it.
  it("hides the Clear press rather than removing it", async () => {
    await mount();
    const press = host.querySelector("[data-task-filter-clear]");
    expect(press.hidden).toBe(true);
    await chooseFilter("label", "bug");
    expect(press.hidden).toBe(false);
    expect(host.querySelector("[data-task-filter-clear]")).toBe(press);
  });

  it("uses one control for all four filters, and no native select", async () => {
    await mount();
    expect(host.querySelectorAll(".task-filters select")).toHaveLength(0);
    expect([...host.querySelectorAll("[data-filter-menu]")].map((one) => one.dataset.filterMenu))
      .toEqual(["state", "status", "assignee", "label"]);
  });

  it("survives a switch to the board and back", async () => {
    await mount();
    const before = controls();
    host.querySelector('[data-task-view="board"]').click();
    await flush();
    host.querySelector('[data-task-view="list"]').click();
    await flush();
    expect(controls()).toEqual(before);
  });
});

describe("the push", () => {
  // The feed moves whenever any agent's state does, and most of those moves
  // change nothing on this tab. Nothing about a filter can move with one.
  it("does not redraw or drop focus when the feed moves without changing what it shows", async () => {
    await mount();
    const filter = host.querySelector(".fmenu-press");
    filter.focus();
    pane.feedMoved();
    pane.feedMoved();
    expect(host.querySelector(".fmenu-press")).toBe(filter);
    expect(document.activeElement).toBe(filter);
  });

  // The rows are keyed, so a re-read that leaves a task where it was leaves
  // its row the element it was — and only the row that changed is written.
  it("keeps the rows a re-read did not change, and patches the one it did", async () => {
    await mount();
    const kept = host.querySelector('[data-task="task-11"]');
    const changed = host.querySelector('[data-task="task-12"]');
    call.mockImplementation(async (method) =>
      method === "tasks.list"
        ? {
            tasks: [
              task({ number: 12, id: "task-12", title: "Renamed", status: "in_progress", labels: ["bug"], assignee: { kind: "user" } }),
              task({ number: 11, id: "task-11", title: "Board is unreadable on a phone", state: "closed", status: "done" }),
            ],
          }
        : {});
    watchers[0].refresh();
    await flush();
    expect(host.querySelector('[data-task="task-11"]')).toBe(kept);
    expect(host.querySelector('[data-task="task-12"]')).toBe(changed);
    expect(changed.querySelector(".task-title").textContent).toBe("Renamed");
  });

  it("subscribes this project for tasks, and nothing else", async () => {
    await mount();
    expect(watchers.map((one) => [one.entity, one.deviceId, one.kinds])).toEqual([["proj-1", "dev-1", ["tasks"]]]);
  });

  it("re-reads the list when the subscription says something moved", async () => {
    await mount();
    call.mockClear();
    watchers[0].refresh();
    await flush();
    expect(listed("tasks.list")).toHaveLength(1);
  });

  // Every kind in one subscribe shares that call's fate, and a refused one
  // takes this device's other subscriptions with it — so a bridge that does
  // not carry tasks is asked for nothing rather than for a word it will
  // refuse. The tab keeps its own reads and the ordered pass keeps its cache.
  it("asks a bridge that does not carry tasks for no kind at all", async () => {
    carriedKinds = ["state", "thread", "git", "files", "terminals"];
    await mount();
    expect(watchers.map((one) => one.kinds)).toEqual([[]]);
  });

  it("takes its subscription down with it", async () => {
    await mount();
    pane.dispose();
    pane = null;
    expect(watchers[0].disposed).toBe(true);
  });
});

// #24, the tab's half. The list and the board are two drawings of one read, so
// both keep what they have when the session under them dies.
describe("a read that fails because the session dropped", () => {
  const WENT = "your device went offline";
  const CACHED = [task({ number: 9, id: "task-9", title: "From the cache", status: "ready" })];

  const refusing = (message) =>
    vi.fn(async (method) => {
      if (method === "tasks.list") throw new Error(message);
      return {};
    });

  const note = () => host.querySelector(".read-wait")?.textContent ?? null;

  const withCache = async (message) => {
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: CACHED, columns: columns() });
    call = refusing(message);
    await mount();
  };

  it("keeps the cached list and says nothing", async () => {
    await withCache(WENT);
    expect(titles()).toEqual(["From the cache"]);
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("marks when that list was read, while the machine is being reconnected to", async () => {
    await withCache(WENT);
    expect(note()).toContain("reconnecting");
  });

  // A board is the same read laid out differently, so it keeps its cards too.
  it("keeps the board's cards just the same", async () => {
    await trackerCache.writeTasksRecord("dev-1", "proj-1", { tasks: CACHED, columns: columns() });
    call = refusing(WENT);
    await mount({ view: "board" });
    expect(cardsIn("ready")).toEqual(["task-9"]);
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("reads again when the machine is back, with nothing polled in between", async () => {
    await withCache(WENT);
    expect(listed("tasks.list")).toHaveLength(1);
    call.mockImplementation(async (method) =>
      method === "tasks.list" ? { tasks: [task({ number: 12, id: "task-12", title: "Kanban drag does not persist" })] } : {},
    );
    await reconnect();
    expect(listed("tasks.list")).toHaveLength(2);
    expect(titles()).toEqual(["Kanban drag does not persist"]);
    expect(note()).toBeNull();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("still says a refusal out loud", async () => {
    await withCache("project_id is required");
    expect(notifyError).toHaveBeenCalledWith("Could not read this project's tasks", "project_id is required");
  });

  it("waits with nothing on screen, then says so when the retry fails too", async () => {
    call = refusing(WENT);
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
    await reconnect();
    expect(notifyError).toHaveBeenCalledWith("Could not read this project's tasks", WENT);
  });
});

// #33. The maintainer, deciding the question #28 raised: "If closed means
// done we shouldn't show them in the default view."
describe("what the tab opens on", () => {
  const OPEN_AND_CLOSED = [
    task({ number: 12, id: "task-12", title: "Still open", state: "open", status: "in_progress" }),
    task({ number: 11, id: "task-11", title: "Finished with", state: "closed", status: "done" }),
  ];

  /** The bridge, narrowing the way the real one does. */
  const listing = () =>
    vi.fn(async (method, params) => {
      if (method !== "tasks.list") return {};
      const wanted = params.state;
      return { tasks: wanted ? OPEN_AND_CLOSED.filter((one) => one.state === wanted) : OPEN_AND_CLOSED };
    });

  const chooseState = (value) => chooseFilter("state", value);

  it("asks the bridge for open tasks, and draws only those", async () => {
    call = listing();
    await mount();
    expect(listed("tasks.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
    expect(titles()).toEqual(["Still open"]);
  });

  it("offers Open and closed, and Closed, and shows them when asked", async () => {
    call = listing();
    await mount();
    await openMenu("state");
    const options = menuRows("state").map((one) => one.dataset.value);
    expect(options).toEqual(["", "open", "closed"]);

    await chooseState("");
    expect(titles()).toEqual(["Still open", "Finished with"]);

    await chooseState("closed");
    expect(titles()).toEqual(["Finished with"]);
  });

  // The row lost its state dot with the dots (#28), so once a closed task is
  // on screen this is what keeps it from reading as an open one.
  it("marks a closed row Closed once one is on screen", async () => {
    call = listing();
    await mount();
    await chooseState("closed");
    const row = host.querySelector(".task-row");
    expect(row.querySelector(".task-closed").textContent).toBe("Closed");
    expect(row.querySelector(".task-row-facts").firstElementChild.className).toBe("task-closed");
  });

  it("draws no such chip on the open rows it opens with", async () => {
    call = listing();
    await mount();
    expect(host.querySelector(".task-closed")).toBeNull();
  });

  // The default is not a narrowing the reader made, so there is nothing to
  // clear and an empty project is not a filter that matched nothing.
  it("offers no Clear until the reader narrows something themselves", async () => {
    call = listing();
    await mount();
    expect(clearPress()).toBeNull();
    await chooseState("closed");
    expect(clearPress()).not.toBeNull();
  });

  it("goes back to open tasks on Clear, not to everything", async () => {
    call = listing();
    await mount();
    await chooseState("");
    expect(titles()).toEqual(["Still open", "Finished with"]);
    clearPress().click();
    await flush();
    expect(titles()).toEqual(["Still open"]);
    expect(clearPress()).toBeNull();
  });

  // A project with nothing in it is at its first state, not looking at a
  // filter that matched none.
  it("still says the project is empty rather than blaming the filter", async () => {
    call = vi.fn(async () => ({ tasks: [] }));
    await mount();
    expect(host.querySelector(".task-empty h2").textContent).toBe("No tasks yet");
  });
});
