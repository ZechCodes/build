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
  { id: "p1", deviceId: "dev-1", projectKey: "dev-1/p1", name: "relaydb" },
  { id: "p2", deviceId: "dev-1", projectKey: "dev-1/p2", name: "dotfiles" },
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
  deliverFeed: () => subscriber && subscriber({ items: feedItems, projects: feedProjects }),
  primaryRunIdFor: () => null,
}));

// The rail is a mounted singleton (one inbox, for the app's whole life), so
// each test takes a fresh module graph rather than a reset switch the app would
// never call.
let App;
let adoptDeviceSession;
let setContextOffline;
let mountInboxList;
let markSeen;
let rememberDeviceFilter;
let subscribeInboxAttentionCount;
let attentionCount = 0;

// The bridge on the home device, as the last homeAnswersWith left it: the spy
// every "did the rail ask the machine this row is on?" is read off.
let homeCall;

/** What the home device answers with: adopting a session for it again is that
 *  bridge reconnecting, so a test that hands over a new call is a reconnect. */
const homeAnswersWith = (call) => {
  homeCall = call;
  adoptDeviceSession({ deviceId: "dev-1", call });
  return call;
};

// The other device on the account, answering for its own rows. Every verb in
// the rail goes to the device whose row it is on, so the rows of this one must
// never reach the home device's call.
const awayCall = vi.fn(async () => ({ ok: true }));

const flush = () => new Promise((done) => setTimeout(done, 0));
const rows = () => [...document.querySelectorAll("#inbox-list .inbox-entry")];
const rowFor = (entityId) => document.querySelector(`.inbox-entry[data-entity="${entityId}"]`);
/** An item of a row's ⋯ menu, opening the menu first when it is shut — a shut
 *  menu is not in the markup at all. */
const menuItem = (row, selector) => {
  if (!row.querySelector(selector)) row.querySelector("[data-menu]").click();
  return row.querySelector(selector);
};

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
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1/p1",
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
  deviceId: "dev-1",
  project_id: "p2",
  projectKey: "dev-1/p2",
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

/** Repaint the rail from a fresh set of rows, and whatever the daemon says it
 *  is making or removing right now. */
const feed = (items, pending = [], devices = null) => {
  feedItems = items;
  subscriber({ items, pending, projects: feedProjects, ...(devices ? { devices } : {}) });
};

beforeEach(async () => {
  vi.resetModules();
  ({ App } = await import("../src/app.js"));
  ({ adoptDeviceSession, setContextOffline } = await import("../src/core/deviceContexts.js"));
  ({ mountInboxList, markSeen } = await import("../src/core/inboxView.js"));
  ({ rememberDeviceFilter } = await import("../src/core/deviceFilter.js"));
  ({ subscribeInboxAttentionCount } = await import("../src/core/inboxAttention.js"));
  document.body.innerHTML = bodyHtml;
  document.body.className = "";
  location.hash = "";
  App.route = { name: "inbox" };
  App.gated = false;
  App.devices = [
    { id: "dev-1", name: "workshop", status: "online" },
    { id: "dev-2", name: "laptop", status: "online" },
  ];
  homeAnswersWith(vi.fn(async () => ({ ok: true })));
  awayCall.mockClear();
  adoptDeviceSession({ deviceId: "dev-2", call: awayCall });
  refreshFeed.mockClear();
  subscribeInboxAttentionCount((count) => {
    attentionCount = count;
  });
  feedItems = [branchRow(), issueRow()];
  mountInboxList();
});

afterEach(() => {
  document.getElementById("confirm-scrim")?.remove();
});

