/** @vitest-environment jsdom */
// The Issues tab, mounted: the list, the board, the filters, and the two moves
// that change something.
//
// Cache-first — the project's whole list is on disk, so the tab paints before
// the bridge is asked — and filter-as-param: every control is an `issues.list`
// param rather than a pass over what happens to be in hand.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

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
    // #57: whether this bridge can carry files on an issue. A case that sets
    // it false is an older bridge answering, not another machine.
    issues: { attachments: carriesAttachments },
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
const EVERY_KIND = ["state", "thread", "git", "files", "terminals", "issues"];
let carriedKinds = EVERY_KIND;
/** Whether the bridge under test has `issues.attach` (1.8). */
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

let host, call, pane, cache, trackerCache, mountIssuesPane;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const mount = async (over = {}) => {
  pane = mountIssuesPane(host, {
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
  const press = host.querySelector("[data-issue-filter-clear]");
  return press && !press.hidden ? press : null;
};
/** One filter menu, by the filter it writes (#44). The native selects went;
 *  every filter is now the same custom control. */
/** The inline composer (#57), and the press that opens it. */
const newPress = () => host.querySelector("[data-issue-new]");
const composer = () => host.querySelector(".issue-compose");
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
  const select = host.querySelector("#issue-new-assignee");
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
const titles = () => [...host.querySelectorAll(".issue-title")].map((one) => one.textContent);
const columnNames = () => [...host.querySelectorAll(".issue-column-head h3")].map((one) => one.textContent);
const cardsIn = (columnId) =>
  [...host.querySelectorAll(`[data-column="${columnId}"] .issue-card`)].map((card) => card.dataset.issue);

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
  ({ mountIssuesPane } = await import("../src/core/trackerIssuesPane.js"));
  call = vi.fn(async (method) => {
    if (method === "issues.list") {
      return {
        issues: [
          issue({ number: 12, id: "issue-12", status: "in_progress", labels: ["bug"], assignee: { kind: "user" } }),
          issue({ number: 11, id: "issue-11", title: "Board is unreadable on a phone", state: "closed", status: "done" }),
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
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", {
      issues: [issue({ number: 9, id: "issue-9", title: "From the cache" })],
      columns: columns(),
    });
    // The cached paint lands before the RPC settles: the call is held open.
    let answer;
    call = vi.fn((method) => (method === "issues.list" ? new Promise((resolve) => { answer = resolve; }) : Promise.resolve({})));
    await mount();
    expect(titles()).toEqual(["From the cache"]);
    answer({ issues: [issue({ number: 12, id: "issue-12", title: "From the bridge" })] });
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

    await trackerCache.writeIssuesQueryRecord(
      "dev-1",
      "proj-1",
      { project_id: "proj-1", state: "open" },
      trackerCache.issuesRecord([issue({ id: "issue-cache", number: 30, title: "Announced from cache" })], columns()),
    );
    await flush();

    expect(titles()).toEqual(["Announced from cache"]);
  });
});

describe("the Dashboard", () => {
  const dashboardRows = (section) => [...host.querySelectorAll(`[data-dashboard-section="${section}"] .issue-dashboard-row`)]
    .map((row) => row.dataset.issue);

  it("defaults the project to Dashboard and paints all sections from cached records while the payload is absent", async () => {
    const working = issue({ id: "working", number: 4, title: "Write release notes", assignee: { kind: "agent", agent_id: "agent-1" } });
    const review = issue({ id: "review", number: 3, title: "Review patch", status: "in_review" });
    const done = issue({ id: "done", number: 2, title: "Shipped fix", status: "done", state: "closed", links: { commits: ["abc123def456"] } });
    const movedAt = new Date(Date.now() - 60_000).toISOString();
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [working, review, done], columns: columns() });
    await trackerCache.writeIssueRecord("dev-1", "proj-1", done.id, trackerCache.issueRecord(done, [
      { type: "event", kind: "moved", at: movedAt, payload: { to: "done" } },
    ]));
    const { threadCacheAddress } = await import("../src/core/conversationCache.js");
    const thread = threadCacheAddress({ deviceId: "dev-1", entityId: "run-1", agentId: "agent-1", conversationId: "conv-1" });
    await cache.writeCached(thread, { items: [{ type: "message", data: { role: "agent", body: "Writing summary\nNext line" } }] });
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true, conversation_id: "conv-1", name: "Writer" }] }] };
    call = vi.fn(() => new Promise(() => {}));
    await mount({ feed: () => activeFeed, defaultView: undefined });

    expect(host.querySelector('[data-issue-view="dashboard"]').getAttribute("aria-pressed")).toBe("true");
    expect(clearPress()).toBeNull();
    expect(dashboardRows("inProgress")).toEqual(["working"]);
    expect(dashboardRows("needsYou")).toEqual(["review"]);
    expect(dashboardRows("doneToday")).toEqual(["done"]);
    expect(host.querySelector('[data-dashboard-section="inProgress"] .issue-dashboard-detail').textContent)
      .toContain("Writing summary");
    expect(host.querySelector('[data-dashboard-section="doneToday"] .issue-dashboard-detail').textContent)
      .toContain("abc123def456");
  });

  it("redraws from real conversation and detail cache writes, with no bridge answer", async () => {
    const working = issue({ id: "working", number: 4, title: "Work", assignee: { kind: "agent", agent_id: "agent-1" } });
    const done = issue({ id: "done", number: 3, title: "Done", status: "done", state: "closed" });
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [working, done], columns: columns() });
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true, conversation_id: "conv-1" }] }] };
    call = vi.fn(() => new Promise(() => {}));
    await mount({ feed: () => activeFeed, defaultView: undefined });
    expect(dashboardRows("inProgress")).toEqual(["working"]);
    expect(dashboardRows("doneToday")).toEqual([]);

    const { threadCacheAddress } = await import("../src/core/conversationCache.js");
    await cache.writeCached(threadCacheAddress({ deviceId: "dev-1", entityId: "run-1", agentId: "agent-1", conversationId: "conv-1" }), {
      items: [{ type: "message", data: { role: "agent", body: "Cached new activity" } }],
    });
    await trackerCache.writeIssueRecord("dev-1", "proj-1", done.id, trackerCache.issueRecord(done, [
      { type: "event", kind: "moved", at: new Date().toISOString(), payload: { to: "done" } },
    ]));
    await flush();

    expect(host.querySelector('[data-dashboard-section="inProgress"] .issue-dashboard-detail').textContent)
      .toContain("Cached new activity");
    expect(dashboardRows("doneToday")).toEqual(["done"]);
    expect(host.querySelector('[data-dashboard-section="doneToday"] .issue-dashboard-link').getAttribute("href"))
      .toContain("done");
  });

  it("derives Needs you from cached unread comments and excludes questions, Done, and closed issues", async () => {
    const questioned = issue({ id: "questioned", number: 4, title: "Question only" });
    const unread = issue({ id: "unread", number: 3, title: "Unread comment", watched: true });
    const done = issue({ id: "done", number: 2, title: "Done", status: "done", assignee: { kind: "user" } });
    const closed = issue({ id: "closed", number: 1, title: "Closed", state: "closed", status: "in_review" });
    const issues = [questioned, unread, done, closed];
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });
    for (const one of issues) {
      await trackerCache.writeIssueRecord("dev-1", "proj-1", one.id, trackerCache.issueRecord(one, [{
        type: "comment", id: "ic-02", author: { kind: "agent", agent_id: "agent-1" }, body: "Could you review this?",
      }]));
    }
    const inboxFeed = { ...feed, items: [...feed.items, ...[unread, done, closed].map((one) => ({
      kind: "tracker_issue", projectKey: PROJECT_KEY, issue_id: one.id, unread: 1,
    }))] };
    call = vi.fn(() => new Promise(() => {}));
    pane = mountIssuesPane(host, {
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

    const read = { ...unread, read_through: "ic-02" };
    await trackerCache.writeIssueRecord("dev-1", "proj-1", unread.id, trackerCache.issueRecord(read, [{
      type: "comment", id: "ic-02", author: { kind: "agent", agent_id: "agent-1" }, body: "Could you review this?",
    }]));
    await vi.waitFor(() => expect(dashboardRows("needsYou")).toEqual([]));
  });
});

