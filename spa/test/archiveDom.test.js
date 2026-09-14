// @vitest-environment jsdom
// The account archive's wiring: one page painted from archived.list, a record
// that opens under the row it belongs to, and the account's two pages named
// above it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

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

const answering = (deviceId) => ({
  deviceId,
  close: () => {},
  call: vi.fn(async (method) => (method === "archived.list" ? { items: filed[deviceId] } : {})),
});

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
  filed = { "dev-1": [issueItem], "dev-2": [branchItem] };
  adoptDeviceSession(answering("dev-1"));
  adoptDeviceSession(answering("dev-2"));
});

afterEach(() => {
  if (App.poll) clearInterval(App.poll);
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
    expect(rows().map((row) => row.dataset.key)).toEqual(["dev-1/issue-1", "dev-2/run-1"]);
    expect(rows()[1].textContent).toContain("relaydb");
    expect(rows()[1].textContent).toContain("Archived");
  });

  it("opens a finished workspace on the machine it was filed on", async () => {
    filed["dev-1"] = [workspaceItem];
    await renderAccount();
    await flush();
    rows()[0].click();
    expect(location.hash).toBe("#/device/dev-1/project/p1/workspace/workspace-1/changes");
  });

  it("keeps the machines' records apart when both name a record the same", async () => {
    filed["dev-2"] = [{ ...issueItem, project: "relaydb", finished_at: "2026-08-11T09:30:00Z" }];
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

  // The archive is history: the 15s poll almost always reads exactly what is
  // already on the page. Rebuilding it anyway drops a selection someone is
  // copying a path out of, and the focus they reached a card with.
  it("leaves the page alone on a tick that reads the same archive", async () => {
    vi.useFakeTimers();
    await renderAccount();
    await vi.advanceTimersByTimeAsync(0);
    rows()[2].click(); // a record open under it
    const row = rows()[2];
    const record = document.querySelector(".archive-record");

    await vi.advanceTimersByTimeAsync(15000 + 10);

    expect(rows()[2], "the rows were rebuilt by a tick that changed nothing").toBe(row);
    expect(document.querySelector(".archive-record")).toBe(record);
    vi.useRealTimers();
  });

  it("redraws as soon as the archive itself moves", async () => {
    vi.useFakeTimers();
    await renderAccount();
    await vi.advanceTimersByTimeAsync(0);
    expect(rows()).toHaveLength(2);
    filed["dev-2"] = [];

    await vi.advanceTimersByTimeAsync(15000 + 10);

    expect(rows()).toHaveLength(2);
    vi.useRealTimers();
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

    expect(rows().map((row) => row.dataset.key)).toEqual(["dev-1/issue-1"]);
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
