// @vitest-environment jsdom
// The inbox rail's wiring: one list painted from the feed's items[], in the
// anchor's order; opening an entry; Done, which deletes a branch behind the
// bridge's own warnings; mute; and the Recent disclosure at the end.

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

let feedItems = [];
const feedProjects = [
  { id: "p1", name: "relaydb" },
  { id: "p2", name: "dotfiles" },
];
let subscriber = null;
const refreshFeed = vi.fn(async () => subscriber && subscriber({ items: feedItems, projects: feedProjects }));
vi.mock("../src/core/taskFeed.js", () => ({
  subscribeFeed: (fn) => {
    subscriber = fn;
    fn({ items: feedItems, projects: feedProjects });
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

const now = () => new Date().toISOString();
const hoursAgo = (hours) => new Date(Date.now() - hours * 3600 * 1000).toISOString();

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
  stat: {
    files_changed: 9,
    insertions: 90,
    deletions: 40,
    uncommitted: { files_changed: 2, insertions: 8, deletions: 1 },
    ahead: 0,
    upstream: "origin/build/login",
  },
  anchor: hoursAgo(3),
  last_activity: now(),
  resume_at: now(),
  can_finish: true,
  finish: { warnings: [] },
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
  anchor: hoursAgo(1),
  last_activity: now(),
  resume_at: now(),
  can_finish: true,
  finish: { warnings: [] },
  muted: false,
  worktree_path: null,
  worktree_id: null,
  run_id: null,
  issue_id: "iss-1",
  implementing_branch: null,
  implementation_active: false,
  primary: false,
  ...over,
});

/** Repaint the rail from a fresh set of rows. */
const feed = (items) => {
  feedItems = items;
  subscriber({ items, projects: feedProjects });
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
  it("paints one list across projects, oldest anchor first, two lines to a row", () => {
    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-1", "iss-1"]);
    const branch = rowFor("run-1");
    expect(branch.querySelector(".sdot").className).toContain("sdot-unread");
    expect(branch.querySelector(".inbox-name").textContent).toContain("build/login");
    expect(branch.querySelector(".inbox-facts").textContent).toBe("2 files · +8 −1");
    // The project and why it needs you are on the row itself, not on a line.
    expect(branch.title).toContain("relaydb");
    expect(branch.title).toContain("The agent finished — review the work");
    expect(rowFor("iss-1").querySelector(".sdot").className).toContain("sdot-working");
    expect(rowFor("iss-1").querySelector(".inbox-facts").textContent).toBe("Getting started");
    // No project blocks, no worktree fold — the rail's list is only entries.
    expect(document.querySelectorAll("#inbox-list .sproj, #inbox-list .swt-line").length).toBe(0);
  });

  it("puts what was picked up most recently at the bottom, not the top", () => {
    feed([
      branchRow({ branch: "build/fresh", run_id: "run-fresh", anchor: now() }),
      branchRow({ anchor: hoursAgo(9) }),
      issueRow({ anchor: hoursAgo(5) }),
    ]);
    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-1", "iss-1", "run-fresh"]);
  });

  // One piece of work, one row: the branch is where the work is, and the issue
  // comes straight back when that branch is deleted without merging.
  it("hides an issue a branch is implementing, and shows it again when the branch goes", () => {
    feed([issueRow({ implementing_branch: "build/cache", implementation_active: true })]);
    expect(rowFor("iss-1")).toBeNull();
    feed([issueRow({ implementing_branch: "build/cache", implementation_active: false })]);
    expect(rowFor("iss-1")).toBeTruthy();
  });

  it("never paints work that is over", () => {
    feed([branchRow({ state: "merged" }), issueRow({ state: "archived" })]);
    expect(rows()).toEqual([]);
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

  it("deletes a branch on Done, dismisses its row at once, and reads the entry", async () => {
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(true);
    expect(App.call).toHaveBeenCalledWith("branch.finish", {
      project_id: "p1",
      branch: "build/login",
      action: "delete",
    });
    expect(rowFor("run-1")).toBeNull(); // gone before the daemon catches up
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(refreshFeed).toHaveBeenCalled();
  });

  // Done is never refused — the bridge says what the deletion would cost and
  // the confirmation is where the user reads it.
  it("puts the bridge's warning in the confirmation before it deletes anything", async () => {
    feed([
      branchRow({
        finish: {
          warnings: [
            { code: "unmerged", message: "build/login has never been pushed, and has 4 commits that main does not", count: 4, ref: "main" },
          ],
        },
      }),
    ]);
    rowFor("run-1").querySelector("[data-done]").click();
    await flush();
    const scrim = document.getElementById("confirm-scrim");
    expect(scrim.querySelector(".confirm-warnings").textContent).toContain("4 commits that main does not");
    expect(scrim.textContent).toContain("Delete branch build/login");
    scrim.querySelector("[data-confirm-cancel]").click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("branch.finish", expect.anything());
  });

  // An issue whose branch was deleted unmerged comes back asking for somebody:
  // clearing its cursor here would swallow the event that says what happened.
  it("reads only the branch it deleted, never the issue it hands back", async () => {
    feed([branchRow({ issue_id: "iss-9" })]);
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(true);
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(App.call).not.toHaveBeenCalledWith("entity.seen", { entity_id: "iss-9" });
  });

  it("keeps the row when the confirmation is declined", async () => {
    rowFor("run-1").querySelector("[data-done]").click();
    await answerConfirm(false);
    expect(App.call).not.toHaveBeenCalledWith("branch.finish", expect.anything());
    expect(rowFor("run-1")).toBeTruthy();
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

  it("archives an issue through the plan verb, warning when nothing ever implemented it", async () => {
    feed([
      issueRow({
        working: false,
        finish: { warnings: [{ code: "unimplemented", message: "No branch has implemented this issue" }] },
      }),
    ]);
    rowFor("iss-1").querySelector("[data-done]").click();
    await flush();
    expect(document.getElementById("confirm-scrim").querySelector(".confirm-warnings").textContent).toContain(
      "No branch has implemented this issue",
    );
    document.getElementById("confirm-scrim").querySelector("[data-confirm-ok]").click();
    await flush();
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

  it("keeps Recent open across a repaint once the user has opened it", async () => {
    // Five live rows, so Recent does not open itself.
    const live = Array.from({ length: 5 }, (_, index) =>
      branchRow({ branch: `build/live-${index}`, run_id: `run-live-${index}`, anchor: hoursAgo(index + 1) }),
    );
    const quiet = branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) });
    feed([...live, quiet]);
    const toggle = () => document.querySelector("[data-recent-toggle]");
    expect(toggle().getAttribute("aria-expanded")).toBe("false");
    expect(rowFor("run-old")).toBeNull();

    toggle().click();
    await flush();
    expect(rowFor("run-old")).toBeTruthy();

    feed([...live, quiet]); // a feed tick must not shut what the user opened
    expect(toggle().getAttribute("aria-expanded")).toBe("true");
    expect(rowFor("run-old")).toBeTruthy();
  });

  it("opens Recent by itself when there is almost nothing above it", () => {
    feed([branchRow(), branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    expect(document.querySelector("[data-recent-toggle]").getAttribute("aria-expanded")).toBe("true");
    expect(rowFor("run-old")).toBeTruthy();
  });

  it("opens a Recent row like any other", async () => {
    feed([branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    rowFor("run-old").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.seen", { entity_id: "run-old" });
    expect(location.hash).toBe("#/project/p1/branch/build%2Fold/changes");
  });

  // Clearing a row is not muting it: a muted row stays and stops asking, a
  // cleared one is off the inbox until something new needs the user. The bridge
  // owns that truth (`dismissed` on the row); the tap only gets there first.
  it("clears an entry from its own menu, and the row leaves before the daemon answers", async () => {
    App.call = vi.fn(async (method, params) => {
      if (method !== "entity.dismiss") return { ok: true };
      feedItems = [branchRow({ dismissed: true }), issueRow()];
      return { entity_id: params.entity_id, dismissed: true };
    });
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    rowFor("run-1").querySelector("[data-dismiss]").click();
    expect(rowFor("run-1")).toBeNull();

    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.dismiss", { entity_id: "run-1" });
    expect(refreshFeed).toHaveBeenCalled();
    expect(rowFor("run-1")).toBeNull();
    expect(rowFor("iss-1")).toBeTruthy();
    expect(location.hash).toBe("");
  });

  it("brings the row back and says why when clearing fails", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "entity.dismiss") throw new Error("the relay is offline");
      return { ok: true };
    });
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    rowFor("run-1").querySelector("[data-dismiss]").click();
    await flush();
    const row = rowFor("run-1");
    expect(row).toBeTruthy();
    const error = row.querySelector("[data-done-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("the relay is offline");
  });

  // Gone until it speaks again: the row comes back by itself the moment the
  // bridge stops calling it cleared.
  it("paints no row the bridge calls cleared, and paints it again when it speaks", () => {
    feed([branchRow({ dismissed: true }), issueRow()]);
    expect(rowFor("run-1")).toBeNull();
    expect(rowFor("iss-1")).toBeTruthy();
    feed([branchRow({ dismissed: false, unread: true }), issueRow()]);
    expect(rowFor("run-1")).toBeTruthy();
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

// ---- the paint ---------------------------------------------------------------
// The list is reconciled by row key, not rewritten: a tick that says what the
// last one said touches nothing at all, and a tick that changes one row touches
// only that row's own subtree.

describe("the inbox rail's paint", () => {
  /** Everything the DOM under `target` did while `act` ran. */
  const churn = (target, act) => {
    const seen = [];
    const observer = new MutationObserver((records) => seen.push(...records));
    observer.observe(target, { childList: true, subtree: true, attributes: true, characterData: true });
    act();
    seen.push(...observer.takeRecords());
    observer.disconnect();
    return seen;
  };

  const list = () => document.getElementById("inbox-list");

  it("touches nothing when the feed repeats what it already said", () => {
    expect(churn(list(), () => feed(feedItems))).toEqual([]);
  });

  it("redraws only the row that changed, and keeps every row's element", () => {
    const branch = rowFor("run-1");
    const issue = rowFor("iss-1");
    const records = churn(list(), () => feed([branchRow({ unread_count: 4 }), issueRow()]));
    expect(rowFor("run-1")).toBe(branch); // patched in place, never rebuilt
    expect(rowFor("iss-1")).toBe(issue);
    expect(records.length).toBeGreaterThan(0);
    expect(records.every((record) => branch.contains(record.target))).toBe(true);
    expect(branch.querySelector(".inbox-unread").textContent).toBe("4");
  });

  it("keeps the rows already there when one arrives at the top of the list", () => {
    const branch = rowFor("run-1");
    const issue = rowFor("iss-1");
    feed([branchRow({ branch: "build/oldest", run_id: "run-oldest", anchor: hoursAgo(20) }), ...feedItems]);
    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-oldest", "run-1", "iss-1"]);
    expect(rowFor("run-1")).toBe(branch);
    expect(rowFor("iss-1")).toBe(issue);
  });

  it("keeps Recent's disclosure after the list proper", () => {
    feed([branchRow(), branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    const toggle = document.querySelector("[data-recent-toggle]");
    expect(rowFor("run-1").compareDocumentPosition(toggle) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    // Recent's own rows are its children, so each list reconciles only its own.
    expect(rowFor("run-old").parentElement.className).toBe("inbox-recent");
  });

  it("leaves Recent alone when the feed repeats itself", () => {
    feed([branchRow(), branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    const quiet = rowFor("run-old");
    expect(churn(list(), () => feed(feedItems))).toEqual([]);
    expect(rowFor("run-old")).toBe(quiet);
  });
});

// ---- captures ----------------------------------------------------------------
// A capture on the rail is a route in progress. The row is where routing is
// made visible and reversible: it answers the router, retries a route that gave
// up, and sends a capture somewhere else.

const captureFeedRow = (over = {}) => ({
  kind: "capture",
  capture_id: "capture-1",
  project_id: "",
  project: "",
  branch: null,
  issue_id: null,
  title: "fix the login redirect",
  text: "fix the login redirect",
  state: "routing",
  created_at: new Date().toISOString(),
  resume_at: new Date().toISOString(),
  unread: false,
  unread_count: 0,
  unread_reason: null,
  working: true,
  working_time: null,
  agents: [],
  stat: null,
  can_finish: false,
  muted: false,
  worktree_path: null,
  worktree_id: null,
  run_id: null,
  primary: false,
  routing: null,
  question: null,
  ...over,
});

const captureRowFor = (id) => document.querySelector(`.capture-entry[data-capture="${id}"]`);

describe("captures on the rail", () => {
  it("shows a capture the router is still deciding, and never asks the daemon to read it", async () => {
    feed([captureFeedRow()]);
    const row = captureRowFor("capture-1");
    expect(row.textContent).toContain("Deciding where this goes");
    row.click();
    await flush();
    expect(App.call).not.toHaveBeenCalledWith("entity.seen", expect.anything());
  });

  // The daemon keeps an unread row visible (unread beats dismissed), so the
  // tap reads it through first — without that, Clear would bounce back on the
  // next poll on exactly the rows people most want to clear.
  it("reads an unread row through before clearing it", async () => {
    feed([branchRow({ unread: true, unread_count: 1, unread_reason: "agent_message" })]);
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    rowFor("run-1").querySelector("[data-dismiss]").click();
    await flush();
    const calls = App.call.mock.calls.map(([method]) => method);
    expect(calls.indexOf("entity.seen")).toBeGreaterThan(-1);
    expect(calls.indexOf("entity.seen")).toBeLessThan(calls.indexOf("entity.dismiss"));
  });

  // A bare checkout holds no conversation, so the bridge clears it at the
  // commit it sits on — and a new commit brings it back. The row is named by
  // the worktree id it already carries.
  it("clears a bare checkout row by its worktree id", async () => {
    feed([branchRow({ run_id: null, issue_id: null, worktree_id: "wt-9", unread: false })]);
    const row = rowFor("wt-9");
    row.querySelector("[data-menu]").click();
    await flush();
    expect(row.querySelector("[data-mute]")).toBeTruthy();
    rowFor("wt-9").querySelector("[data-dismiss]").click();
    expect(rowFor("wt-9")).toBeNull(); // gone before the daemon answers
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.dismiss", { entity_id: "wt-9" });
  });

  // The primary checkout names no entity at all, so the clear names the row by
  // what it IS: the project's own checkout. Nothing destructive is offered
  // beside it — there is no voice to mute and nothing to finish.
  it("clears the primary row by naming the project's checkout", async () => {
    feed([branchRow({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, unread: false })]);
    const row = document.querySelector('.inbox-entry[data-key="branch:p1:main"]');
    row.querySelector("[data-menu]").click();
    await flush();
    const open = document.querySelector('.inbox-entry[data-key="branch:p1:main"]');
    expect(open.querySelector("[data-mute]")).toBeNull();
    expect(open.querySelector("[data-done]")).toBeNull();
    open.querySelector("[data-dismiss]").click();
    expect(document.querySelector('.inbox-entry[data-key="branch:p1:main"]')).toBeNull();
    await flush();
    expect(App.call).toHaveBeenCalledWith("entity.dismiss", { project_id: "p1", primary: true });
    expect(App.call).not.toHaveBeenCalledWith("entity.seen", expect.anything());
    expect(refreshFeed).toHaveBeenCalled();
  });

  it("brings the primary row back and says why when its clear fails", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "entity.dismiss") throw new Error("unknown project p1");
      return { ok: true };
    });
    feed([branchRow({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, unread: false })]);
    document.querySelector('.inbox-entry[data-key="branch:p1:main"]').querySelector("[data-menu]").click();
    await flush();
    document.querySelector('.inbox-entry[data-key="branch:p1:main"]').querySelector("[data-dismiss]").click();
    await flush();
    const row = document.querySelector('.inbox-entry[data-key="branch:p1:main"]');
    expect(row).toBeTruthy();
    const error = row.querySelector("[data-done-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("unknown project p1");
  });

  // A capture leaves the inbox by being routed, so there is nothing to clear —
  // and no entity to clear it on.
  it("offers no way to clear a capture", () => {
    feed([captureFeedRow()]);
    const row = captureRowFor("capture-1");
    expect(row.querySelector("[data-dismiss]")).toBeNull();
    expect(row.querySelector("[data-menu]")).toBeNull();
  });

  it("opens a routed capture where it was routed", async () => {
    feed([
      captureFeedRow({
        state: "routed",
        project_id: "p1",
        project: "relaydb",
        issue_id: "iss-9",
        routing: { project_id: "p1", kind: "issue", target_id: "iss-9" },
        question: { text: "which project?", asked_at: "t", answer: null },
        unread: true,
        unread_count: 1,
        unread_reason: "router_question",
      }),
    ]);
    captureRowFor("capture-1").click();
    await flush();
    expect(location.hash).toBe("#/project/p1/issue/iss-9");
  });

  // Answering the router is a decision, not a text field wedged into a row:
  // the row is the conversation entry, and it opens the page that decides.
  it("opens the decision page for a capture the router is asking about", async () => {
    feed([
      captureFeedRow({
        state: "unrouted",
        unread: true,
        unread_count: 1,
        unread_reason: "router_question",
        question: { text: "Which project?", asked_at: "t", answer: null },
      }),
    ]);
    const row = captureRowFor("capture-1");
    expect(row.textContent).toContain("Which project?");
    expect(row.querySelector("[data-capture-answer]")).toBeNull();
    row.click();
    await flush();
    expect(location.hash).toBe("#/capture/capture-1");
    expect(App.call).not.toHaveBeenCalledWith("capture.answer", expect.anything());
  });

  it("opens the decision page for a capture the router is still deciding", async () => {
    feed([captureFeedRow()]);
    captureRowFor("capture-1").click();
    await flush();
    expect(location.hash).toBe("#/capture/capture-1");
  });

  it("re-fires the router on a route that gave up", async () => {
    feed([captureFeedRow({ state: "failed", unread: true, unread_count: 1, unread_reason: "routing_failed" })]);
    captureRowFor("capture-1").querySelector("[data-capture-retry]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("capture.reroute", { capture_id: "capture-1" });
  });

  const routedCapture = (over = {}) =>
    captureFeedRow({
      state: "routed",
      project_id: "p1",
      project: "relaydb",
      issue_id: "iss-9",
      routing: { project_id: "p1", kind: "issue", target_id: "iss-9" },
      question: { text: "which project?", asked_at: "t", answer: null },
      unread: true,
      unread_reason: "router_question",
      ...over,
    });

  it("sends a capture somewhere else through the picker on its row", async () => {
    feed([routedCapture()]);
    captureRowFor("capture-1").querySelector("[data-capture-reroute]").click();
    await flush();
    const picker = captureRowFor("capture-1").querySelector(".reroute-menu");
    expect(picker).toBeTruthy();
    picker.querySelector('[data-reroute-project="p2"][data-reroute-kind="issue"]').click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("capture.reroute", { capture_id: "capture-1", project_id: "p2", kind: "issue" });
  });

  it("names the branch it is rerouted to, offering the ones the project has", async () => {
    feed([branchRow(), routedCapture()]);
    captureRowFor("capture-1").querySelector("[data-capture-reroute]").click();
    await flush();
    captureRowFor("capture-1").querySelector('[data-reroute-branch-open="p1"]').click();
    await flush();
    const field = captureRowFor("capture-1").querySelector("[data-reroute-branch]");
    expect(field).toBeTruthy();
    expect([...captureRowFor("capture-1").querySelectorAll("#reroute-branches option")].map((o) => o.value)).toEqual([
      "build/login",
    ]);
    field.value = "build/csv-export";
    captureRowFor("capture-1").querySelector('[data-reroute-project="p1"][data-reroute-kind="branch"]').click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("capture.reroute", {
      capture_id: "capture-1",
      project_id: "p1",
      kind: "branch",
      branch: "build/csv-export",
    });
  });

  // The list is rewritten whole on every feed tick, and naming a branch is
  // typing into a box that lives in it.
  it("holds the feed off the branch box while it is being typed into", async () => {
    feed([routedCapture()]);
    captureRowFor("capture-1").querySelector("[data-capture-reroute]").click();
    await flush();
    captureRowFor("capture-1").querySelector('[data-reroute-branch-open="p1"]').click();
    await flush();
    const field = captureRowFor("capture-1").querySelector("[data-reroute-branch]");
    field.focus();
    field.value = "build/csv";

    feed([routedCapture(), branchRow()]); // a tick with something new to say
    expect(captureRowFor("capture-1").querySelector("[data-reroute-branch]")).toBe(field);
    expect(rowFor("run-1")).toBeNull(); // held back while typing

    field.blur();
    feed([routedCapture(), branchRow()]);
    expect(rowFor("run-1")).toBeTruthy();
  });

  it("lets a branch go unnamed, which is the daemon naming it after what was said", async () => {
    feed([routedCapture()]);
    captureRowFor("capture-1").querySelector("[data-capture-reroute]").click();
    await flush();
    captureRowFor("capture-1").querySelector('[data-reroute-branch-open="p2"]').click();
    await flush();
    captureRowFor("capture-1").querySelector('[data-reroute-project="p2"][data-reroute-kind="branch"]').click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("capture.reroute", { capture_id: "capture-1", project_id: "p2", kind: "branch" });
  });

  it("says on the row when a reroute is refused", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "capture.reroute") throw new Error("unknown project_id: p2");
      return { ok: true };
    });
    feed([captureFeedRow({ state: "failed", unread: true, unread_reason: "routing_failed" })]);
    captureRowFor("capture-1").querySelector("[data-capture-retry]").click();
    await flush();
    const error = captureRowFor("capture-1").querySelector("[data-capture-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toContain("unknown project_id");
  });
});