describe("the list", () => {
  it("groups cached working and review issues ahead of the remaining rows and collapses each group", async () => {
    const issues = [
      issue({ id: "rest", number: 4, title: "Other work" }),
      issue({ id: "review", number: 3, title: "Review this", status: "in_review" }),
      issue({ id: "working", number: 2, title: "Agent is working", assignee: { kind: "agent", agent_id: "agent-1" } }),
      issue({ id: "mine", number: 1, title: "Assigned to me", assignee: { kind: "user" } }),
    ];
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    const activeFeed = { ...feed, items: [{ ...feed.items[0], agents: [{ id: "agent-1", working: true }] }] };
    await mount({ feed: () => activeFeed });

    expect(titles()).toEqual(["Agent is working", "Review this", "Assigned to me", "Other work"]);
    expect([...host.querySelectorAll(".issue-group-heading")].map((one) => one.textContent.trim()))
      .toEqual(["In progress with an agent1", "Needs you2", "Other issues1"]);
    host.querySelector('[data-issue-group-toggle="working"]').click();
    await vi.waitFor(() => expect(host.querySelector('[data-issue-group="working"] .issue-rows').hidden).toBe(true));
    expect(host.querySelector('[data-issue-group-toggle="working"]').getAttribute("aria-expanded")).toBe("false");
  });

  it("moves a row into Needs you after a real issue-detail cache announcement", async () => {
    const one = issue({ id: "watched", number: 8, title: "Watched work", watched: true });
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [one], columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    const inboxFeed = { ...feed, items: [...feed.items, {
      kind: "tracker_issue", projectKey: PROJECT_KEY, issue_id: one.id, unread: 1,
    }] };
    await mount({ feed: () => inboxFeed });
    expect(host.querySelector('[data-issue-group="rest"] [data-issue="watched"]')).not.toBeNull();

    await trackerCache.writeIssueRecord("dev-1", "proj-1", one.id, trackerCache.issueRecord(one, [{
      type: "comment", id: "ic-02", author: { kind: "agent", agent_id: "agent-1" }, body: "Please take a look",
    }]));
    await vi.waitFor(() => expect(host.querySelector('[data-issue-group="needsYou"] [data-issue="watched"]')).not.toBeNull());
  });

  it("shows cached issues a page at a time and appends the next page without a bridge answer", async () => {
    const issues = Array.from({ length: 54 }, (_, index) => issue({
      id: `issue-${54 - index}`, number: 54 - index, title: `Cached ${54 - index}`,
    }));
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });
    call = vi.fn(() => new Promise(() => {}));
    await mount();

    expect(titles()).toHaveLength(25);
    expect(titles()[0]).toBe("Cached 54");
    expect(host.querySelector(".issue-paging").textContent).toContain("25 of 54");
    host.querySelector("[data-issue-more]").click();
    await vi.waitFor(() => expect(titles()).toHaveLength(50));
    expect(titles()[25]).toBe("Cached 29");
    host.querySelector("[data-issue-more]").click();
    await vi.waitFor(() => expect(titles()).toHaveLength(54));
    expect(host.querySelector("[data-issue-more]")).toBeNull();
  });

  it("keeps an expanded page after a late cache write", async () => {
    const issues = Array.from({ length: 30 }, (_, index) => issue({
      id: `issue-${30 - index}`, number: 30 - index, title: `Cached ${30 - index}`,
    }));
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });
    let answer;
    call = vi.fn((method) => method === "issues.list" ? new Promise((resolve) => { answer = resolve; }) : Promise.resolve({}));
    await mount();
    host.querySelector("[data-issue-more]").click();
    answer({ issues: [...issues, issue({ id: "issue-31", number: 31, title: "New issue" })] });
    await flush();
    expect(titles()).toHaveLength(31);
    expect(titles()[0]).toBe("New issue");
  });

  it("draws a row per issue, newest number first", async () => {
    await mount();
    expect([...host.querySelectorAll(".issue-number")].map((one) => one.textContent)).toEqual(["#12", "#11"]);
  });

  // #28: the number and the title have line one to themselves, and everything
  // else is on line two in one order.
  it("puts the number and the title on the first line, alone", async () => {
    await mount();
    const line = host.querySelector(".issue-row .issue-row-open");
    expect([...line.children].map((one) => one.classList[0])).toEqual(["issue-number", "issue-title"]);
    expect(line.querySelector(".issue-title").textContent).toBe("Kanban drag does not persist");
  });

  it("puts the column, the age, the labels and the holder on the second", async () => {
    await mount();
    const facts = host.querySelector(".issue-row .issue-row-facts");
    // The labels are one group of muted words rather than a pill each (#45).
    expect([...facts.children].map((one) => one.classList[0]))
      .toEqual(["issue-status", "issue-age", "issue-labels", "issue-assign"]);
    expect(facts.querySelector(".issue-status").textContent).toBe("In progress");
    expect(facts.querySelector(".issue-labels .issue-label").textContent).toBe("bug");
    expect(facts.querySelector(".issue-assign").textContent.trim()).toBe("You");
  });

  // The coloured state dot went with the dots (#28). The board card, the
  // agent's entry and the issue's own page still carry it.
  it("draws no state dot and no separators on a row", async () => {
    await mount();
    const rows = host.querySelectorAll(".issue-row");
    expect([...rows].some((one) => one.querySelector(".issue-state"))).toBe(false);
    expect([...rows].some((one) => one.querySelector(".issue-sep"))).toBe(false);
  });

  // An unheld row says nothing rather than saying "Unassigned" — but the press
  // is still there, offering the word it would act on.
  it("says nothing about an assignee nobody is, and still offers the press", async () => {
    await mount();
    const unheld = host.querySelectorAll(".issue-row")[1];
    expect(unheld.textContent).not.toContain("Unassigned");
    expect(unheld.querySelector(".issue-assignee")).toBeNull();
    expect(unheld.querySelector(".issue-assign").textContent.trim()).toBe("Assign");
  });

  it("opens each row on that issue's page, on the machine the project is on", async () => {
    await mount();
    expect(host.querySelector(".issue-row-open").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/issues/issue-12");
  });

  it("says nothing is here yet when the project has no issues", async () => {
    call = vi.fn(async () => ({ issues: [] }));
    await mount();
    expect(host.querySelector(".issue-empty h2").textContent).toBe("No issues yet");
  });
});

