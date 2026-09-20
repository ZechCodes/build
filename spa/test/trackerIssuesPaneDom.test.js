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
  watchChanges: (registration) => {
    const named = RETIRED.filter((option) => option in registration);
    if (named.length) throw new TypeError(`watchChanges does not poll: remove ${named.join(", ")}`);
    const watcher = { ...registration, disposed: false };
    watchers.push(watcher);
    return { dispose: () => { watcher.disposed = true; } };
  },
}));

const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

const openAssigneePicker = vi.fn(() => ({ close: vi.fn(), setCatalog: vi.fn() }));
vi.mock("../src/core/trackerAssigneePicker.js", () => ({
  openAssigneePicker: (...args) => openAssigneePicker(...args),
}));

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

  // Open/closed is independent of the Done column, so a row shows both.
  it("shows the state and the column as two separate facts", async () => {
    await mount();
    const closed = host.querySelectorAll(".issue-row")[1];
    expect(closed.querySelector(".issue-state").classList.contains("issue-state-closed")).toBe(true);
    expect(closed.querySelector(".issue-status").textContent).toBe("Done");
    const open = host.querySelectorAll(".issue-row")[0];
    expect(open.querySelector(".issue-state").classList.contains("issue-state-open")).toBe(true);
    expect(open.querySelector(".issue-status").textContent).toBe("In progress");
  });

  it("draws the labels and who holds it", async () => {
    await mount();
    expect(host.querySelector(".issue-label").textContent).toBe("bug");
    expect(host.querySelector(".issue-assign").textContent.trim()).toBe("You");
    expect(host.querySelectorAll(".issue-assign")[1].textContent.trim()).toBe("Unassigned");
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
});

describe("the push", () => {
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

  it("takes its subscription down with it", async () => {
    await mount();
    pane.dispose();
    pane = null;
    expect(watchers[0].disposed).toBe(true);
  });
});