describe("the inbox rail", () => {
  it("publishes the unread message total across visible and Recent rows", () => {
    feed([
      branchRow({ unread_count: 4 }),
      issueRow({ unread: true, unread_count: 2, working: false, anchor: hoursAgo(80), last_activity: hoursAgo(40) }),
    ]);
    expect(rowFor("iss-1")).toBeNull();
    expect(attentionCount).toBe(6);

    feed([branchRow({ unread: false, unread_count: 0 }), issueRow()]);
    expect(attentionCount).toBe(0);
  });

  it("updates the unread total while a focused inbox input defers repainting", () => {
    const input = document.createElement("input");
    document.getElementById("inbox-list").appendChild(input);
    input.focus();
    feed([branchRow({ unread_count: 5 })]);
    expect(attentionCount).toBe(5);
    expect(document.activeElement).toBe(input);
  });

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

  // The daemon puts a row on the board the moment a create reaches for git and
  // replaces it with the record when the git lands. Both are the same row: the
  // placeholder carries the id the checkout will settle under.
  it("paints a checkout being cut, and lets its record settle into the same row", () => {
    feed([branchRow()], [
      {
        entity_id: "wt-new",
        project_id: "p1",
        project: "relaydb",
        title: "mascot spike",
        branch: "build/mascot-spike",
        state: "creating",
        checkout_id: null,
        implements: null,
        pending_seconds: 0,
      },
    ]);
    const cutting = rowFor("wt-new");
    expect(cutting).toBeTruthy();
    expect(cutting.querySelector(".inbox-facts").textContent).toBe("Creating…");
    expect(cutting.querySelector(".sdot").className).toContain("sdot-working");
    // Nothing is there to open yet, and nothing to act on either.
    expect(cutting.classList.contains("inbox-unroutable")).toBe(true);
    expect(cutting.querySelector("[data-menu]")).toBeNull();

    feed([
      branchRow(),
      branchRow({ branch: "build/mascot-spike", run_id: null, worktree_id: "wt-new", state: "created", unread: false }),
    ]);
    const settled = rowFor("wt-new");
    expect(settled).toBe(cutting);
    expect(settled.querySelector("[data-menu]")).toBeTruthy();
    expect(settled.querySelector(".inbox-facts").textContent).not.toBe("Creating…");
  });

  // The wire's real shape: the row names the run it settles as and the checkout
  // it holds, and the run's card is listed under its run id. Both ids have to
  // find the one card, or the placeholder is pushed as a second row under the
  // same key and the two overwrite each other.
  it("says on the card itself that its checkout is being removed", () => {
    feed([branchRow()], [
      {
        entity_id: "run-1",
        project_id: "p1",
        project: "relaydb",
        title: "build/login",
        branch: "build/login",
        state: "discarding",
        checkout_id: "wt-1",
        implements: null,
        pending_seconds: 0,
      },
    ]);
    expect(rows()).toHaveLength(1);
    expect(rowFor("run-1").querySelector(".inbox-facts").textContent).toBe("Removing…");
    expect(rowFor("run-1").querySelector("[data-menu]")).toBeNull();
  });

  // An implement that adopts a checkout the run already owns leaves the run on
  // the board AND publishes a row for it, so one snapshot carries both.
  it("keeps a run whose checkout is being claimed to one row", () => {
    feed([branchRow()], [
      {
        entity_id: "run-1",
        project_id: "p1",
        project: "relaydb",
        title: "build/login",
        branch: "build/login",
        state: "creating",
        checkout_id: "wt-1",
        implements: "iss-1",
        pending_seconds: 0,
      },
    ]);
    expect(rows()).toHaveLength(1);
    expect(rowFor("run-1").querySelector(".inbox-facts").textContent).toBe("Creating…");
    expect(rowFor("run-1").querySelector("[data-menu]")).toBeNull();
  });

  // A planning workspace is cut against the primary checkout, so its row holds
  // no checkout at all — the issue's own card is where it is said.
  it("says on an issue's own card that its planning workspace is being cut", () => {
    feed([issueRow()], [
      {
        entity_id: "iss-1",
        project_id: "p2",
        project: "dotfiles",
        title: "Rework the prompt cache",
        branch: null,
        state: "creating",
        checkout_id: null,
        implements: null,
        pending_seconds: 0,
      },
    ]);
    expect(rows()).toHaveLength(1);
    expect(rowFor("iss-1").querySelector(".inbox-facts").textContent).toBe("Creating…");
    expect(rowFor("iss-1").querySelector("[data-menu]")).toBeNull();
  });

  it("never paints work that is over", () => {
    feed([branchRow({ state: "merged" }), issueRow({ state: "archived" })]);
    expect(rows()).toEqual([]);
  });

  it("opens an entry to its work item and tells the bridge it was read", async () => {
    rowFor("run-1").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/build%2Flogin/changes");
  });

  it("opens an issue by its own id", async () => {
    rowFor("iss-1").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "iss-1" });
    expect(location.hash).toBe("#/device/dev-1/project/p2/issue/iss-1");
  });

  it("opens a project's primary checkout, which names no entity to read", async () => {
    feed([branchRow({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, unread: false })]);
    document.querySelector('#inbox-list .inbox-entry[data-key="branch:dev-1/p1:main"]').click();
    await flush();
    expect(homeCall).not.toHaveBeenCalledWith("entity.seen", expect.anything());
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/main/changes");
  });

  // A reader of a long conversation holds a window on it, not the whole
  // transcript, and the daemon needs to hear which — reading the end of a
  // window is no claim about the messages below its floor.
  it("carries the floor of the reader's window onto the wire", async () => {
    await markSeen("run-1", "ag-1", 341);
    expect(homeCall).toHaveBeenCalledWith("entity.seen", {
      entity_id: "run-1",
      agent_id: "ag-1",
      read_from_sequence: 341,
    });
  });

  // The read cursor is the daemon's, and only the machine that answered for a
  // row keeps it: a cursor sent to the wrong bridge names nothing it holds.
  it("sends a read cursor to the device holding the row", async () => {
    feed([branchRow(), branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", run_id: "run-2", worktree_id: "wt-2" })]);

    await markSeen("run-2", "ag-1", null);

    expect(awayCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-2", agent_id: "ag-1" });
    expect(homeCall).not.toHaveBeenCalledWith("entity.seen", { entity_id: "run-2", agent_id: "ag-1" });
  });

  it("names no floor for a conversation that arrived whole", async () => {
    await markSeen("run-1", "ag-1", null);
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1", agent_id: "ag-1" });
  });

  it("marks the entry the route stands on", async () => {
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
    feed(feedItems);
    expect(rowFor("run-1").className).toContain("active");
    expect(rowFor("iss-1").className).not.toContain("active");
  });

  // Every device's first project is `proj-1` and every repo has a `main`, so a
  // route that names neither can stand on two rows. The route names its
  // device, so the mark goes where the reader actually is.
  it("marks the row on the route's device", () => {
    feed([branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", run_id: "run-2", worktree_id: "wt-2" }), branchRow()]);

    App.route = { name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/login", tab: "changes" };
    feed(feedItems);
    expect(rowFor("run-2").className).toContain("active");
    expect(rowFor("run-1").className).not.toContain("active");

    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
    feed(feedItems);
    expect(rowFor("run-1").className).toContain("active");
    expect(rowFor("run-2").className).not.toContain("active");
  });

  it("deletes a branch on Done, dismisses its row at once, and reads the entry", async () => {
    menuItem(rowFor("run-1"), "[data-done]").click();
    await answerConfirm(true);
    expect(homeCall).toHaveBeenCalledWith("branch.finish", {
      project_id: "p1",
      branch: "build/login",
      action: "delete",
    });
    expect(rowFor("run-1")).toBeNull(); // gone before the daemon catches up
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
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
    menuItem(rowFor("run-1"), "[data-done]").click();
    await flush();
    const scrim = document.getElementById("confirm-scrim");
    expect(scrim.querySelector(".confirm-warnings").textContent).toContain("4 commits that main does not");
    expect(scrim.textContent).toContain("Delete branch build/login");
    scrim.querySelector("[data-confirm-cancel]").click();
    await flush();
    expect(homeCall).not.toHaveBeenCalledWith("branch.finish", expect.anything());
  });

  // An issue whose branch was deleted unmerged comes back asking for somebody:
  // clearing its cursor here would swallow the event that says what happened.
  it("reads only the branch it deleted, never the issue it hands back", async () => {
    feed([branchRow({ issue_id: "iss-9" })]);
    menuItem(rowFor("run-1"), "[data-done]").click();
    await answerConfirm(true);
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(homeCall).not.toHaveBeenCalledWith("entity.seen", { entity_id: "iss-9" });
  });

  it("keeps the row when the confirmation is declined", async () => {
    menuItem(rowFor("run-1"), "[data-done]").click();
    await answerConfirm(false);
    expect(homeCall).not.toHaveBeenCalledWith("branch.finish", expect.anything());
    expect(rowFor("run-1")).toBeTruthy();
  });

  it("takes the row off the list before branch.finish answers, and leaves its neighbours alone", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "branch.finish") return new Promise(() => {});
      return { ok: true };
    }));
    const neighbour = rowFor("iss-1");
    menuItem(rowFor("run-1"), "[data-done]").click();
    await answerConfirm(true);
    expect(rowFor("run-1")).toBeNull();
    expect(rowFor("iss-1")).toBe(neighbour);
  });

  it("keeps the row off the list when a feed tick lands while Done is in flight", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "branch.finish") return new Promise(() => {});
      return { ok: true };
    }));
    menuItem(rowFor("run-1"), "[data-done]").click();
    await answerConfirm(true);
    feed([branchRow(), issueRow()]);
    expect(rowFor("run-1")).toBeNull();
    expect(rowFor("iss-1")).toBeTruthy();
  });

  it("restores the row and says why when Done fails", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "branch.finish") throw new Error("worktree is dirty");
      return { ok: true };
    }));
    menuItem(rowFor("run-1"), "[data-done]").click();
    await answerConfirm(true);
    const row = rowFor("run-1");
    expect(row).toBeTruthy();
    const error = row.querySelector("[data-done-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("worktree is dirty");
    const notices = [...document.querySelectorAll("#notices .notice.error")];
    expect(notices).toHaveLength(1);
    expect(notices[0].textContent).toContain("Couldn't finish build/login");
    expect(notices[0].textContent).toContain("worktree is dirty");
  });

  it("archives an issue through the plan verb, warning when nothing ever implemented it", async () => {
    feed([
      issueRow({
        working: false,
        finish: { warnings: [{ code: "unimplemented", message: "No branch has implemented this issue" }] },
      }),
    ]);
    menuItem(rowFor("iss-1"), "[data-done]").click();
    await flush();
    expect(document.getElementById("confirm-scrim").querySelector(".confirm-warnings").textContent).toContain(
      "No branch has implemented this issue",
    );
    document.getElementById("confirm-scrim").querySelector("[data-confirm-ok]").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("plan.archive", { plan_id: "iss-1" });
  });

  // The reviewer's screenshot: a menu that could only be shut by choosing.
  // The ⋯ shuts it again, and a shut menu leaves the markup — the DOM patcher
  // never re-hides a split menu, so hiding it would not have worked.
  it("shuts the menu on a second press of the ⋯", async () => {
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector(".inbox-menu")).toBeTruthy();
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector(".inbox-menu")).toBeNull();
  });

  it("shuts the menu on a press anywhere outside it", async () => {
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector(".inbox-menu")).toBeTruthy();
    document.body.dispatchEvent(new Event("pointerdown", { bubbles: true }));
    await flush();
    expect(rowFor("run-1").querySelector(".inbox-menu")).toBeNull();
  });

  it("mutes an entry from its own menu, on the derived entity id", async () => {
    expect(rowFor("run-1").querySelector(".inbox-menu")).toBeNull();
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector(".inbox-menu")).toBeTruthy();
    menuItem(rowFor("run-1"), "[data-mute]").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.mute", { entity_id: "run-1", muted: true });
  });

  it("mutes an entry the instant the menu item is pressed, before entity.mute answers", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.mute") return new Promise(() => {});
      return { ok: true };
    }));
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-mute]").click();

    expect(rowFor("run-1").className).toContain("inbox-muted");
    expect(attentionCount).toBe(0);
    await flush();
    expect(rowFor("run-1").className).toContain("inbox-muted");
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector("[data-mute] .mt").textContent).toBe("Unmute");
    expect(homeCall).toHaveBeenCalledWith("entity.mute", { entity_id: "run-1", muted: true });
  });

  it("puts the mute back and says why when entity.mute is refused", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.mute") throw new Error("the relay is offline");
      return { ok: true };
    }));
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-mute]").click();
    await flush();

    const row = rowFor("run-1");
    expect(row.className).not.toContain("inbox-muted");
    expect(attentionCount).toBe(1);
    const error = row.querySelector("[data-done-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("the relay is offline");
    row.querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").querySelector("[data-mute] .mt").textContent).toBe("Mute");
  });

  it("keeps Recent open across a repaint once the user has opened it", async () => {
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

  it("starts Recent shut even when there is almost nothing above it", () => {
    feed([branchRow(), branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    expect(document.querySelector("[data-recent-toggle]").getAttribute("aria-expanded")).toBe("false");
    expect(rowFor("run-old")).toBeNull();
  });

  it("paints Recent's rows as one quiet line each, with no state dot", () => {
    feed([branchRow(), branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    document.querySelector("[data-recent-toggle]").click();
    expect(rowFor("run-1").querySelector(".sdot")).toBeTruthy();
    expect(rowFor("run-old").classList.contains("inbox-quiet")).toBe(true);
    expect(rowFor("run-old").querySelector(".sdot")).toBeNull();
    expect(rowFor("run-old").querySelector(".inbox-facts-float").textContent).toBe("2 files · +8 −1");
  });

  it("opens a Recent row like any other", async () => {
    feed([branchRow({ branch: "build/old", run_id: "run-old", anchor: hoursAgo(300), last_activity: hoursAgo(40) })]);
    document.querySelector("[data-recent-toggle]").click();
    rowFor("run-old").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-old" });
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/build%2Fold/changes");
  });

  // Clearing a row is not muting it: a muted row stays and stops asking, a
  // cleared one is off the inbox until something new needs the user. The bridge
  // owns that truth (`dismissed` on the row); the tap only gets there first.
  it("clears an entry from its own menu, and the row leaves before the daemon answers", async () => {
    homeAnswersWith(vi.fn(async (method, params) => {
      if (method !== "entity.dismiss") return { ok: true };
      feedItems = [branchRow({ dismissed: true }), issueRow()];
      return { entity_id: params.entity_id, dismissed: true };
    }));
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-dismiss]").click();
    expect(rowFor("run-1")).toBeNull();
    expect(attentionCount).toBe(0);

    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.dismiss", { entity_id: "run-1" });
    expect(refreshFeed).toHaveBeenCalled();
    expect(rowFor("run-1")).toBeNull();
    expect(rowFor("iss-1")).toBeTruthy();
    expect(location.hash).toBe("");
  });

  it("brings the row back and says why when clearing fails", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.dismiss") throw new Error("the relay is offline");
      return { ok: true };
    }));
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-dismiss]").click();
    await flush();
    const row = rowFor("run-1");
    expect(row).toBeTruthy();
    expect(attentionCount).toBe(1);
    const error = row.querySelector("[data-done-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toBe("the relay is offline");
  });

  it("keeps the row cleared through a feed tick that still carries it", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.dismiss") return new Promise(() => {});
      return { ok: true };
    }));
    const neighbour = rowFor("iss-1");
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-dismiss]").click();
    await flush();

    feed([branchRow(), issueRow()]);

    expect(rowFor("run-1")).toBeNull();
    expect(rowFor("iss-1")).toBe(neighbour);
  });

  it("refuses a second press on a row whose verb is still in flight", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.dismiss") return new Promise(() => {});
      return { ok: true };
    }));
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-dismiss]").click();
    await flush();

    feed([branchRow(), issueRow()]);
    expect(rowFor("run-1")).toBeNull();
    const dismissals = homeCall.mock.calls.filter(([method]) => method === "entity.dismiss");
    expect(dismissals).toHaveLength(1);
  });

  it("lets a cleared row come back when something new needs the user", async () => {
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-dismiss]").click();
    await flush();
    expect(rowFor("run-1")).toBeNull();

    feed([branchRow({ dismissed: true }), issueRow()]);
    expect(rowFor("run-1")).toBeNull();

    feed([branchRow({ dismissed: false, unread: true }), issueRow()]);
    expect(rowFor("run-1")).toBeTruthy();
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

  it("says which way the mute was going when it is refused", async () => {
    feed([branchRow({ muted: true, unread: false })]);
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.mute") throw new Error("the relay is offline");
      return { ok: true };
    }));
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-mute]").click();
    await flush();

    const notices = [...document.querySelectorAll("#notices .notice.error")];
    expect(notices).toHaveLength(1);
    expect(notices[0].textContent).toContain("Couldn't unmute build/login");
  });

  it("offers to unmute a muted entry, and never navigates from the menu", async () => {
    feed([branchRow({ muted: true, unread: false })]);
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    expect(rowFor("run-1").className).toContain("inbox-muted");
    menuItem(rowFor("run-1"), "[data-mute]").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.mute", { entity_id: "run-1", muted: false });
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
    toggle.click();
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
    expect(homeCall).not.toHaveBeenCalledWith("entity.seen", expect.anything());
  });

  // The daemon keeps an unread row visible (unread beats dismissed), so the
  // tap reads it through first — without that, Clear would bounce back on the
  // next poll on exactly the rows people most want to clear.
  it("reads an unread row through before clearing it", async () => {
    feed([branchRow({ unread: true, unread_count: 1, unread_reason: "agent_message" })]);
    rowFor("run-1").querySelector("[data-menu]").click();
    await flush();
    menuItem(rowFor("run-1"), "[data-dismiss]").click();
    await flush();
    const calls = homeCall.mock.calls.map(([method]) => method);
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
    menuItem(rowFor("wt-9"), "[data-dismiss]").click();
    expect(rowFor("wt-9")).toBeNull(); // gone before the daemon answers
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.dismiss", { entity_id: "wt-9" });
  });

  // The primary checkout names no entity at all, so the clear names the row by
  // what it IS: the project's own checkout. Nothing destructive is offered
  // beside it — there is no voice to mute and nothing to finish.
  it("clears the primary row by naming the project's checkout", async () => {
    feed([branchRow({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, unread: false })]);
    const row = document.querySelector('.inbox-entry[data-key="branch:dev-1/p1:main"]');
    row.querySelector("[data-menu]").click();
    await flush();
    const open = document.querySelector('.inbox-entry[data-key="branch:dev-1/p1:main"]');
    expect(open.querySelector("[data-mute]")).toBeNull();
    expect(open.querySelector("[data-done]")).toBeNull();
    open.querySelector("[data-dismiss]").click();
    expect(document.querySelector('.inbox-entry[data-key="branch:dev-1/p1:main"]')).toBeNull();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.dismiss", { project_id: "p1", primary: true });
    expect(homeCall).not.toHaveBeenCalledWith("entity.seen", expect.anything());
    expect(refreshFeed).toHaveBeenCalled();
  });

  it("brings the primary row back and says why when its clear fails", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "entity.dismiss") throw new Error("unknown project p1");
      return { ok: true };
    }));
    feed([branchRow({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, unread: false })]);
    document.querySelector('.inbox-entry[data-key="branch:dev-1/p1:main"]').querySelector("[data-menu]").click();
    await flush();
    document.querySelector('.inbox-entry[data-key="branch:dev-1/p1:main"]').querySelector("[data-dismiss]").click();
    await flush();
    const row = document.querySelector('.inbox-entry[data-key="branch:dev-1/p1:main"]');
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
    expect(homeCall).not.toHaveBeenCalledWith("capture.answer", expect.anything());
  });

  it("opens the decision page for a capture the router is still deciding", async () => {
    feed([captureFeedRow()]);
    captureRowFor("capture-1").click();
    await flush();
    expect(location.hash).toBe("#/capture/capture-1");
  });

  it("retries one failed route while another retry is still in flight", async () => {
    const failed = (id) =>
      captureFeedRow({
        capture_id: id,
        state: "failed",
        unread: true,
        unread_count: 1,
        unread_reason: "routing_failed",
      });
    feed([failed("capture-1"), failed("capture-2")]);
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "capture.reroute") return new Promise(() => {});
      return {};
    }));

    captureRowFor("capture-1").querySelector("[data-capture-retry]").click();
    await flush();
    captureRowFor("capture-2").querySelector("[data-capture-retry]").click();
    await flush();

    const rerouted = homeCall.mock.calls.filter(([method]) => method === "capture.reroute");
    expect(rerouted.map(([, params]) => params.capture_id)).toEqual(["capture-1", "capture-2"]);
  });

  it("re-fires the router on a route that gave up", async () => {
    feed([captureFeedRow({ state: "failed", unread: true, unread_count: 1, unread_reason: "routing_failed" })]);
    captureRowFor("capture-1").querySelector("[data-capture-retry]").click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("capture.reroute", { capture_id: "capture-1" });
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
    expect(homeCall).toHaveBeenCalledWith("capture.reroute", { capture_id: "capture-1", project_id: "p2", kind: "issue" });
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
    expect(homeCall).toHaveBeenCalledWith("capture.reroute", {
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
    expect(homeCall).toHaveBeenCalledWith("capture.reroute", { capture_id: "capture-1", project_id: "p2", kind: "branch" });
  });

  it("says on the row when a reroute is refused", async () => {
    homeAnswersWith(vi.fn(async (method) => {
      if (method === "capture.reroute") throw new Error("unknown project_id: p2");
      return { ok: true };
    }));
    feed([captureFeedRow({ state: "failed", unread: true, unread_reason: "routing_failed" })]);
    captureRowFor("capture-1").querySelector("[data-capture-retry]").click();
    await flush();
    const error = captureRowFor("capture-1").querySelector("[data-capture-error]");
    expect(error.hidden).toBe(false);
    expect(error.textContent).toContain("unknown project_id");
  });
});

