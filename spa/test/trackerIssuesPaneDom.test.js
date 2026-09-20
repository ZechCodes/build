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
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: carriedKinds } }),
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

const openCreateIssue = vi.fn();
vi.mock("../src/core/trackerCreate.js", () => ({
  openCreateIssue: (...args) => openCreateIssue(...args),
  labelsFromText: (text) => String(text || "").split(",").map((one) => one.trim()).filter(Boolean),
}));

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
    navigate: vi.fn(),
    ...over,
  });
  await flush();
  return pane;
};

const listed = (method) => call.mock.calls.filter(([name]) => name === method);
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
  away = true;
  reconnecting = true;
  movedListeners = new Set();
  notifyError.mockClear();
  openAssigneePicker.mockClear();
  openCreateIssue.mockClear();
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
});

describe("the list", () => {
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
    expect([...facts.children].map((one) => one.classList[0]))
      .toEqual(["issue-status", "issue-age", "issue-label", "issue-assign"]);
    expect(facts.querySelector(".issue-status").textContent).toBe("In progress");
    expect(facts.querySelector(".issue-label").textContent).toBe("bug");
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
    const state = host.querySelector('[data-issue-filter="state"]');
    state.value = "open";
    state.dispatchEvent(new Event("change"));
    await flush();
    expect(listed("issues.list")[0][1]).toEqual({ project_id: "proj-1", state: "open" });
  });

  it("offers every column, including the ones nothing stands in", async () => {
    await trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues: [], columns: columns() });
    await mount();
    const options = [...host.querySelectorAll('[data-issue-filter="status"] option')].map((one) => one.value);
    expect(options).toEqual(["", "backlog", "ready", "in_progress", "in_review", "done"]);
  });

  it("offers a clear only once something is narrowed", async () => {
    await mount();
    expect(host.querySelector("[data-issue-filter-clear]")).toBeNull();
    const label = host.querySelector('[data-issue-filter="label"]');
    label.value = "bug";
    label.dispatchEvent(new Event("change"));
    await flush();
    expect(host.querySelector("[data-issue-filter-clear]")).not.toBeNull();
  });

  it("says the filter is why the list is empty, not the project", async () => {
    call = vi.fn(async () => ({ issues: [] }));
    await mount();
    const state = host.querySelector('[data-issue-filter="state"]');
    state.value = "closed";
    state.dispatchEvent(new Event("change"));
    await flush();
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
  it("does not send the column filter while the board is open", async () => {
    await board();
    call.mockClear();
    const status = host.querySelector('[data-issue-filter="status"]');
    status.value = "done";
    status.dispatchEvent(new Event("change"));
    await flush();
    expect(listed("issues.list")[0][1]).toEqual({ project_id: "proj-1" });
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

  it("files a new issue in this project", async () => {
    await mount();
    host.querySelector("[data-issue-new]").click();
    expect(openCreateIssue.mock.calls[0][0].projectId).toBe("proj-1");
  });

  // A bridge that predates `issues.create`'s assignee drops it at the facade
  // and answers ok, so the issue is filed and nobody holds it. Assignment is
  // dispatch, so that is work the reader believes has started and has not.
  it("says when a filed issue's assignee went nowhere", async () => {
    await mount();
    host.querySelector("[data-issue-new]").click();
    const { onFiled } = openCreateIssue.mock.calls[0][0];
    onFiled({ issue: issue({ id: "issue-1", number: 12 }) }, { assigneeWentNowhere: true });
    await flush();
    expect(notifyError).toHaveBeenCalledWith(
      "Filed #12 — but nobody was assigned",
      expect.stringContaining("created unassigned and nothing was started"),
    );
  });

  it("says nothing of the sort when the assignee landed", async () => {
    await mount();
    host.querySelector("[data-issue-new]").click();
    const { onFiled } = openCreateIssue.mock.calls[0][0];
    onFiled({ issue: issue({ id: "issue-1", number: 12 }) }, { assigneeWentNowhere: false });
    await flush();
    expect(notifyError).not.toHaveBeenCalled();
  });
});

describe("the push", () => {
  // The feed moves whenever any agent's state does, and most of those moves
  // change nothing on this tab. A tab that redrew for each one would take the
  // reader's focus out of the filter they were using.
  it("does not redraw or drop focus when the feed moves without changing what it shows", async () => {
    await mount();
    const filter = host.querySelector("select");
    filter.focus();
    pane.feedMoved();
    pane.feedMoved();
    expect(host.querySelector("select")).toBe(filter);
    expect(document.activeElement).toBe(filter);
  });

  it("puts the focus back on the same control when a re-read does change the list", async () => {
    await mount();
    const filter = host.querySelector("select");
    filter.focus();
    const name = filter.name || filter.id;
    call.mockImplementation(async (method) => (method === "issues.list" ? { issues: [issue({ id: "issue-9", number: 9, title: "Fresh" })], columns: columns() } : {}));
    watchers[0].refresh();
    await flush();
    expect(host.textContent).toContain("Fresh");
    const after = host.querySelector("select");
    expect(after.name || after.id).toBe(name);
    expect(document.activeElement).toBe(after);
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
