// @vitest-environment jsdom
// The account archive's wiring: one page painted from archived.list, a record
// that opens under the row it belongs to, and the account's two pages named
// above it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// The page reads again when a machine's board record moves, so the suite needs
// a store for that record to move in.
globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

// The page reads at most once a frame and never while its last read is still
// out (core/feedRows.js), so what follows a write here is frames rather than
// turns.
const frame = () => new Promise((done) => requestAnimationFrame(() => setTimeout(done, 0)));
const settle = async () => {
  for (let turn = 0; turn < 3; turn += 1) await frame();
};

// The account's archive spans the account: every machine is asked what it
// filed away, and the rows come back in one list, newest first, each carrying
// the machine that answered for it.
const workspaceItem = {
  kind: "workspace",
  workspace_id: "workspace-1",
  project_id: "p1",
  project: "relaydb",
  title: "Clean checkout",
  state: "finished",
  finished_at: "2026-08-13T18:00:00Z",
  worktree_path: "/work/clean",
};

const issueItem = {
  kind: "issue",
  project_id: "p2",
  project: "dotfiles",
  title: "Split the prompt templates",
  branch: null,
  state: "approved",
  finished_at: "2026-08-12T18:00:00Z",
  issue_id: "issue-1",
  stages: 3,
};

const branchItem = {
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  title: "Fix the login flow",
  branch: "build/login",
  state: "archived",
  action: "delete",
  finished_at: "2026-08-10T09:30:00Z",
  run_id: "run-1",
  worktree_id: "wt-1",
  worktree_path: "/wt/login",
  head_sha: "abc1234",
};

let App;
let renderAccount;
let adoptDeviceSession;
let resetDeviceContexts;
// What each machine says it has filed away, as the test writes it.
let filed;

// The sessions this suite adopted, by machine, so a test can count what one
// of them was asked.
let sessions;

// While this holds a promise, every machine's archive read waits on it — the
// suite's way of standing a read up on the wire and leaving it there.
let answerGate;

const answering = (deviceId) => {
  const session = {
    deviceId,
    close: () => {},
    call: vi.fn(async (method) => {
      if (method !== "archived.list") return {};
      if (answerGate) await answerGate;
      return { items: filed[deviceId] };
    }),
  };
  sessions[deviceId] = session;
  return session;
};

/** How many times the machines were asked for their archive, across them. */
const archiveReads = () =>
  Object.values(sessions)
    .map((session) => session.call.mock.calls.filter(([method]) => method === "archived.list").length)
    .reduce((total, count) => total + count, 0);

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/account/archive";
  ({ App } = await import("../src/app.js"));
  ({ renderAccount } = await import("../src/views/account.js"));
  ({ adoptDeviceSession, resetDeviceContexts } = await import("../src/core/deviceContexts.js"));
  App.route = { name: "account", page: "archive" };
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  filed = { "dev-1": [workspaceItem, issueItem], "dev-2": [branchItem] };
  sessions = {};
  answerGate = null;
  adoptDeviceSession(answering("dev-1"));
  adoptDeviceSession(answering("dev-2"));
});

afterEach(() => {
  vi.useRealTimers();
  if (App.poll) App.poll.dispose?.();
  App.poll = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
  resetDeviceContexts();
});

const rows = () => [...document.querySelectorAll("#archive-list .archive-row")];

