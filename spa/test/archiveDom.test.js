// @vitest-environment jsdom
// The account archive's wiring: one page painted from archived.list, a record
// that opens under the row it belongs to, and the account's two pages named
// above it.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

const items = [
  {
    kind: "issue",
    project_id: "p2",
    project: "dotfiles",
    title: "Split the prompt templates",
    branch: null,
    state: "approved",
    finished_at: "2026-08-12T18:00:00Z",
    issue_id: "issue-1",
    stages: 3,
  },
  {
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
  },
];

let App;
let renderAccount;

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/account/archive";
  ({ App } = await import("../src/app.js"));
  ({ renderAccount } = await import("../src/views/account.js"));
  App.route = { name: "account", page: "archive" };
  App.call = vi.fn(async (method) => (method === "archived.list" ? { items } : {}));
});

afterEach(() => {
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
});

const rows = () => [...document.querySelectorAll("#archive-list .archive-row")];

describe("the account archive page", () => {
  it("lists what every project filed away, newest first", async () => {
    await renderAccount();
    await flush();
    expect(App.call).toHaveBeenCalledWith("archived.list");
    expect(rows().map((row) => row.dataset.key)).toEqual(["issue-1", "run-1"]);
    expect(rows()[1].textContent).toContain("relaydb");
    expect(rows()[1].textContent).toContain("Archived");
  });

  it("opens one record at a time, under its own row", async () => {
    await renderAccount();
    await flush();
    rows()[1].click();
    let record = document.querySelector(".archive-record");
    expect(record.textContent).toContain("/wt/login");
    expect(record.textContent).toContain("abc1234");
    expect(record.querySelector("button")).toBeNull();

    rows()[0].click();
    expect(document.querySelectorAll(".archive-record")).toHaveLength(1);
    record = document.querySelector(".archive-record");
    expect(record.textContent).toContain("3 stages");

    rows()[0].click();
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

  it("says so when the device cannot answer, and keeps what it has", async () => {
    App.call = vi.fn(async () => {
      throw new Error("offline");
    });
    await renderAccount();
    await flush();
    expect(document.querySelector("#archive-list").textContent).toContain("unavailable");
    // The page — and the way back off it — is on screen either way.
    expect(document.querySelectorAll("#root .account-nav .t")).toHaveLength(2);
  });
});
