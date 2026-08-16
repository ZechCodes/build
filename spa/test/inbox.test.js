// The inbox's pure model: one list across every project, in the anchor's order,
// two lines per row, the Recent partition at the end, and what Done promises
// before it destroys anything.

import { describe, it, expect } from "vitest";
import {
  RECENT_AUTO_OPEN_BELOW,
  activeEntryKey,
  branchDoneConfirm,
  entryFactsText,
  entryRoute,
  entryState,
  inboxEntries,
  inboxListHtml,
  inboxRowHtml,
  issueDoneConfirm,
  unreadReasonText,
} from "../src/core/inbox.js";

const NOW = Date.parse("2026-08-13T12:00:00Z");
const ago = (hours) => new Date(NOW - hours * 3600 * 1000).toISOString();

const branch = (over = {}) => ({
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  title: "Fix the login flow",
  state: "building",
  unread: false,
  unread_count: 0,
  unread_reason: null,
  working: false,
  working_time: null,
  agents: [],
  stat: {
    files_changed: 3,
    insertions: 42,
    deletions: 7,
    uncommitted: { files_changed: 0, insertions: 0, deletions: 0 },
    ahead: 0,
    behind: 0,
    upstream: "origin/build/login",
    comparison_ref: "origin/build/login",
  },
  anchor: ago(3),
  last_activity: ago(1),
  resume_at: ago(1),
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

const issue = (over = {}) => ({
  kind: "issue",
  project_id: "p2",
  project: "dotfiles",
  branch: null,
  title: "Rework the prompt cache",
  state: "plan_review",
  unread: false,
  unread_count: 0,
  unread_reason: null,
  working: false,
  working_time: null,
  agents: [],
  stat: null,
  anchor: ago(2),
  last_activity: ago(2),
  resume_at: ago(2),
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

/** The main list, which is what "the inbox" means everywhere but Recent. */
const listed = (items, over = {}) => inboxEntries({ items, nowMs: NOW, ...over }).entries;

describe("an entry's state", () => {
  it("is unread when an attention event is waiting, whatever else is true", () => {
    expect(entryState(branch({ unread: true, working: true }))).toBe("unread");
  });

  it("is working while an agent is running on it", () => {
    expect(entryState(branch({ working: true }))).toBe("working");
  });

  it("is inactive otherwise", () => {
    expect(entryState(branch())).toBe("inactive");
  });
});

describe("why an entry needs you", () => {
  it("says what the attention event was", () => {
    expect(unreadReasonText("done", "branch")).toBe("The agent finished — review the work");
    expect(unreadReasonText("blocked", "branch")).toBe("Blocked — the agent needs you");
    expect(unreadReasonText("agent_message", "branch")).toBe("The agent sent a message");
  });

  it("speaks an issue's own vocabulary for the same event", () => {
    expect(unreadReasonText("done", "issue")).toBe("The draft is ready to review");
  });

  it("falls back to something honest for a kind it has never heard of", () => {
    expect(unreadReasonText("some_new_event", "branch")).toBe("Something needs you");
    expect(unreadReasonText(null, "branch")).toBe("");
  });
});

// ---- the order ---------------------------------------------------------------
// The anchor is seeded at creation and moved only by the user picking the work
// back up. Oldest first: the top of the list is what has been waiting longest,
// and a fresh pickup appends to the bottom instead of shoving everything down.

describe("the order the list reads in", () => {
  it("is the anchor's, oldest first, whatever order the rows arrived in", () => {
    const entries = listed([
      branch({ branch: "build/new", run_id: "run-new", anchor: ago(1) }),
      issue({ anchor: ago(200) }),
      branch({ branch: "build/mid", run_id: "run-mid", anchor: ago(50) }),
    ]);
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1", "run-mid", "run-new"]);
  });

  // Nothing but a user message moves an anchor, so an agent working all night,
  // a diff landing and a doc being read must all leave a row exactly where it
  // is. The list is the same list after any of it.
  it("does not move a row because something happened to it", () => {
    const before = listed([branch({ anchor: ago(9) }), issue({ anchor: ago(4) })]);
    const after = listed([
      branch({ anchor: ago(9), working: true, unread: true, unread_count: 4, unread_reason: "done", last_activity: ago(0) }),
      issue({ anchor: ago(4) }),
    ]);
    expect(after.map((entry) => entry.key)).toEqual(before.map((entry) => entry.key));
  });

  // A capture hands its anchor to the work it becomes, so what you said and
  // what it turned into are one entry in the list, in one place.
  it("keeps a routed capture's place for the issue it became", () => {
    const captured = ago(30);
    const entries = listed([
      branch({ anchor: ago(2) }),
      issue({ anchor: captured, issue_id: "iss-from-capture" }),
    ]);
    expect(entries[0].issueId).toBe("iss-from-capture");
  });

  // Unknown age is not evidence of being old (the bridge's own rule).
  it("puts a row nobody can date under the ones somebody can", () => {
    const entries = listed([
      branch({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, anchor: null }),
      issue({ anchor: ago(1) }),
    ]);
    expect(entries.map((entry) => entry.key)).toEqual(["iss-1", "branch:p1:main"]);
  });
});

// ---- what is a row at all ------------------------------------------------------

describe("what the inbox lists", () => {
  it("lists issues, branches and captures in one list across every project", () => {
    const entries = listed([branch(), issue()]);
    // The branch was taken on an hour before the issue, so it reads first.
    expect(entries.map((entry) => entry.kind)).toEqual(["branch", "issue"]);
    expect(entries.map((entry) => entry.project)).toEqual(["relaydb", "dotfiles"]);
  });

  // One piece of work, one row: while a branch is implementing an issue, the
  // branch is where that work is.
  it("hides an issue while a branch is implementing it", () => {
    const entries = listed([issue({ implementing_branch: "build/cache", implementation_active: true })]);
    expect(entries).toEqual([]);
  });

  // Delete that branch without merging and the issue is work again — the bridge
  // stops calling it implemented, and the row comes straight back.
  it("brings the issue back when the branch that was implementing it is gone", () => {
    const entries = listed([issue({ implementing_branch: "build/cache", implementation_active: false })]);
    expect(entries.map((entry) => entry.issueId)).toEqual(["iss-1"]);
  });

  it("never lists anything that is over", () => {
    expect(listed([branch({ state: "merged" })])).toEqual([]);
    expect(listed([branch({ state: "abandoned" })])).toEqual([]);
    expect(listed([issue({ state: "archived" })])).toEqual([]);
  });

  it("leaves out what the user just said Done to", () => {
    const entries = inboxEntries({ items: [branch(), issue()], nowMs: NOW, hiddenEntityIds: new Set(["run-1"]) }).entries;
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1"]);
  });

  it("carries a project's primary checkout — it is the project's own row", () => {
    const entries = listed([
      branch({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, anchor: ago(1) }),
    ]);
    expect(entries.length).toBe(1);
    expect(entries[0].branch).toBe("main");
    // The repository takes no attention, so the row names no entity — and it
    // still has a key to be opened by, and no verbs it cannot perform.
    expect(entries[0].entityId).toBeNull();
    expect(entries[0].key).toBe("branch:p1:main");
    expect(entries[0].route).toEqual({ name: "branch", projectId: "p1", branch: "main", tab: "changes" });
    const html = inboxRowHtml(entries[0], {});
    expect(html).toContain('data-key="branch:p1:main"');
    expect(html).not.toContain("data-entity");
    expect(html).not.toContain("data-menu");
  });

  it("routes an issue to its own surface", () => {
    expect(entryRoute(issue())).toEqual({ name: "issue", projectId: "p2", id: "iss-1" });
  });

  it("has nowhere to send a detached checkout — it has no branch to name", () => {
    expect(entryRoute(branch({ branch: null, run_id: null }))).toBeNull();
  });
});

// ---- Recent --------------------------------------------------------------------
// Everything that has said nothing for a day is still there; it is just not what
// today is about, so it sits behind one disclosure at the end of the list.

describe("the Recent section", () => {
  const quiet = () => branch({ branch: "build/old", run_id: "run-old", anchor: ago(200), last_activity: ago(30) });

  it("takes everything whose last activity is over a day old", () => {
    const { entries, recent } = inboxEntries({ items: [branch(), quiet()], nowMs: NOW });
    expect(entries.map((entry) => entry.entityId)).toEqual(["run-1"]);
    expect(recent.map((entry) => entry.entityId)).toEqual(["run-old"]);
  });

  it("keeps the anchor's order inside itself", () => {
    const { recent } = inboxEntries({
      items: [
        branch({ branch: "build/b", run_id: "run-b", anchor: ago(40), last_activity: ago(30) }),
        branch({ branch: "build/a", run_id: "run-a", anchor: ago(90), last_activity: ago(30) }),
      ],
      nowMs: NOW,
    });
    expect(recent.map((entry) => entry.entityId)).toEqual(["run-a", "run-b"]);
  });

  it("opens itself when the inbox proper is nearly empty", () => {
    expect(inboxEntries({ items: [branch(), quiet()], nowMs: NOW }).autoOpen).toBe(true);
  });

  it("stays shut when the inbox proper has enough to read", () => {
    const busy = Array.from({ length: RECENT_AUTO_OPEN_BELOW }, (_, index) =>
      branch({ branch: `build/live-${index}`, run_id: `run-live-${index}`, anchor: ago(index + 1) }),
    );
    expect(inboxEntries({ items: [...busy, quiet()], nowMs: NOW }).autoOpen).toBe(false);
  });

  it("is one disclosure at the end of the list, counting what is behind it", () => {
    const partition = inboxEntries({ items: [branch(), quiet()], nowMs: NOW });
    const shut = inboxListHtml(partition, { recentOpen: false });
    expect(shut).toContain("data-recent-toggle");
    expect(shut).toContain("Recent");
    expect(shut).toContain('aria-expanded="false"');
    expect(shut).not.toContain('data-entity="run-old"');
    // The disclosure comes after the list proper, never before it.
    expect(shut.indexOf('data-entity="run-1"')).toBeLessThan(shut.indexOf("data-recent-toggle"));

    const open = inboxListHtml(partition, { recentOpen: true });
    expect(open).toContain('data-entity="run-old"');
    expect(open).toContain('aria-expanded="true"');
  });

  it("follows its own auto-open when nobody has said otherwise", () => {
    const partition = inboxEntries({ items: [branch(), quiet()], nowMs: NOW });
    expect(inboxListHtml(partition, {})).toContain('data-entity="run-old"');
  });

  it("says nothing at all when nothing has gone quiet", () => {
    expect(inboxListHtml(inboxEntries({ items: [branch()], nowMs: NOW }), {})).not.toContain("data-recent-toggle");
  });

  it("says so when there is nothing waiting at all", () => {
    expect(inboxListHtml(inboxEntries({ items: [], nowMs: NOW }), {})).toContain("Nothing needs you");
  });
});

// ---- cleared from the inbox ----------------------------------------------------
// A row the user cleared is GONE, not demoted: Recent is where a quiet row goes,
// and a cleared one is in neither list until something new needs the user. It is
// nothing like mute, which keeps the row and stops it asking.

describe("a row the user cleared", () => {
  it("is in neither the list nor Recent", () => {
    const { entries, recent } = inboxEntries({ items: [branch({ dismissed: true }), issue()], nowMs: NOW });
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1"]);
    expect(recent).toEqual([]);
  });

  it("is not demoted to Recent by having gone quiet either", () => {
    const cleared = branch({
      branch: "build/old",
      run_id: "run-old",
      anchor: ago(200),
      last_activity: ago(30),
      dismissed: true,
    });
    const { entries, recent } = inboxEntries({ items: [issue(), cleared], nowMs: NOW });
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1"]);
    expect(recent).toEqual([]);
  });

  it("keeps a row the bridge has not cleared, and carries what it said", () => {
    const [entry] = listed([branch({ unread: true, unread_count: 2, unread_reason: "done", dismissed: false })]);
    expect(entry.entityId).toBe("run-1");
    expect(entry.dismissed).toBe(false);
  });

  it("keeps a muted row — muting silences a row, clearing removes it", () => {
    const entries = listed([branch({ muted: true })]);
    expect(entries.map((entry) => entry.entityId)).toEqual(["run-1"]);
    expect(entries[0].muted).toBe(true);
  });

  it("is offered above Mute in the row's own menu, in the words of what it does", () => {
    const [entry] = listed([branch()]);
    const html = inboxRowHtml(entry, { openMenuKey: "run-1" });
    expect(html).toContain('data-dismiss="run-1"');
    expect(html).toContain("Clear from inbox");
    expect(html).toContain("Hides it until something new needs you");
    expect(html.indexOf("Clear from inbox")).toBeLessThan(html.indexOf(">Mute<"));
  });
});

describe("the active entry", () => {
  const { entries, recent } = inboxEntries({ items: [branch(), issue()], nowMs: NOW });
  const all = [...entries, ...recent];

  it("is the one the route is standing on", () => {
    expect(activeEntryKey({ name: "branch", projectId: "p1", branch: "build/login" }, all)).toBe("run-1");
    expect(activeEntryKey({ name: "issue", projectId: "p2", id: "iss-1" }, all)).toBe("iss-1");
  });

  it("is nothing on a route that names no work item", () => {
    expect(activeEntryKey({ name: "inbox" }, all)).toBeNull();
  });
});

// ---- the two lines --------------------------------------------------------------

describe("what a row says", () => {
  it("names the branch on line one, with the unread count at the right edge", () => {
    const [entry] = listed([branch({ unread: true, unread_count: 3, unread_reason: "done" })]);
    expect(entry.name).toBe("build/login");
    const html = inboxRowHtml(entry, {});
    const line = html.match(/<div class="inbox-line inbox-name">.*?<\/div>/s)[0];
    expect(line).toContain("build/login");
    expect(line).toContain("inbox-unread");
    expect(line).toContain(">3<");
    // Pulled right: the count is the last thing on the line.
    expect(line.indexOf("build/login")).toBeLessThan(line.indexOf("inbox-unread"));
    expect(html).toContain("sdot-unread");
    expect(html).toContain('data-entity="run-1"');
  });

  it("names the issue on line one, and says nothing about an unread count it has none of", () => {
    const [entry] = listed([issue()]);
    expect(entry.name).toBe("Rework the prompt cache");
    expect(inboxRowHtml(entry, {})).not.toContain("inbox-unread");
  });

  it("puts the files, the ahead/behind and the +/− on line two", () => {
    const [entry] = listed([branch({ stat: { ...branch().stat, ahead: 2, behind: 1 } })]);
    expect(entry.facts).toBe("3 files · ↑2 ↓1 · +42 −7");
    expect(inboxRowHtml(entry, {})).toContain("3 files · ↑2 ↓1 · +42 −7");
  });

  it("counts one file as one file", () => {
    expect(entryFactsText({ stat: { files_changed: 1, insertions: 1, deletions: 0 } })).toBe("1 file · +1 −0");
  });

  it("says nothing about a branch that is level with what it is compared to", () => {
    expect(entryFactsText({ stat: { files_changed: 2, insertions: 3, deletions: 4, ahead: 0, behind: 0 } })).toBe(
      "2 files · +3 −4",
    );
  });

  // An issue has no checkout and no commits: there is nothing to weigh yet.
  it("says Getting started when there is nothing for line two", () => {
    const [entry] = listed([issue()]);
    expect(entry.facts).toBe("");
    expect(inboxRowHtml(entry, {})).toContain("Getting started");
    const [fresh] = listed([branch({ stat: { files_changed: 0, insertions: 0, deletions: 0, ahead: 0, behind: 0 } })]);
    expect(inboxRowHtml(fresh, {})).toContain("Getting started");
  });

  it("keeps the row to two lines — what it is, and what it weighs", () => {
    const [entry] = listed([branch({ unread: true, unread_count: 1, unread_reason: "blocked" })]);
    const html = inboxRowHtml(entry, {});
    expect(html.match(/class="inbox-line/g).length).toBe(1);
    expect(html).toContain("inbox-facts");
    // The project and why it needs you are still said — on the row's own title,
    // where a second look finds them and a first glance is not spent on them.
    expect(html).toContain("relaydb");
    expect(html).toContain("Blocked — the agent needs you");
  });

  it("offers Done whenever there is something to finish", () => {
    const [nothing] = listed([branch({ can_finish: false })]);
    expect(inboxRowHtml(nothing, {})).not.toContain("data-done=");
    const [finishable] = listed([branch()]);
    expect(inboxRowHtml(finishable, {})).toContain('data-done="run-1"');
  });

  it("offers mute in the entry's own menu, and says so when it is already muted", () => {
    const [entry] = listed([branch({ muted: true })]);
    const html = inboxRowHtml(entry, { openMenuKey: "run-1" });
    expect(html).toContain('data-mute="run-1"');
    expect(html).toContain("Unmute");
    expect(html).toContain("inbox-muted");
    // The open menu is the one whose row was asked for; a shut one is hidden.
    expect(html).toMatch(/class="splitmenu inbox-menu">/);
    expect(inboxRowHtml(entry, {})).toMatch(/class="splitmenu inbox-menu" hidden>/);
  });

  it("escapes everything the repo named", () => {
    const [entry] = listed([branch({ branch: '<img src=x onerror="alert(1)">' })]);
    const html = inboxRowHtml(entry, {});
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });
});

// ---- Done ------------------------------------------------------------------------
// Done on a branch deletes it. Done on an issue archives it. Neither is refused:
// what the destruction would cost travels with the row, and the confirmation is
// where the user reads it.

describe("the Done confirmations", () => {
  const warned = (warnings) => listed([branch({ finish: { warnings } })])[0];

  it("outlines the deletion, in the order it happens", () => {
    const confirm = branchDoneConfirm(listed([branch()])[0]);
    expect(confirm.title).toContain("build/login");
    expect(confirm.actions).toEqual([
      "Delete branch build/login",
      "Remove its checkout",
      "Take its conversation off the inbox",
    ]);
    expect(confirm.confirmLabel).toBe("Delete");
    expect(confirm.danger).toBe(true);
  });

  it("carries the bridge's warning about work that would go with it", () => {
    const confirm = branchDoneConfirm(
      warned([
        { code: "unpushed", message: "build/login has 3 commits that origin/build/login does not", count: 3, ref: "origin/build/login" },
      ]),
    );
    expect(confirm.warnings).toEqual(["build/login has 3 commits that origin/build/login does not"]);
  });

  it("carries every warning the row has, and says nothing when it has none", () => {
    const confirm = branchDoneConfirm(
      warned([
        { code: "uncommitted", message: "build/login has 2 uncommitted files — removing the checkout discards them" },
        { code: "unmerged", message: "build/login has never been pushed, and has 4 commits that main does not" },
      ]),
    );
    expect(confirm.warnings.length).toBe(2);
    expect(confirm.warnings[1]).toContain("main does not");
    expect(branchDoneConfirm(listed([branch()])[0]).warnings).toEqual([]);
  });

  // Deleting an unmerged branch hands its issue back to the inbox, with an
  // event naming the branch it lost. The outline says so before the click.
  it("says the issue it implements comes back to the inbox", () => {
    const [entry] = listed([branch({ issue_id: "iss-7" })]);
    const outline = branchDoneConfirm(entry).actions.join(" ");
    expect(outline).toContain("issue");
    expect(outline).toContain("build/login");
  });

  // A merged branch is not on the inbox — it is over — but the branch surface
  // still stands in one, and there Done files the issue away with it.
  it("says the issue is filed away instead once the work is merged", () => {
    const merged = branchDoneConfirm({ branch: "build/login", issueId: "iss-7", merged: true, warnings: [] });
    expect(merged.actions.join(" ")).toContain("Archive the issue");
  });

  it("says nothing about an issue when the branch implements none", () => {
    const [entry] = listed([branch()]);
    expect(branchDoneConfirm(entry).actions.some((action) => action.toLowerCase().includes("issue"))).toBe(false);
  });

  it("archives an issue with its stage plans", () => {
    const [entry] = listed([issue()]);
    const confirm = issueDoneConfirm(entry);
    expect(confirm.actions.join(" ")).toContain("stage plans");
    expect(confirm.danger).toBe(false);
  });

  it("warns when no branch ever implemented the issue being filed away", () => {
    const [entry] = listed([
      issue({ finish: { warnings: [{ code: "unimplemented", message: "No branch has implemented this issue" }] } }),
    ]);
    expect(issueDoneConfirm(entry).warnings).toEqual(["No branch has implemented this issue"]);
  });
});

// ---- captures ----------------------------------------------------------------
// A capture is unfinished business until the router reaches a destination, so
// it is a row of the inbox like anything else — sorted by the same anchor, with
// its own states, and the two things a user can do about a route: answer, and
// send it somewhere else.

const captureItem = (over = {}) => ({
  kind: "capture",
  capture_id: "capture-1",
  project_id: "",
  project: "",
  branch: null,
  issue_id: null,
  title: "fix the login redirect",
  text: "fix the login redirect",
  state: "routing",
  created_at: ago(0),
  anchor: ago(0),
  last_activity: ago(0),
  resume_at: ago(0),
  unread: false,
  unread_count: 0,
  unread_reason: null,
  working: true,
  working_time: null,
  agents: [],
  stat: null,
  can_finish: false,
  finish: { warnings: [] },
  muted: false,
  worktree_path: null,
  worktree_id: null,
  run_id: null,
  primary: false,
  routing: null,
  question: null,
  ...over,
});

describe("capture rows", () => {
  const entryOf = (item) => listed([item])[0];

  it("sort by the same anchor as everything else", () => {
    const entries = listed([branch({ anchor: ago(2) }), captureItem({ anchor: ago(40), last_activity: ago(1) })]);
    expect(entries.map((entry) => entry.kind)).toEqual(["capture", "branch"]);
  });

  // One this client is holding for an absent device has no record yet, so no
  // anchor — and it is still dated by when the user said it.
  it("sort a capture this client is holding by when it was taken", () => {
    const entries = listed([
      branch({ anchor: ago(2) }),
      captureItem({ state: "queued", anchor: null, created_at: ago(9), last_activity: null }),
    ]);
    expect(entries.map((entry) => entry.kind)).toEqual(["capture", "branch"]);
  });

  it("name themselves by the capture, never by the destination it was routed to", () => {
    const entry = entryOf(
      captureItem({ state: "routed", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" }, issue_id: "iss-9" }),
    );
    expect(entry.key).toBe("capture:capture-1");
    expect(entry.entityId).toBeNull(); // a capture takes no attention cursor and no mute
  });

  it("read as working while nothing has decided about them yet", () => {
    expect(entryOf(captureItem({ state: "queued" })).state).toBe("working");
    expect(entryOf(captureItem({ state: "unrouted" })).state).toBe("working");
    expect(entryOf(captureItem({ state: "routing" })).state).toBe("working");
  });

  it("say what is happening to them, in the order it happens", () => {
    expect(inboxRowHtml(entryOf(captureItem({ state: "queued" })), {})).toContain("Waiting for your device");
    expect(inboxRowHtml(entryOf(captureItem({ state: "routing" })), {})).toContain("Deciding where this goes");
    expect(inboxRowHtml(entryOf(captureItem({ state: "routing" })), {})).toContain("capture-spinner");
  });

  it("say where a routed capture went, and offer to send it somewhere else", () => {
    const entry = entryOf(
      captureItem({
        state: "routed",
        project: "relaydb",
        routing: { project_id: "p1", kind: "issue", target_id: "iss-9" },
        issue_id: "iss-9",
      }),
    );
    const html = inboxRowHtml(entry, {});
    expect(html).toContain("→ relaydb as issue");
    expect(html).toContain('data-capture-reroute="capture-1"');
  });

  it("offer a retry, and nothing else, when routing failed", () => {
    const entry = entryOf(captureItem({ state: "failed", unread: true, unread_count: 1, unread_reason: "routing_failed" }));
    expect(entry.state).toBe("unread");
    const html = inboxRowHtml(entry, {});
    expect(html).toContain("Routing failed");
    expect(html).toContain('data-capture-retry="capture-1"');
    expect(html).not.toContain("data-capture-answer");
  });

  it("say the router asked, and open the page that answers it", () => {
    const entry = entryOf(
      captureItem({
        state: "unrouted",
        unread: true,
        unread_count: 1,
        unread_reason: "router_question",
        question: { text: "Which project is the login redirect in?", asked_at: ago(0), answer: null },
      }),
    );
    expect(entry.reason).toBe("Which project is the login redirect in?");
    // The row is where the question is seen; deciding it is its own surface.
    expect(entry.route).toEqual({ name: "capture", id: "capture-1" });
    const html = inboxRowHtml(entry, {});
    expect(html).toContain("Which project is the login redirect in?");
    expect(html).toContain("Waiting for your answer");
    expect(html).not.toContain("capture-spinner"); // nobody is working on it — it is waiting on you
    expect(html).not.toContain("data-capture-answer");
  });

  it("open the decision page for every capture nothing has decided yet", () => {
    expect(entryOf(captureItem({ state: "unrouted" })).route).toEqual({ name: "capture", id: "capture-1" });
    expect(entryOf(captureItem({ state: "routing" })).route).toEqual({ name: "capture", id: "capture-1" });
    expect(entryOf(captureItem({ state: "failed" })).route).toEqual({ name: "capture", id: "capture-1" });
    // One this client is still holding has no record to decide about yet.
    expect(entryOf(captureItem({ state: "queued" })).route).toBeNull();
    // And one that reached a destination opens the work it became.
    expect(
      entryOf(
        captureItem({
          state: "routed",
          project_id: "p1",
          issue_id: "iss-9",
          routing: { project_id: "p1", kind: "issue", target_id: "iss-9" },
        }),
      ).route,
    ).toEqual({ name: "issue", projectId: "p1", id: "iss-9" });
  });

  it("mark the row the decision page is standing on", () => {
    const entries = listed([captureItem({ state: "unrouted" }), branch()]);
    expect(activeEntryKey({ name: "capture", id: "capture-1" }, entries)).toBe("capture:capture-1");
    expect(activeEntryKey({ name: "capture", id: "capture-9" }, entries)).toBeNull();
  });

  it("open the destination picker on the row that asked for it", () => {
    const entry = entryOf(
      captureItem({ state: "routed", project: "relaydb", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }),
    );
    const projects = [
      { id: "p1", name: "relaydb" },
      { id: "p2", name: "dotfiles" },
    ];
    const html = inboxRowHtml(entry, { rerouteKey: entry.key, projects });
    expect(html).toContain('data-reroute-project="p2"');
    expect(html).toContain('data-reroute-kind="issue"');
    expect(html).toContain('data-reroute-branch-open="p2"');
    expect(inboxRowHtml(entry, { projects })).not.toContain("data-reroute-project");
  });

  it("take the branch's name in the picker, from the branches the project has", () => {
    const entry = entryOf(
      captureItem({ state: "routed", project: "relaydb", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }),
    );
    const projects = [
      { id: "p1", name: "relaydb" },
      { id: "p2", name: "dotfiles" },
    ];
    const shut = inboxRowHtml(entry, { rerouteKey: entry.key, projects });
    expect(shut).not.toContain('data-reroute-branch="');

    const open = inboxRowHtml(entry, {
      rerouteKey: entry.key,
      projects,
      rerouteBranchProject: "p2",
      rerouteBranches: ["build/login", "build/toast"],
    });
    expect(open).toContain('data-reroute-branch="p2"');
    expect(open).toContain('<option value="build/toast">');
    // The field is the branch's name; the button beside it is the dispatch.
    expect(open).toContain('data-reroute-project="p2" data-reroute-kind="branch"');
    expect(open).not.toContain('data-reroute-branch="p1"');
  });

  it("never stand in for the work item the route is on", () => {
    const entries = listed([
      captureItem({
        state: "routed",
        project_id: "p1",
        branch: "build/login",
        routing: { project_id: "p1", kind: "branch", target_id: "build/login" },
        question: { text: "which?", asked_at: ago(0), answer: null },
        unread: true,
        unread_reason: "router_question",
      }),
      branch(),
    ]);
    expect(activeEntryKey({ name: "branch", projectId: "p1", branch: "build/login" }, entries)).toBe("run-1");
  });

  it("escape what the user said and what the router asked", () => {
    const entry = entryOf(
      captureItem({
        title: '<img src=x onerror="alert(1)">',
        state: "unrouted",
        unread: true,
        unread_reason: "router_question",
        question: { text: "<script>alert(2)</script>", asked_at: ago(0), answer: null },
      }),
    );
    const html = inboxRowHtml(entry, {});
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
  });
});

describe("the project on every row", () => {
  // Two rows both named "main" from different projects must never read as the
  // same thing: the project leads line one, whatever else the row carries.
  it("falls back to the project id when the bridge sends no name", () => {
    const { entries: [entry] } = inboxEntries({ items: [branch({ project: "" })], nowMs: NOW });
    expect(entry.project).toBe("p1");
  });

  it("says so out loud when there is no project at all", () => {
    const { entries: [entry] } = inboxEntries({ items: [branch({ project: "", project_id: "" })], nowMs: NOW });
    expect(inboxRowHtml(entry, {})).toContain("unknown project");
  });

  it("leads line one with the project, before the name", () => {
    const { entries: [entry] } = inboxEntries({ items: [branch()], nowMs: NOW });
    const html = inboxRowHtml(entry, {});
    const line = html.slice(html.indexOf('class="inbox-line inbox-name"'));
    expect(line.indexOf('inbox-tag')).toBeGreaterThan(-1);
    expect(line.indexOf('inbox-tag')).toBeLessThan(line.indexOf('stitle'));
    expect(html).toContain("relaydb");
  });
});