describe("the account archive page", () => {
  it("lists what every device filed away, newest first", async () => {
    await renderAccount();
    await flush();
    // One list across the account, each row named by the machine it is on.
    expect(rows().map((row) => row.dataset.key)).toEqual(["dev-1/workspace-1", "dev-1/issue-1", "dev-2/run-1"]);
    expect(rows()[2].textContent).toContain("relaydb");
    expect(rows()[2].textContent).toContain("Archived");
  });

  // Done removes the workspace, so a finished workspace has no surface left to
  // open: its record states what was finished, like every other archived row.
  it("states a finished workspace's record instead of opening a workspace", async () => {
    filed["dev-1"] = [workspaceItem];
    const hash = location.hash;
    await renderAccount();
    await flush();
    rows()[0].click();
    expect(location.hash).toBe(hash);
    expect(rows()[0].getAttribute("aria-expanded")).toBe("true");
    expect(document.querySelectorAll(".archive-record")).toHaveLength(1);
  });

  it("keeps the machines' records apart when both name a record the same", async () => {
    filed = { "dev-1": [issueItem], "dev-2": [{ ...issueItem, project: "relaydb", finished_at: "2026-08-11T09:30:00Z" }] };
    await renderAccount();
    await flush();

    expect(rows().map((row) => row.dataset.key)).toEqual(["dev-1/issue-1", "dev-2/issue-1"]);
    rows()[1].click();
    expect(document.querySelectorAll(".archive-record")).toHaveLength(1);
    // The record opened is the one under the row that was pressed, not the
    // other machine's record of the same name.
    expect(rows()[1].getAttribute("aria-expanded")).toBe("true");
    expect(document.querySelector(".archive-record").textContent).toContain("relaydb");
  });

  // Two machines' `repo` are two different checkouts under one word. The rail
  // says the machine after a project name two machines share; the archive says
  // it after a title two machines share, by the same rule and in the same dim
  // words — and says nothing where the title is the account's own.
  it("says the machine after a title two machines both filed", async () => {
    filed = {
      "dev-1": [{ ...workspaceItem, title: "repo" }, issueItem],
      "dev-2": [{ ...branchItem, title: "repo" }],
    };
    await renderAccount();
    await flush();

    const named = (row) => [...row.querySelectorAll(".title, .title + .dim")].map((node) => node.textContent);
    expect(named(rows()[0])).toEqual(["repo", "workshop"]);
    expect(named(rows()[2])).toEqual(["repo", "laptop"]);
    expect(named(rows()[1])).toEqual(["Split the prompt templates"]);
  });

  it("says no machine when one machine holds the title", async () => {
    filed = { "dev-1": [workspaceItem], "dev-2": [] };
    await renderAccount();
    await flush();

    expect(rows()[0].querySelector(".dim")).toBeNull();
  });

  it("opens one record at a time, under its own row", async () => {
    await renderAccount();
    await flush();
    rows()[2].click();
    let record = document.querySelector(".archive-record");
    expect(record.textContent).toContain("/wt/login");
    expect(record.textContent).toContain("abc1234");
    expect(record.querySelector("button")).toBeNull();

    rows()[1].click();
    expect(document.querySelectorAll(".archive-record")).toHaveLength(1);
    record = document.querySelector(".archive-record");
    expect(record.textContent).toContain("3 stages");

    rows()[1].click();
    expect(document.querySelector(".archive-record")).toBeNull();
  });

  it("names the account's pages above it and marks the open one", async () => {
    await renderAccount();
    await flush();
    const tabs = [...document.querySelectorAll("#root .account-nav .t")];
    expect(tabs.map((tab) => tab.dataset.page)).toEqual(["settings", "archive"]);
    expect(tabs[1].classList.contains("active")).toBe(true);
    tabs[0].click();
    expect(location.hash).toBe("#/account/settings");
  });

  // The archive is history: a re-read almost always lands exactly what is
  // already on the page. Rebuilding it anyway drops a selection someone is
  // copying a path out of, and the focus they reached a card with.
  it("leaves the page alone on a read that lands the same archive", async () => {
    const { writeCached } = await import("../src/core/localCache.js");
    await renderAccount();
    await flush();
    rows()[2].click(); // a record open under it
    const row = rows()[2];
    const record = document.querySelector(".archive-record");

    await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [] });
    await settle();

    expect(rows()[2], "the rows were rebuilt by a read that changed nothing").toBe(row);
    expect(document.querySelector(".archive-record")).toBe(record);
  });

  it("redraws when a machine's board record moves", async () => {
    const { writeCached } = await import("../src/core/localCache.js");
    await renderAccount();
    await flush();
    expect(rows()).toHaveLength(3);
    filed["dev-2"] = [];

    // A pass wrote that machine's board. Nothing here polls; the announcement
    // behind that write is what says the archive may have moved.
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [] });
    await settle();

    expect(rows()).toHaveLength(2);
  });

  // A `state` push is what moves one row, and it writes that row's OWN record —
  // the whole board's record is written by a pass and by a removal, and by
  // nothing else. Archiving is exactly a row moving, so hearing only the board
  // would leave this page on the last pass's history until the next one.
  it("redraws when one row's own record moves", async () => {
    const { writeCached } = await import("../src/core/localCache.js");
    await renderAccount();
    await flush();
    expect(rows()).toHaveLength(3);
    filed["dev-2"] = [];

    await writeCached(
      { deviceId: "dev-1", entityId: "workspace-1", kind: "row" },
      { ...workspaceItem, state: "finished" },
    );
    await settle();

    expect(rows()).toHaveLength(2);
  });

  // A pass writes a machine's board and then every row on it, each write its
  // own announcement. A page that read on each of them would ask every machine
  // on the account for its whole archive a dozen times over for one pass.
  it("reads each machine once for a pass that writes a board and every row on it", async () => {
    const { writeCached } = await import("../src/core/localCache.js");
    await renderAccount();
    await flush();
    const before = archiveReads();

    await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [] });
    for (const entityId of ["run-1", "run-2", "run-3", "run-4", "run-5"]) {
      await writeCached({ deviceId: "dev-1", entityId, kind: "row" }, { entityId });
    }
    await settle();

    expect(archiveReads() - before).toBe(App.devices.length);
  });

  // Rows keep landing while the account's machines are still answering the
  // last read. Reading again per row would put a read per machine per row on
  // the wire, all of them answering the same history.
  it("does not read again while its last read is still out on the wire", async () => {
    const { writeCached } = await import("../src/core/localCache.js");
    await renderAccount();
    await settle();
    const before = archiveReads();
    const perRead = App.devices.length;

    let answer;
    answerGate = new Promise((done) => {
      answer = done;
    });
    await writeCached({ deviceId: "dev-1", entityId: "", kind: "feed" }, { items: [] });
    await settle();
    expect(archiveReads() - before, "the board write put one read on the wire").toBe(perRead);

    for (const entityId of ["run-1", "run-2", "run-3"]) {
      await writeCached({ deviceId: "dev-1", entityId, kind: "row" }, { entityId });
    }
    await settle();
    expect(archiveReads() - before, "nothing joined the read that was already out").toBe(perRead);

    answerGate = null;
    answer();
    await settle();
    expect(archiveReads() - before, "what landed under it is one further read").toBe(perRead * 2);
  });

  it("keeps the machines that did answer when one of them will not", async () => {
    resetDeviceContexts();
    adoptDeviceSession(answering("dev-1"));
    adoptDeviceSession({
      deviceId: "dev-2",
      close: () => {},
      call: vi.fn(async () => {
        throw new Error("offline");
      }),
    });

    await renderAccount();
    await flush();

    expect(rows().map((row) => row.dataset.key)).toEqual(["dev-1/workspace-1", "dev-1/issue-1"]);
  });

  it("says so when no device can answer, and keeps what it has", async () => {
    resetDeviceContexts();
    await renderAccount();
    await flush();
    expect(document.querySelector("#archive-list").textContent).toContain("unavailable");
    // The page — and the way back off it — is on screen either way.
    expect(document.querySelectorAll("#root .account-nav .t")).toHaveLength(2);
  });
});