describe("the filters", () => {
  // A filter is a param, not a client-side pass over everything.
  it("sends each one to issues.list", async () => {
    await mount();
    call.mockClear();
    await chooseFilter("state", "open");
    expect(listed("issues.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
  });

  it("offers every column, including the ones nothing stands in", async () => {
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
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
    call = vi.fn(async () => ({ issues: [] }));
    await mount();
    await chooseFilter("state", "closed");
    expect(host.querySelector(".issue-empty").textContent).toContain("No issue matches these filters");
  });
});

describe("the board", () => {
  const board = async () => {
    await mount({ view: "board" });
  };

  it("draws one column per column the project has, in their order", async () => {
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
    await board();
    expect(columnNames()).toEqual(["Backlog", "Ready", "In progress", "In review", "Done"]);
  });

  // A column is what `status` says, not a filter somebody typed.
  it("draws a column nothing stands in", async () => {
    await board();
    expect(cardsIn("ready")).toEqual([]);
    expect(cardsIn("in_progress")).toEqual(["issue-12"]);
  });

  // The columns ARE the statuses, so narrowing by one would empty the rest.
  // The state filter still rides — the board opens on open issues like the
  // list does (#33) — but the column does not: the board's columns ARE the
  // statuses, so narrowing by one would empty every other column.
  it("does not send the column filter while the board is open", async () => {
    await board();
    call.mockClear();
    await chooseFilter("status", "done");
    expect(listed("issues.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
  });

  // Hover for a pointer, an info glyph for everything without one — the same
  // words both ways, so a phone is not told less than a laptop.
  it("says what each column means, on hover and behind a glyph", async () => {
    await board();
    const head = host.querySelector('[data-column="in_review"] .issue-column-head');
    expect(head.getAttribute("title")).toContain("ready to be looked at, not that it is accepted");
    const why = head.querySelector(".issue-column-why");
    expect(why.open).toBe(false);
    why.querySelector("summary").click();
    expect(why.open).toBe(true);
    expect(why.querySelector(".issue-column-note").textContent)
      .toContain("ready to be looked at, not that it is accepted");
  });

  it("puts the glyph on every column, empty ones included", async () => {
    await board();
    const columns = [...host.querySelectorAll(".issue-column")];
    expect(columns.length).toBeGreaterThan(1);
    expect(columns.every((one) => one.querySelector(".issue-column-why"))).toBe(true);
  });

  // The other half of the complaint: five columns and an open/closed mark with
  // no legend. The list row dropped its mark with the dots (#28), so the board
  // — where the mark still is — is where this is now asked.
  it("says on the open/closed mark that it moves independently of the column", async () => {
    await mount();
    host.querySelector('[data-issue-view="board"]').click();
    await flush();
    expect(host.querySelector(".issue-card .issue-state").getAttribute("title"))
      .toContain("The two move independently");
  });

  it("switches between the two views", async () => {
    await mount();
    expect(host.querySelector(".issue-board")).toBeNull();
    host.querySelector('[data-issue-view="board"]').click();
    await flush();
    expect(host.querySelector(".issue-board")).not.toBeNull();
    expect(host.querySelector(".issue-rows")).toBeNull();
  });
});

describe("moving a card", () => {
  const drop = async (issueId, columnId) => {
    const data = new Map([["text/plain", issueId]]);
    host.querySelector(`[data-column-drop="${columnId}"]`).dispatchEvent(
      Object.assign(new Event("drop", { bubbles: true }), {
        preventDefault() {},
        dataTransfer: { getData: (key) => data.get(key) },
      }),
    );
    await flush();
  };

  it("calls issues.update with the new status, and only that", async () => {
    await mount({ view: "board" });
    call.mockClear();
    await drop("issue-12", "in_review");
    expect(listed("issues.update")[0][1]).toEqual({ issue_id: "issue-12", status: "in_review" });
  });

  // The card moves now and the column repaints from the push.
  it("moves the card before the bridge has answered", async () => {
    let settle;
    const listAnswer = { issues: [issue({ number: 12, id: "issue-12", status: "backlog" })] };
    call = vi.fn((method) => {
      if (method === "issues.list") return Promise.resolve(listAnswer);
      return new Promise((resolve) => { settle = resolve; });
    });
    await mount({ view: "board" });
    await drop("issue-12", "done");
    expect(cardsIn("done")).toEqual(["issue-12"]);
    settle({});
    await flush();
  });

  // A card that stayed put with no word is a card the reader will drag again.
  it("puts a refused move back and says why", async () => {
    const listAnswer = { issues: [issue({ number: 12, id: "issue-12", status: "backlog" })] };
    call = vi.fn(async (method) => {
      if (method === "issues.list") return listAnswer;
      throw new Error("issue is closed");
    });
    await mount({ view: "board" });
    await drop("issue-12", "done");
    expect(cardsIn("backlog")).toEqual(["issue-12"]);
    expect(notifyError).toHaveBeenCalledWith("Could not move this issue", "issue is closed");
  });

  // Dragging is one way to move a card; the arrow keys are the other.
  it("moves a focused card with the arrow keys", async () => {
    await mount({ view: "board" });
    call.mockClear();
    const card = host.querySelector('.issue-card[data-issue="issue-12"]');
    card.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await flush();
    expect(listed("issues.update")[0][1]).toEqual({ issue_id: "issue-12", status: "in_review" });
  });

  it("stops at the ends rather than wrapping", async () => {
    const listAnswer = { issues: [issue({ number: 12, id: "issue-12", status: "backlog" })] };
    call = vi.fn(async (method) => (method === "issues.list" ? listAnswer : {}));
    await mount({ view: "board" });
    call.mockClear();
    host.querySelector(".issue-card").dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true }));
    await flush();
    expect(listed("issues.update")).toEqual([]);
  });
});

describe("the two presses", () => {
  it("opens the picker on a row, standing on that issue's current assignee", async () => {
    await mount();
    host.querySelector("[data-issue-assign]").click();
    await flush();
    const [given] = openAssigneePicker.mock.calls[0];
    expect([given.issue.id, given.current]).toEqual(["issue-12", "user"]);
  });

  it("offers the project's agents to the picker, grouped by workspace", async () => {
    await mount();
    host.querySelector("[data-issue-assign]").click();
    await flush();
    const [given] = openAssigneePicker.mock.calls[0];
    expect(given.options.map((one) => one.id)).toEqual([
      "none", "user", "project_agent", "agent:agent-1", "new_agent:ws-1", "new_workspace",
    ]);
  });

  it("opens the picker from a card too", async () => {
    await mount({ view: "board" });
    host.querySelector(".issue-card [data-issue-assign]").click();
    await flush();
    expect(openAssigneePicker).toHaveBeenCalled();
  });

  // #57: filing happens IN the tab. No dialog, no navigation — the list the
  // issue is being filed against stays on screen while it is written.
  it("opens the composer in place, above the list and over nothing", async () => {
    await mount();
    expect(composer()).toBeNull();
    await openComposer();
    expect(composer()).not.toBeNull();
    expect(document.querySelector(".modal, #issue-new-scrim")).toBeNull();
    expect(titles()).toEqual(["Kanban drag does not persist", "Board is unreadable on a phone"]);
    expect(document.activeElement).toBe(host.querySelector(".issue-compose-title"));
  });

  it("files what was typed, as issues.create params", async () => {
    await mount();
    await openComposer();
    typeIn(".issue-compose-title", "Kanban drag");
    typeIn("#issue-new-body", "It does not persist.");
    call.mockClear();
    await fileIt();
    expect(listed("issues.create")[0][1]).toMatchObject({
      project_id: "proj-1",
      title: "Kanban drag",
      body: "It does not persist.",
    });
  });

  it("shuts once it has filed, and leaves the list behind it", async () => {
    await mount();
    await openComposer();
    typeIn(".issue-compose-title", "Kanban drag");
    await fileIt();
    expect(composer()).toBeNull();
  });

  it("refuses an untitled issue without asking the bridge", async () => {
    await mount();
    await openComposer();
    call.mockClear();
    await fileIt();
    expect(listed("issues.create")).toHaveLength(0);
    expect(host.querySelector(".issue-compose-error").hidden).toBe(false);
  });

  // A bridge that predates `issues.create`'s assignee drops it at the facade
  // and answers ok, so the issue is filed and nobody holds it. Assignment is
  // dispatch, so that is work the reader believes has started and has not.
  it("says when a filed issue's assignee went nowhere", async () => {
    await mount();
    call.mockImplementation(async (method) =>
      method === "issues.create" ? { issue: issue({ id: "issue-1", number: 12, assignee: null }) } : { issues: [] });
    await openComposer();
    typeIn(".issue-compose-title", "Kanban drag");
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
    const title = typeIn(".issue-compose-title", "Half written");
    title.focus();
    call.mockImplementation(async (method) =>
      method === "issues.list" ? { issues: [issue({ id: "issue-9", number: 9, title: "Fresh" })], columns: columns() } : {});
    watchers[0].refresh();
    await flush();
    expect(titles()).toEqual(["Fresh"]);
    expect(host.querySelector(".issue-compose-title")).toBe(title);
    expect(title.value).toBe("Half written");
    expect(document.activeElement).toBe(title);
  });

  // The list is keyed, so a filed issue is one row INSERTED rather than a
  // repaint — which is the whole point of filing in place: you watch the thing
  // you just wrote appear in the list you wrote it against.
  //
  // The re-read behind it is held open here, so what is asserted is the
  // OPTIMISTIC row and not the one a refresh would have painted anyway.
  const fileWithTheReadHeldOpen = async () => {
    await openComposer();
    typeIn(".issue-compose-title", "Just filed");
    call.mockImplementation((method) =>
      method === "issues.create"
        ? Promise.resolve({ issue: issue({ id: "issue-20", number: 20, title: "Just filed" }) })
        : new Promise(() => {}), // the list never answers
    );
    await fileIt();
  };

  it("puts the new row in the list without rebuilding the rows around it", async () => {
    await mount();
    const kept = host.querySelector('[data-issue="issue-12"]');
    await fileWithTheReadHeldOpen();
    expect(host.querySelector('[data-issue-group="rest"] .issue-title').textContent).toBe("Just filed");
    expect(host.querySelector('[data-issue="issue-12"]')).toBe(kept);
  });

  it("puts the focus on the row it just made", async () => {
    await mount();
    await fileWithTheReadHeldOpen();
    expect(document.activeElement.closest(".issue-row")?.dataset.issue).toBe("issue-20");
  });

  // Cancelling gives the keyboard back to the press that opened the form —
  // closing a focused subtree otherwise leaves the focus nowhere.
  it("gives the focus back to New issue when nothing was filed", async () => {
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
    typeIn(".issue-compose-title", "Half written");
    await openComposer();
    expect(host.querySelectorAll(".issue-compose")).toHaveLength(1);
    expect(host.querySelector(".issue-compose-title").value).toBe("Half written");
  });

  // #57: the bytes go up BEFORE the issue does, the way a chat attachment
  // does — and against the PROJECT, because an issue being created has no
  // conversation to attach to (`thread.attach` would answer "unknown
  // conversation owner").
  const drop = (file) => {
    const event = new Event("drop", { bubbles: true, cancelable: true });
    Object.defineProperty(event, "dataTransfer", { value: { files: [file] } });
    host.querySelector(".issue-compose").dispatchEvent(event);
  };

  it("sends a dropped file to issues.attach for this project, then files it with the issue", async () => {
    await mount();
    await openComposer();
    call.mockImplementation(async (method) => {
      if (method === "issues.attach") return { name: "shot.png", path: ".build/attachments/abc-shot.png", mime: "image/png", size: 3 };
      if (method === "issues.create") return { issue: issue({ id: "issue-20", number: 20, attachments: [{ path: ".build/attachments/abc-shot.png" }] }) };
      return { issues: [] };
    });
    drop(new File(["png"], "shot.png", { type: "image/png" }));
    await flush();
    expect(listed("issues.attach")[0][1]).toMatchObject({ project_id: "proj-1", filename: "shot.png" });
    expect(host.querySelector(".composer-tray").hidden).toBe(false);

    typeIn(".issue-compose-title", "Kanban drag");
    await fileIt();
    expect(listed("issues.create")[0][1].attachments).toEqual([
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
    expect(host.querySelector(".issue-compose-title")).not.toBeNull();
    expect(host.querySelector("[data-compose-file]")).not.toBeNull();
  });

  // The v1 facade drops a field the bridge predates rather than refusing it,
  // so filing with files on today's bridge answers ok with none. The
  // screenshot was usually the reason for filing, so that is said out loud.
  it("says when the files a bridge cannot carry went nowhere", async () => {
    await mount();
    await openComposer();
    call.mockImplementation(async (method) => {
      if (method === "issues.attach") return { name: "shot.png", path: ".build/attachments/abc-shot.png", mime: "image/png", size: 3 };
      if (method === "issues.create") return { issue: issue({ id: "issue-20", number: 20 }) }; // no attachments came back
      return { issues: [] };
    });
    drop(new File(["png"], "shot.png", { type: "image/png" }));
    await flush();
    typeIn(".issue-compose-title", "Kanban drag");
    await fileIt();
    expect(notifyError).toHaveBeenCalledWith(
      "Filed #20 — but the file did not go with it",
      expect.stringContaining("cannot carry files on an issue"),
    );
  });

  it("says nothing of the sort when the assignee landed", async () => {
    await mount();
    call.mockImplementation(async (method) =>
      method === "issues.create"
        ? { issue: issue({ id: "issue-1", number: 12, assignee: { kind: "project_agent" } }) }
        : { issues: [] });
    await openComposer();
    typeIn(".issue-compose-title", "Kanban drag");
    await chooseAssignee("project_agent");
    await fileIt();
    expect(notifyError).not.toHaveBeenCalled();
  });
});

describe("the filter bar, mounted once", () => {
  // #43. The bar is not redrawn — ever. Not on a push, not on a feed move, not
  // on a re-read that changes the list, not on a filter change, and not when
  // switching to the board and back. Zech: "The inputs/selects really
  // shouldn't be redrawing ever."
  // Every control on the bar: the four menus' presses and the Clear beside
  // them. The search boxes and rows inside a menu are mounted with it.
  const controls = () => [
    ...host.querySelectorAll(".issue-filters .fmenu-press"),
    host.querySelector("[data-issue-filter-clear]"),
  ];

  it("is the same DOM nodes through ten paints and a push", async () => {
    await mount();
    const before = controls();
    expect(before).toHaveLength(5); // four filters and the Clear press
    for (let i = 0; i < 5; i++) pane.feedMoved();
    call.mockImplementation(async (method) =>
      method === "issues.list" ? { issues: [issue({ id: "issue-9", number: 9, title: "Fresh" })], columns: columns() } : {});
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
      method === "issues.list" ? { issues: [issue({ id: "issue-9", number: 9, title: "Fresh", labels: ["bug"] })], columns: columns() } : {});
    watchers[0].refresh();
    await flush();
    expect(titles()).toEqual(["Fresh"]);
    expect(menuPress("label")).toBe(press);
    expect(document.activeElement).toBe(press);
    expect(press.textContent.trim()).toBe("bug");
  });

  // The whole reason the bar is mounted once: a menu the reader has OPEN, with
  // a query half typed into it and a row walked to, is state that lives in the
  // DOM — and a push about an issue must not touch any of it (#43, #44).
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
      method === "issues.list" ? { issues: [issue({ id: "issue-9", number: 9, title: "Fresh", labels: ["bug"] })], columns: columns() } : {});
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
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", {
      issues: [issue({ number: 13, id: "issue-13", labels: ["bug", "spa"] })],
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
    const press = host.querySelector("[data-issue-filter-clear]");
    expect(press.hidden).toBe(true);
    await chooseFilter("label", "bug");
    expect(press.hidden).toBe(false);
    expect(host.querySelector("[data-issue-filter-clear]")).toBe(press);
  });

  it("uses one control for all four filters, and no native select", async () => {
    await mount();
    expect(host.querySelectorAll(".issue-filters select")).toHaveLength(0);
    expect([...host.querySelectorAll("[data-filter-menu]")].map((one) => one.dataset.filterMenu))
      .toEqual(["state", "status", "assignee", "label"]);
  });

  it("survives a switch to the board and back", async () => {
    await mount();
    const before = controls();
    host.querySelector('[data-issue-view="board"]').click();
    await flush();
    host.querySelector('[data-issue-view="list"]').click();
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

  // The rows are keyed, so a re-read that leaves an issue where it was leaves
  // its row the element it was — and only the row that changed is written.
  it("keeps the rows a re-read did not change, and patches the one it did", async () => {
    await mount();
    const kept = host.querySelector('[data-issue="issue-11"]');
    const changed = host.querySelector('[data-issue="issue-12"]');
    call.mockImplementation(async (method) =>
      method === "issues.list"
        ? {
            issues: [
              issue({ number: 12, id: "issue-12", title: "Renamed", status: "in_progress", labels: ["bug"], assignee: { kind: "user" } }),
              issue({ number: 11, id: "issue-11", title: "Board is unreadable on a phone", state: "closed", status: "done" }),
            ],
          }
        : {});
    watchers[0].refresh();
    await flush();
    expect(host.querySelector('[data-issue="issue-11"]')).toBe(kept);
    expect(host.querySelector('[data-issue="issue-12"]')).toBe(changed);
    expect(changed.querySelector(".issue-title").textContent).toBe("Renamed");
  });

  it("subscribes this project for issues, and nothing else", async () => {
    await mount();
    expect(watchers.map((one) => [one.entity, one.deviceId, one.kinds])).toEqual([["proj-1", "dev-1", ["issues"]]]);
  });

  it("re-reads the list when the subscription says something moved", async () => {
    await mount();
    call.mockClear();
    watchers[0].refresh();
    await flush();
    expect(listed("issues.list")).toHaveLength(1);
  });

  // Every kind in one subscribe shares that call's fate, and a refused one
  // takes this device's other subscriptions with it — so a bridge that does
  // not carry issues is asked for nothing rather than for a word it will
  // refuse. The tab keeps its own reads and the ordered pass keeps its cache.
  it("asks a bridge that does not carry issues for no kind at all", async () => {
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
  const CACHED = [issue({ number: 9, id: "issue-9", title: "From the cache", status: "ready" })];

  const refusing = (message) =>
    vi.fn(async (method) => {
      if (method === "issues.list") throw new Error(message);
      return {};
    });

  const note = () => host.querySelector(".read-wait")?.textContent ?? null;

  const withCache = async (message) => {
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: CACHED, columns: columns() });
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
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: CACHED, columns: columns() });
    call = refusing(WENT);
    await mount({ view: "board" });
    expect(cardsIn("ready")).toEqual(["issue-9"]);
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("reads again when the machine is back, with nothing polled in between", async () => {
    await withCache(WENT);
    expect(listed("issues.list")).toHaveLength(1);
    call.mockImplementation(async (method) =>
      method === "issues.list" ? { issues: [issue({ number: 12, id: "issue-12", title: "Kanban drag does not persist" })] } : {},
    );
    await reconnect();
    expect(listed("issues.list")).toHaveLength(2);
    expect(titles()).toEqual(["Kanban drag does not persist"]);
    expect(note()).toBeNull();
    expect(notifyError).not.toHaveBeenCalled();
  });

  it("still says a refusal out loud", async () => {
    await withCache("project_id is required");
    expect(notifyError).toHaveBeenCalledWith("Could not read this project's issues", "project_id is required");
  });

  it("waits with nothing on screen, then says so when the retry fails too", async () => {
    call = refusing(WENT);
    await mount();
    expect(notifyError).not.toHaveBeenCalled();
    await reconnect();
    expect(notifyError).toHaveBeenCalledWith("Could not read this project's issues", WENT);
  });
});

