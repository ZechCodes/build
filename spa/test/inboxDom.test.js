// @vitest-environment jsdom
// The inbox rail's wiring: one list painted from the feed's items[], opening an
// entry, Done (with the linked-issue disclosure behind its refusal), and mute.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

let feedItems = [];
let subscriber = null;
const refreshFeed = vi.fn(async () => subscriber && subscriber({ items: feedItems }));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscriber = fn;
    fn({ items: feedItems });
    return () => {};
  },
  startFeed: () => {},
  stopFeed: () => {},
  refreshFeed: (...args) => refreshFeed(...args),
  primaryRunIdFor: () => null,
}));

// The rail is a mounted singleton (one inbox, for the app's whole life), so
// each test takes a fresh module graph rather than a reset switch the app would
// never call.
let App;
let mountInboxList;

const flush = () => new Promise((done) => setTimeout(done, 0));
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
const rowFor = (entityId) => document.querySelector(`.inbox-entry[data-entity="${entityId}"]`);

/** Answer the confirmation modal the decisive verbs open. */
async function answerConfirm(ok) {
  await flush();
  const scrim = document.getElementById("confirm-scrim");
  expect(scrim, "a confirmation was expected").toBeTruthy();
  scrim.querySelector(ok ? "[data-confirm-ok]" : "[data-confirm-cancel]").click();
  await flush();
}

const branchRow = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  title: "Fix the login flow",
  state: "review",
  unread: true,
  unread_count: 1,
  unread_reason: "done",
  working: false,
  working_time: null,
  agents: [],
  stat: { files_changed: 2, insertions: 8, deletions: 1, uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" },
  resume_at: new Date().toISOString(),
  can_finish: true,
  muted: false,
  worktree_path: "/wt/login",
  worktree_id: "wt-1",
  run_id: "run-1",
  issue_id: null,
  primary: false,
  ...over,
});

const issueRow = (over = {}) => ({
  kind: "issue",
  project_id: "p2",
  project: "dotfiles",
  branch: null,
  title: "Rework the prompt cache",
  state: "plan_review",
  unread: false,
  unread_count: 0,
  unread_reason: null,
  working: true,
  working_time: { since: new Date().toISOString(), seconds: 30 },
  agents: [],
  stat: null,
  resume_at: new Date().toISOString(),
  can_finish: false,
  muted: false,
  worktree_path: null,
  worktree_id: null,
  run_id: null,
  issue_id: "iss-1",
  primary: false,
  ...over,
});

/** Repaint the rail from a fresh set of rows. */
const feed = (items) => {
  feedItems = items;
  subscriber({ items });
};

beforeEach(async () => {
  vi.resetModules();
  ({ App } = await import("../src/app.js"));
  ({ mountInboxList } = await import("../src/core/inboxView.js"));
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  location.hash = "";
  App.route = { name: "inbox" };
  App.gated = false;
  App.call = vi.fn(async () => ({ ok: true }));
  refreshFeed.mockClear();
  feedItems = [branchRow(), issueRow()];
  mountInboxList();
});

afterEach(() => {
  document.getElementById("confirm-scrim")?.remove();
});

describe("the inbox rail", () => {
  it("paints one list across projects, with each row's state, context and reason", () => {
    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-1", "iss-1"]);
    const branch = rowFor("run-1");
    expect(branch.querySelector(".sdot").className).toContain("sdot-unread");
    expect(branch.textContent).toContain("relaydb");
    expect(branch.textContent).toContain("build/login");
    expect(branch.textContent).toContain("The agent finished — review the work");
    expect(rowFor("iss-1").querySelector(".sdot").className).toContain("sdot-working");
    // No project blocks, no worktree fold — the rail's list is only entries.
    expect(document.querySelectorAll("#inbox-list .sproj, #inbox-list .swt-line").length).toBe(0);
  });

  it("opens an entry to its work item and tells the bridge it was read", async () => {
    rowFor("run-1").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(location.hash).toBe("#/project/p1/branch/build%2Flogin/changes");
  });

  it("opens an issue by its own id", async () => {
    rowFor("iss-1").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "iss-1" });
    expect(location.hash).toBe("#/project/p2/issue/iss-1");
  });

  it("opens a project's primary checkout, which names no entity to read", async () => {
    feed([branchRow({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, unread: false })]);
    document.querySelector('#inbox-list .inbox-entry[data-key="branch:p1:main"]').click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("entity.seen", expect.anything());
    expect(location.hash).toBe("#/project/p1/branch/main/changes");
  });

  it("marks the entry the route stands on", async () => {
    App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
    feed(feedItems);
    expect(rowFor("run-1").className).toContain("active");
    expect(rowFor("iss-1").className).not.toContain("active");
  });

  it("finishes a branch on Done, dismisses its row at once, and reads the entry", async () => {
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(true);
    expect(App.call).toHaveBeenCalledWith("branch.finish", {
      project_id: "p1",
      branch: "build/login",
      action: "cleanup",
    });
    expect(rowFor("run-1")).toBeNull(); // gone before the daemon catches up
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(refreshFeed).toHaveBeenCalled();
  });

  it("keeps the row when the confirmation is declined", async () => {
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(false);
    expect(App.call).not.toHaveBeenCalledWith("branch.finish", expect.anything());
    expect(rowFor("run-1")).toBeTruthy();
  });

  it("discloses the unlink override when the linked issue refuses, then finishes alone", async () => {
    feed([branchRow({ issue_id: "iss-9" })]);
    App.call = vi.fn(async (method, params) => {
      if (method === "branch.finish" && !params.unlink) {
        throw new Error("branch.finish: Done also archives the issue it implements — pass unlink to finish the branch alone");
      }
      return { ok: true };
    });
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(true); // Done
    await answerConfirm(true); // the disclosure
    expect(App.call).toHaveBeenCalledWith("branch.finish", {
      project_id: "p1",
      branch: "build/login",
      action: "cleanup",
      unlink: true,
    });
    expect(rowFor("run-1")).toBeNull();
  });

  it("restores the row and says why when Done fails", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "branch.finish") throw new Error("worktree is dirty");
      return { ok: true };
    });
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(true);
    const row = rowFor("run-1");
    expect(row).toBeTruthy();
    const error = row.querySelector("[data-done-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("worktree is dirty");
  });

  it("archives an issue through the plan verb", async () => {
    feed([issueRow({ can_finish: true, working: false })]);
    rowFor("iss-1").querySelector("[data-done]").click();
    await answerConfirm(true);
    expect(App.call).toHaveBeenCalledWith("plan.archive", { plan_id: "iss-1" });
  });

  it("mutes an entry from its own menu, on the derived entity id", async () => {
    expect(rowFor("run-1").querySelector(".inbox-menu").hidden).toBe(true);
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector(".inbox-menu").hidden).toBe(false);
    rowFor("run-1").querySelector("[data-mute]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.mute", { entity_id: "run-1", muted: true });
  });

  it("offers to unmute a muted entry, and never navigates from the menu", async () => {
    feed([branchRow({ muted: true, unread: false })]);
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").className).toContain("inbox-muted");
    rowFor("run-1").querySelector("[data-mute]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.mute", { entity_id: "run-1", muted: false });
    expect(location.hash).toBe("");
  });
});