// Every row in the rail names the machine that answered for it, and its route
// names that machine too: a row from another device is the account's row as
// much as any other — it opens, and everything it does it does against ITS
// device.
describe("a row on another device", () => {
  const awayRow = (over = {}) =>
    branchRow({ deviceId: "dev-2", projectKey: "dev-2/p1", run_id: "run-2", branch: "build/away", ...over });

  it("opens on its own device and is read there", async () => {
    feed([awayRow()]);
    const row = rowFor("run-2");
    expect(row.className).not.toContain("inbox-unroutable");

    row.click();
    await flush();
    expect(location.hash).toBe("#/device/dev-2/project/p1/branch/build%2Faway/changes");
    expect(awayCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-2" });
    expect(homeCall).not.toHaveBeenCalledWith("entity.seen", expect.anything());
  });

  it("clears, finishes and mutes on its own device", async () => {
    feed([awayRow(), awayRow({ run_id: "run-3", branch: "build/other" }), awayRow({ run_id: "run-4", branch: "build/third" })]);
    menuItem(rowFor("run-2"), "[data-dismiss]").click();
    await flush();
    expect(awayCall).toHaveBeenCalledWith("entity.dismiss", { entity_id: "run-2" });

    menuItem(rowFor("run-3"), "[data-done]").click();
    await answerConfirm(true);
    expect(awayCall).toHaveBeenCalledWith("branch.finish", { project_id: "p1", branch: "build/other", action: "delete" });

    menuItem(rowFor("run-4"), "[data-mute]").click();
    await flush();
    expect(awayCall).toHaveBeenCalledWith("entity.mute", { entity_id: "run-4", muted: true });

    expect(homeCall).not.toHaveBeenCalledWith("entity.dismiss", expect.anything());
    expect(homeCall).not.toHaveBeenCalledWith("branch.finish", expect.anything());
    expect(homeCall).not.toHaveBeenCalledWith("entity.mute", expect.anything());
  });

  it("is greyed and its verbs are shut while its device is offline", async () => {
    setContextOffline("dev-2", { offline: true });
    feed([awayRow()]);
    const row = rowFor("run-2");
    expect(row.className).toContain("inbox-offline");
    const clear = menuItem(row, "[data-dismiss]");
    expect(clear.getAttribute("aria-disabled")).toBe("true");
    expect(clear.title).toBe("Device offline");

    clear.click();
    await flush();
    expect(awayCall).not.toHaveBeenCalledWith("entity.dismiss", expect.anything());
  });

  // Grey on its own is not a mark: a dimmed row reads as "this matters less",
  // not as "this machine is not here". The row says the word, where its own
  // tags go, and the word goes the moment the machine answers again.
  it("wears the word offline while its device is away", async () => {
    setContextOffline("dev-2", { offline: true });
    feed([branchRow(), awayRow()]);

    expect(rowFor("run-2").querySelector(".inbox-away").textContent).toBe("offline");
    expect(rowFor("run-1").querySelector(".inbox-away")).toBe(null);

    setContextOffline("dev-2", { offline: false });

    expect(rowFor("run-2").querySelector(".inbox-away")).toBe(null);
  });

  // A reroute goes to the machine holding the capture, and that daemon knows
  // only the projects it minted itself — every machine has a `p1`. So the
  // picker offers that device's projects and the branches they already have.
  it("offers its own device's projects when its capture is rerouted", async () => {
    const theirProject = { id: "p1", deviceId: "dev-2", projectKey: "dev-2/p1", name: "their notes" };
    const theirCapture = captureFeedRow({
      deviceId: "dev-2",
      projectKey: "dev-2/p1",
      state: "routed",
      project_id: "p1",
      project: "their notes",
      issue_id: "iss-9",
      routing: { project_id: "p1", kind: "issue", target_id: "iss-9" },
    });
    const mine = { items: [branchRow()], projects: feedProjects };
    const theirs = { items: [awayRow(), theirCapture], projects: [theirProject] };
    feed([...mine.items, ...theirs.items], [], { "dev-1": mine, "dev-2": theirs });

    captureRowFor("capture-1").querySelector("[data-capture-reroute]").click();
    await flush();
    const picker = captureRowFor("capture-1").querySelector(".reroute-menu");
    expect([...picker.querySelectorAll(".reroute-project .mt")].map((name) => name.textContent)).toEqual(["their notes"]);

    picker.querySelector('[data-reroute-branch-open="p1"]').click();
    await flush();
    expect([...captureRowFor("capture-1").querySelectorAll("#reroute-branches option")].map((option) => option.value)).toEqual([
      "build/away",
    ]);
  });

  // The picker narrows what is LISTED and nothing else: the surface you are
  // standing on is about one machine already, and hiding its rows must not move
  // you off it.
  it("the filter hides the other device's rows and never changes the route", async () => {
    App.route = { name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login", tab: "changes" };
    location.hash = "#/device/dev-1/project/p1/branch/build%2Flogin/changes";
    const standing = App.route;
    feed([branchRow(), awayRow()]);
    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-1", "run-2"]);

    rememberDeviceFilter("dev-2");

    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-2"]);
    expect(App.route).toBe(standing);
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/build%2Flogin/changes");

    rememberDeviceFilter(null);

    expect(rows().map((row) => row.dataset.entity)).toEqual(["run-1", "run-2"]);
  });

  it("leaves the home device's own rows alone", async () => {
    feed([branchRow(), awayRow()]);
    const home = rowFor("run-1");
    expect(home.className).not.toContain("inbox-offline");
    expect(menuItem(home, "[data-dismiss]").getAttribute("aria-disabled")).toBe(null);

    home.click();
    await flush();
    expect(homeCall).toHaveBeenCalledWith("entity.seen", { entity_id: "run-1" });
    expect(location.hash).toBe("#/device/dev-1/project/p1/branch/build%2Flogin/changes");
  });
});