// #33. Zech, deciding the question #28 raised: "If closed means done we
// shouldn't show them in the default view."
describe("what the tab opens on", () => {
  const OPEN_AND_CLOSED = [
    issue({ number: 12, id: "issue-12", title: "Still open", state: "open", status: "in_progress" }),
    issue({ number: 11, id: "issue-11", title: "Finished with", state: "closed", status: "done" }),
  ];

  /** The bridge, narrowing the way the real one does. */
  const listing = () =>
    vi.fn(async (method, params) => {
      if (method !== "issues.list") return {};
      const wanted = params.state;
      return { issues: wanted ? OPEN_AND_CLOSED.filter((one) => one.state === wanted) : OPEN_AND_CLOSED };
    });

  const chooseState = (value) => chooseFilter("state", value);

  it("asks the bridge for open issues, and draws only those", async () => {
    call = listing();
    await mount();
    expect(listed("issues.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
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

  // The row lost its state dot with the dots (#28), so once a closed issue is
  // on screen this is what keeps it from reading as an open one.
  it("marks a closed row Closed once one is on screen", async () => {
    call = listing();
    await mount();
    await chooseState("closed");
    const row = host.querySelector(".issue-row");
    expect(row.querySelector(".issue-closed").textContent).toBe("Closed");
    expect(row.querySelector(".issue-row-facts").firstElementChild.className).toBe("issue-closed");
  });

  it("draws no such chip on the open rows it opens with", async () => {
    call = listing();
    await mount();
    expect(host.querySelector(".issue-closed")).toBeNull();
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

  it("goes back to open issues on Clear, not to everything", async () => {
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
    call = vi.fn(async () => ({ issues: [] }));
    await mount();
    expect(host.querySelector(".issue-empty h2").textContent).toBe("No issues yet");
  });
});
