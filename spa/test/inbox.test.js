// The inbox's pure model: one list across every project, in the anchor's order,
// two lines per row, the Recent partition at the end, and what Done promises
// before it destroys anything.

import { describe, it, expect } from "vitest";
import {
  activeEntryKey,
  branchDoneConfirm,
  dismissParamsOf,
  entryFactsText,
  entryKeyOf,
  entryRoute,
  entryState,
  inboxEmptyHtml,
  inboxEntries,
  inboxRowHtml,
  issueDoneConfirm,
  mergePendingRows,
  recentIsOpen,
  recentToggleHtml,
  cacheableEntityIds,
  unreadReasonText,
} from "../src/core/inbox.js";

const NOW = Date.parse("2026-08-13T12:00:00Z");
const ago = (hours) => new Date(NOW - hours * 3600 * 1000).toISOString();

const branch = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1/p1",
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
    const item = branch({ unread: true, working: true });
    expect(entryState(item)).toBe("unread");
    const [entry] = listed([item]);
    expect(entry.state).toBe("unread");
    expect(entry.working).toBe(true);
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
    expect(entries.map((entry) => entry.key)).toEqual(["iss-1", "branch:dev-1/p1:main"]);
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

  it("carries a project's primary checkout — it is the project's own row", () => {
    const entries = listed([
      branch({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false, anchor: ago(1) }),
    ]);
    expect(entries.length).toBe(1);
    expect(entries[0].branch).toBe("main");
    // The repository takes no attention, so the row names no entity — and it
    // still has a key to be opened by, and its own menu to be cleared from.
    expect(entries[0].entityId).toBeNull();
    expect(entries[0].key).toBe("branch:dev-1/p1:main");
    expect(entries[0].route).toEqual({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "main", tab: "changes" });
    const html = inboxRowHtml(entries[0], {});
    expect(html).toContain('data-key="branch:dev-1/p1:main"');
    expect(html).not.toContain("data-entity");
    expect(html).toContain("data-menu");
  });

  it("routes an issue to its own surface", () => {
    expect(entryRoute(issue())).toEqual({ name: "issue", deviceId: "dev-1", projectId: "p2", id: "iss-1" });
  });

  // Every machine mints a `proj-1`, so a route that names a project without
  // naming the machine names two projects. The row knows which machine
  // answered for it, and hands that to the route it opens.
  it("entryRoute carries deviceId on branch and issue routes", () => {
    expect(entryRoute({ kind: "branch", deviceId: "d", project_id: "p", branch: "b" })).toEqual({
      name: "branch",
      deviceId: "d",
      projectId: "p",
      branch: "b",
      tab: "changes",
    });
    expect(entryRoute({ kind: "issue", deviceId: "d", project_id: "p", issue_id: "iss-2" })).toEqual({
      name: "issue",
      deviceId: "d",
      projectId: "p",
      id: "iss-2",
    });
  });

  // A capture is not per-device business: its decision page is named by the
  // capture and nothing else.
  it("a capture's own decision route carries no device", () => {
    expect(entryRoute({ kind: "capture", deviceId: "d", capture_id: "cap-1", state: "unrouted" })).toEqual({
      name: "capture",
      id: "cap-1",
    });
  });

  it("has nowhere to send a detached checkout — it has no branch to name", () => {
    expect(entryRoute(branch({ branch: null, run_id: null }))).toBeNull();
  });
});

// ---- lifecycle verbs in flight ---------------------------------------------------
// The daemon puts a row on the board the moment a verb reaches for git and takes
// it off when the record lands (`board.list`'s `pending`). A checkout being cut
// is on the inbox from the moment it is asked for, under the id it will settle
// as, so the list never goes quiet while the work is being made.

describe("the rows a lifecycle verb in flight leaves", () => {
  const creating = (over = {}) => ({
    entity_id: "wt-new",
    // Stamped on every row by the feed (core/feedMerge.js), pending ones too.
    deviceId: "dev-1",
    project_id: "p1",
    projectKey: "dev-1/p1",
    project: "relaydb",
    title: "mascot spike",
    branch: "build/mascot-spike",
    state: "creating",
    checkout_id: null,
    implements: null,
    pending_seconds: 1,
    ...over,
  });

  it("lists a checkout being cut, under the id it will settle as", () => {
    const entries = listed(mergePendingRows([branch()], [creating()]));
    const pending = entries.find((entry) => entry.key === "wt-new");
    expect(pending.kind).toBe("branch");
    expect(pending.project).toBe("relaydb");
    expect(pending.name).toBe("build/mascot-spike");
    expect(pending.state).toBe("working");
    expect(pending.facts).toBe("Creating…");
    // Nothing is there to open yet, so the row opens nowhere — it opens itself
    // the moment the record lands under the same key.
    expect(pending.route).toBeNull();
    expect(pending.canFinish).toBe(false);
  });

  // The wire's real shape: the row names the run it settles as (`entity_id`)
  // and the checkout it holds (`checkout_id`, a worktree hash). The run's card
  // carries both, under its run id.
  it("says what is happening to a card that is already there, rather than adding a second row", () => {
    const items = mergePendingRows(
      [branch()],
      [creating({ entity_id: "run-1", state: "discarding", checkout_id: "wt-1", title: "build/login" })],
    );
    expect(items).toHaveLength(1);
    const entries = listed(items);
    expect(entries).toHaveLength(1);
    expect(entries[0].entityId).toBe("run-1");
    expect(entries[0].facts).toBe("Removing…");
    expect(entries[0].state).toBe("working");
  });

  // An adopt leaves the run on the board and publishes a pending row for it, so
  // both are in the same snapshot. Two rows under one key is one row painted
  // twice, which loses the state the second write did not carry.
  it("keeps a run whose checkout is being claimed to one row", () => {
    const items = mergePendingRows(
      [branch()],
      [creating({ entity_id: "run-1", checkout_id: "wt-1", implements: "iss-1", title: "build/login" })],
    );
    expect(items).toHaveLength(1);
    const entries = listed(items);
    expect(entries).toHaveLength(1);
    expect(entries[0].key).toBe("run-1");
    expect(entries[0].facts).toBe("Creating…");
    expect(entries[0].state).toBe("working");
  });

  // A planning workspace holds no checkout — it is written against the primary
  // one — so its row names the issue and nothing else. The issue card is
  // already listed, and that is where it is said.
  it("says on an issue's own card that its planning workspace is being cut", () => {
    const items = mergePendingRows(
      [issue()],
      [creating({ entity_id: "iss-1", checkout_id: null, title: "Ship the mascot" })],
    );
    expect(items).toHaveLength(1);
    const entries = listed(items);
    expect(entries).toHaveLength(1);
    expect(entries[0].key).toBe("iss-1");
    expect(entries[0].facts).toBe("Creating…");
    expect(entries[0].state).toBe("working");
  });

  // Three verbs put `creating` on a card that is already there and already
  // openable: a plan workspace on its issue, an implementation on the run that
  // owns the checkout, and a restore on its run. The card is the reader's, and
  // it keeps opening for the whole of that git.
  it("keeps a standing card openable while a verb runs on it", () => {
    const [entry] = listed(
      mergePendingRows([issue()], [creating({ entity_id: "iss-1", checkout_id: null, title: "Ship the mascot" })]),
    );
    expect(entry.facts).toBe("Creating…");
    expect(entry.placeholder).toBe(false);
    expect(entry.route).toEqual({ name: "issue", deviceId: "dev-1", projectId: "p2", id: "iss-1" });
  });

  // The bridge says how the checkout a verb is cutting is isolated from the
  // moment it is asked for, so the row stands for the card in that too: a
  // reader of a row's isolation gets the same answer before and after the git.
  it("carries the isolation the checkout is being made as", () => {
    const [entry] = mergePendingRows([], [creating({ isolation: "rift" })]);
    expect(entry.isolation).toBe("rift");
  });

  // A verb that cuts nothing — a discard, an adoption of a checkout already on
  // disk — names no isolation, and a bridge that predates the field names none
  // either.
  it("names no isolation for a verb that makes no checkout", () => {
    const [entry] = mergePendingRows([], [creating({ isolation: null })]);
    expect(entry.isolation).toBeNull();
  });

  it("sends a row with nothing behind it nowhere, whatever the verb is called", () => {
    const [entry] = listed(mergePendingRows([], [creating({ state: "resurrecting" })]));
    expect(entry.placeholder).toBe(true);
    expect(entry.route).toBeNull();
  });

  // The project's own checkout is the one listed card with no id of its own:
  // adopting it publishes a row naming the primary of the project, and the card
  // it is running on is right there on the board.
  it("says on a project's primary card that its checkout is being adopted", () => {
    const primary = branch({
      branch: "main",
      run_id: null,
      worktree_id: null,
      primary: true,
      can_finish: false,
      anchor: ago(1),
    });
    const items = mergePendingRows(
      [primary],
      [creating({ entity_id: "run-new", checkout_id: "wt-repo-root", primary: true, title: "relaydb" })],
    );
    expect(items).toHaveLength(1);
    const entries = listed(items);
    expect(entries).toHaveLength(1);
    expect(entries[0].key).toBe("branch:dev-1/p1:main");
    expect(entries[0].facts).toBe("Creating…");
    expect(entries[0].placeholder).toBe(false);
  });

  // Two machines both mint a `p1`, so what names the project on a pending row
  // is the account-wide name. Matching on the bare id would say the laptop's
  // adopt on the desktop's primary card — whichever of them the merge listed
  // first.
  it("says it on the primary card of the device the verb is running on", () => {
    const primaryOn = (deviceId) =>
      branch({
        deviceId,
        projectKey: `${deviceId}/p1`,
        branch: "main",
        run_id: null,
        worktree_id: null,
        primary: true,
        can_finish: false,
        anchor: ago(1),
      });
    const items = mergePendingRows(
      [primaryOn("dev-2"), primaryOn("dev-1")],
      [
        creating({
          entity_id: "run-new",
          checkout_id: "wt-repo-root",
          primary: true,
          title: "relaydb",
        }),
      ],
    );
    expect(items).toHaveLength(2);
    expect(items.filter((item) => item.pending).map((item) => item.projectKey)).toEqual(["dev-1/p1"]);
  });

  it("reads a state it has never heard of as work in flight, not as nothing", () => {
    const entries = listed(mergePendingRows([], [creating({ state: "resurrecting" })]));
    expect(entries).toHaveLength(1);
    expect(entries[0].state).toBe("working");
    expect(entries[0].facts).toBe("");
  });

  // The verb in flight is said in one place, `pending`. A row's `state` is the
  // run vocabulary the list filters on, and a verb name that happened to match
  // a finished state would have dropped the row out of the list.
  it("keeps a row listed and unfinished whatever its verb is called", () => {
    const entries = listed(mergePendingRows([], [creating({ state: "merged" })]));
    expect(entries).toHaveLength(1);
    expect(entries[0].state).toBe("working");
    expect(entries[0].merged).toBe(false);
    expect(entries[0].pending).toBe("merged");
  });

  // Every verb in that menu would race the one the daemon is already running,
  // which it refuses anyway. The row says what is happening and offers nothing.
  it("offers no verbs on a row a verb is already running on", () => {
    const [pending] = listed(mergePendingRows([], [creating()]));
    const html = inboxRowHtml(pending, {});
    expect(html).not.toContain("data-menu");
    expect(html).not.toContain("data-done=");
    expect(html).not.toContain("data-dismiss=");
    expect(html).toContain("Creating…");
  });

  it("takes no pending rows at all in its stride", () => {
    expect(mergePendingRows([branch()], [])).toEqual([branch()]);
    expect(mergePendingRows(undefined, undefined)).toEqual([]);
  });
});

// ---- Recent --------------------------------------------------------------------
// Everything that has said nothing for a day is still there; it is just not what
// today is about, so it sits behind one disclosure at the end of the list.

describe("the Recent section", () => {
  const quiet = () => branch({ branch: "build/old", run_id: "run-old", anchor: ago(200), last_activity: ago(30) });

  it("takes everything whose last message activity is at least a day old", () => {
    const { entries, recent } = inboxEntries({ items: [branch(), quiet()], nowMs: NOW });
    expect(entries.map((entry) => entry.entityId)).toEqual(["run-1"]);
    expect(recent.map((entry) => entry.entityId)).toEqual(["run-old"]);
  });

  it("uses the 24-hour boundary", () => {
    const { entries, recent } = inboxEntries({ items: [branch({ last_activity: ago(24) })], nowMs: NOW });
    expect(entries).toEqual([]);
    expect(recent.map((entry) => entry.entityId)).toEqual(["run-1"]);
  });

  it("keeps an old entry in Inbox while any agent is working", () => {
    const { entries, recent } = inboxEntries({
      items: [branch({ last_activity: ago(72), working: true })],
      nowMs: NOW,
    });
    expect(entries.map((entry) => entry.entityId)).toEqual(["run-1"]);
    expect(recent).toEqual([]);
  });

  it("keeps an old unread entry in Inbox when it is also working", () => {
    const { entries } = inboxEntries({
      items: [branch({ last_activity: ago(72), unread: true, unread_count: 1, working: true })],
      nowMs: NOW,
    });
    expect(entries[0]).toMatchObject({ state: "unread", working: true });
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

  it("is one disclosure, counting what is behind it", () => {
    const { recent } = inboxEntries({ items: [branch(), quiet()], nowMs: NOW });
    const shut = recentToggleHtml(recent, false);
    expect(shut).toContain("data-recent-toggle");
    expect(shut).toContain("Recent");
    expect(shut).toContain('aria-expanded="false"');
    expect(shut).toContain('class="inbox-recent-count">1<');
    expect(recentToggleHtml(recent, true)).toContain('aria-expanded="true"');
  });

  // Recent always starts shut, however thin the list above it: only the user
  // opens it.
  it("starts shut and opens only when the user says so", () => {
    expect(recentIsOpen(null)).toBe(false);
    expect(recentIsOpen(undefined)).toBe(false);
    expect(recentIsOpen(false)).toBe(false);
    expect(recentIsOpen(true)).toBe(true);
  });

  it("says nothing at all when nothing has gone quiet", () => {
    expect(inboxEntries({ items: [branch()], nowMs: NOW }).recent).toEqual([]);
  });

  it("says so when there is nothing waiting at all", () => {
    expect(inboxEmptyHtml()).toContain("Nothing needs you");
  });
});

// ---- cleared from the inbox ----------------------------------------------------
// A row the user cleared moves to Recent until any user or agent message causes
// the bridge to remove the marker. Clear outranks working in the meantime.

describe("a row the user cleared", () => {
  it("moves from the list to Recent", () => {
    const { entries, recent } = inboxEntries({ items: [branch({ dismissed: true }), issue()], nowMs: NOW });
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1"]);
    expect(recent.map((entry) => entry.entityId)).toEqual(["run-1"]);
  });

  it("remains in Recent when it has also gone quiet", () => {
    const cleared = branch({
      branch: "build/old",
      run_id: "run-old",
      anchor: ago(200),
      last_activity: ago(30),
      dismissed: true,
    });
    const { entries, recent } = inboxEntries({ items: [issue(), cleared], nowMs: NOW });
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1"]);
    expect(recent.map((entry) => entry.entityId)).toEqual(["run-old"]);
  });

  it("stays in Recent even while an agent is working", () => {
    const { entries, recent } = inboxEntries({
      items: [branch({ dismissed: true, working: true })],
      nowMs: NOW,
    });
    expect(entries).toEqual([]);
    expect(recent[0]).toMatchObject({ entityId: "run-1", working: true });
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
    expect(html).toContain("Moves it to Recent until a new message");
    expect(html.indexOf("Clear from inbox")).toBeLessThan(html.indexOf(">Mute<"));
  });

  // Every row can be cleared, including the ones no entity stands behind — the
  // bridge clears those at the commit they sit on, and a new commit brings them
  // back. Nothing destructive stands beside Clear on such a row: mute needs a
  // voice to take, Done needs something to finish, and the row has neither.
  it("is offered on the primary row, with nothing destructive beside it", () => {
    const [entry] = listed([
      branch({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false }),
    ]);
    const html = inboxRowHtml(entry, { openMenuKey: "branch:dev-1/p1:main" });
    expect(html).toContain('data-dismiss="branch:dev-1/p1:main"');
    expect(html).toContain("Clear from inbox");
    expect(html).not.toContain('data-mute="');
    expect(html).not.toContain('data-done="');
  });

  // Done destroys an entity: a branch's records, an issue's plans. A row that
  // names none has nothing to destroy and no id to say it with, so the feed
  // calling it finishable is not enough to put a Done on it.
  it("puts no Done on a row that names no entity, whatever the feed says", () => {
    const [entry] = listed([
      branch({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: true }),
    ]);
    expect(entry.canFinish).toBe(false);
    const html = inboxRowHtml(entry, { openMenuKey: "branch:dev-1/p1:main" });
    expect(html).not.toContain("data-done=");
  });

  it("is offered on a bare checkout row, which the bridge clears by its worktree id", () => {
    const [entry] = listed([branch({ run_id: null, issue_id: null, worktree_id: "wt-9" })]);
    const html = inboxRowHtml(entry, { openMenuKey: "wt-9" });
    expect(html).toContain('data-dismiss="wt-9"');
    expect(html).toContain('data-mute="wt-9"');
  });

  // What the clear says on the wire: an entity by its id; a row with none by
  // what it IS — the project's checkout (primary), or a branch in the project.
  it("names the row being cleared the way the bridge expects", () => {
    const [run] = listed([branch()]);
    expect(dismissParamsOf(run)).toEqual({ entity_id: "run-1" });
    const [primary] = listed([
      branch({ branch: "main", run_id: null, worktree_id: null, primary: true, can_finish: false }),
    ]);
    expect(dismissParamsOf(primary)).toEqual({ project_id: "p1", primary: true });
    const [bare] = listed([branch({ run_id: null, issue_id: null, worktree_id: null })]);
    expect(dismissParamsOf(bare)).toEqual({ project_id: "p1", branch: "build/login" });
  });

  it("has no name for a row that is neither an entity nor a project's branch", () => {
    expect(dismissParamsOf({ entityId: null, projectId: "", branch: null, primary: false })).toBeNull();
  });
});

describe("the active entry", () => {
  const { entries, recent } = inboxEntries({ items: [branch(), issue()], nowMs: NOW });
  const all = [...entries, ...recent];

  it("is the one the route is standing on", () => {
    expect(activeEntryKey({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" }, all)).toBe("run-1");
    expect(activeEntryKey({ name: "issue", deviceId: "dev-1", projectId: "p2", id: "iss-1" }, all)).toBe("iss-1");
  });

  // Two machines both hold a `proj-1` with a `build/login` in it, and those are
  // two rows. The route says which machine it is standing on, so the mark goes
  // on that machine's row and on no other.
  it("activeEntryKey marks the row on the route's device, not another device's copy of the same branch", () => {
    const twice = inboxEntries({
      items: [branch(), branch({ deviceId: "dev-2", projectKey: "dev-2/p1", run_id: "run-2", worktree_id: "wt-2" })],
      nowMs: NOW,
    });
    const both = [...twice.entries, ...twice.recent];
    expect(activeEntryKey({ name: "branch", deviceId: "dev-2", projectId: "p1", branch: "build/login" }, both)).toBe("run-2");
    expect(activeEntryKey({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" }, both)).toBe("run-1");
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

  it("weighs the uncommitted tree on line two, not the branch's whole history", () => {
    const [entry] = listed([
      branch({
        stat: { ...branch().stat, ahead: 2, behind: 1, uncommitted: { files_changed: 3, insertions: 42, deletions: 7 } },
      }),
    ]);
    expect(entry.facts).toBe("3 files · ↑2 ↓1 · +42 −7");
    expect(inboxRowHtml(entry, {})).toContain("3 files · ↑2 ↓1 · +42 −7");
  });

  it("says nothing about work the branch has already committed", () => {
    const [entry] = listed([branch({ stat: { ...branch().stat, ahead: 2 } })]);
    expect(entry.facts).toBe("↑2");
  });

  it("counts one file as one file", () => {
    expect(entryFactsText({ stat: { uncommitted: { files_changed: 1, insertions: 1, deletions: 0 } } })).toBe("1 file · +1 −0");
  });

  it("says nothing about a branch that is level with what it is compared to", () => {
    expect(
      entryFactsText({ stat: { uncommitted: { files_changed: 2, insertions: 3, deletions: 4 }, ahead: 0, behind: 0 } }),
    ).toBe("2 files · +3 −4");
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
    expect(inboxRowHtml(nothing, { openMenuKey: "run-1" })).not.toContain("data-done=");
    const [finishable] = listed([branch()]);
    expect(inboxRowHtml(finishable, { openMenuKey: "run-1" })).toContain('data-done="run-1"');
  });

  it("offers mute in the entry's own menu, and says so when it is already muted", () => {
    const [entry] = listed([branch({ muted: true })]);
    const html = inboxRowHtml(entry, { openMenuKey: "run-1" });
    expect(html).toContain('data-mute="run-1"');
    expect(html).toContain("Unmute");
    expect(html).toContain("inbox-muted");
    // The open menu is the one whose row was asked for; a shut one is not in
    // the markup at all — the DOM patcher would never re-hide it.
    expect(html).toMatch(/class="splitmenu inbox-menu">/);
    expect(inboxRowHtml(entry, {})).not.toContain("splitmenu");
    expect(inboxRowHtml(entry, {})).not.toContain("data-mute=");
  });

  // The reviewer's second screenshot: a Done button with a caret laid over the
  // row covered the row's own words. Done destroys, so one step behind the
  // row's one quiet ⋯ — first in the menu, with its confirmation behind it —
  // is the right distance for it.
  it("keeps Done one step behind the ⋯, first in the menu, and never on the row", () => {
    const [entry] = listed([branch()]);
    const html = inboxRowHtml(entry, { openMenuKey: "run-1" });
    expect(html).toContain("inbox-more");
    expect(html).toContain("⋯");
    expect(html).not.toContain('class="splitbtn"');
    expect(html).not.toMatch(/<button[^>]*data-done=/);
    const order = [...html.matchAll(/data-(done|dismiss|mute)="run-1"/g)].map((m) => m[1]);
    expect(order).toEqual(["done", "dismiss", "mute"]);
  });

  it("wears the same ⋯ on a row with no Done", () => {
    const [entry] = listed([branch({ can_finish: false })]);
    const html = inboxRowHtml(entry, { openMenuKey: "run-1" });
    expect(html).toContain("inbox-more");
    expect(html).not.toContain("data-done=");
    expect(html).toContain('data-dismiss="run-1"');
  });

  // Recent's rows are quiet: nothing about them needs a glance, so the state
  // dot goes and the row is one line, with what it weighs at the right edge.
  it("paints a quiet row as one line with no state dot and its weight floating at the right", () => {
    const [entry] = listed([branch({ stat: { uncommitted: { files_changed: 3, insertions: 4, deletions: 1 }, ahead: 2 } })]);
    const html = inboxRowHtml(entry, { quiet: true });
    expect(html).toContain("inbox-quiet");
    expect(html).not.toContain("sdot");
    expect(html).not.toContain('class="inbox-facts"');
    expect(html).toMatch(/inbox-facts-float">3 files · ↑2 · \+4 −1</);
    expect(html).toContain('data-key="run-1"');
    expect(html).toContain('data-menu="run-1"');
    expect(html).toContain('class="inbox-tag">relaydb');
    expect(inboxRowHtml(entry, { quiet: true, showProject: false })).not.toContain("inbox-tag");
    // A quiet row with nothing to weigh floats nothing.
    const [bare] = listed([branch({ stat: null })]);
    expect(inboxRowHtml(bare, { quiet: true })).not.toContain("inbox-facts-float");
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
  deviceId: "dev-1",
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
    ).toEqual({ name: "issue", deviceId: "dev-1", projectId: "p1", id: "iss-9" });
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
    expect(html).not.toContain('data-reroute-kind="issue"');
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
    expect(activeEntryKey({ name: "branch", deviceId: "dev-1", projectId: "p1", branch: "build/login" }, entries)).toBe("run-1");
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

// ---- what names a row across the account ---------------------------------------
// Two devices both call their first project `proj-1`, so a row that has no
// entity of its own — a project's primary checkout, a branch with no checkout —
// is named by the project key, never the bare project id.
describe("what names a row", () => {
  it("keys a no-entity row by its projectKey, in the same shape as before", () => {
    const primary = branch({ run_id: null, worktree_id: null, branch: "main", primary: true });
    expect(entryKeyOf(primary)).toBe("branch:dev-1/p1:main");
    expect(entryKeyOf({ ...primary, deviceId: "dev-2", projectKey: "dev-2/p1" })).toBe("branch:dev-2/p1:main");
    const plan = issue({ issue_id: null, run_id: null, worktree_id: null });
    expect(entryKeyOf(plan)).toBe("issue:dev-1/p2");
  });

  it("keys a row that has an entity by that entity, whichever device it is on", () => {
    expect(entryKeyOf(branch())).toBe("run-1");
    expect(entryKeyOf({ ...branch(), deviceId: "dev-2", projectKey: "dev-2/p1" })).toBe("run-1");
  });

  it("keys a capture by the capture, as it always has", () => {
    expect(entryKeyOf(captureItem())).toBe("capture:capture-1");
    expect(entryKeyOf(captureItem({ deviceId: "dev-2" }))).toBe("capture:capture-1");
  });

  it("carries deviceId and projectKey onto every entry", () => {
    const entries = listed([branch(), issue(), captureItem()]);
    expect(entries.map((entry) => entry.deviceId)).toEqual(["dev-1", "dev-1", "dev-1"]);
    expect(entries.map((entry) => entry.projectKey)).toEqual(["dev-1/p1", "dev-1/p2", undefined]);
    // The wire's own field is untouched: the bridge still wants the bare id.
    expect(entries[0].projectId).toBe("p1");
  });

  it("still names a cleared row on the wire by its bare project id", () => {
    const [entry] = listed([branch({ run_id: null, worktree_id: null, branch: "main", primary: true })]);
    expect(dismissParamsOf(entry)).toEqual({ project_id: "p1", primary: true });
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

// ---- what the cache keeps warm -------------------------------------------------
// The local cache follows the inbox's own partition: what is listed is active,
// and going Recent, being cleared, or finishing all mean immediate eviction.
describe("cacheableEntityIds", () => {
  it("names every listed entity and drops what is quiet, cleared, or finished", () => {
    const items = [
      branch(),
      branch({ branch: "b2", run_id: "run-2", worktree_id: "wt-2", anchor: ago(40), last_activity: ago(30) }),
      branch({ branch: "b3", run_id: "run-3", worktree_id: "wt-3", dismissed: true }),
      branch({ branch: "b4", run_id: "run-4", worktree_id: "wt-4", state: "merged" }),
      issue(),
    ];
    expect(cacheableEntityIds({ items, nowMs: NOW }).sort()).toEqual(["iss-1", "run-1"]);
  });

  it("keeps the issue an active branch is implementing, though it is not listed", () => {
    const items = [issue({ implementation_active: true })];
    expect(inboxEntries({ items, nowMs: NOW }).entries).toEqual([]);
    expect(cacheableEntityIds({ items, nowMs: NOW })).toEqual(["iss-1"]);
  });

  it("does not keep a finished or cleared issue even while marked implementing", () => {
    expect(cacheableEntityIds({ items: [issue({ implementation_active: true, state: "archived" })], nowMs: NOW })).toEqual([]);
    expect(cacheableEntityIds({ items: [issue({ implementation_active: true, dismissed: true })], nowMs: NOW })).toEqual([]);
  });

  it("names no entity for rows that hold none", () => {
    const items = [branch({ branch: "main", run_id: null, worktree_id: null, primary: true })];
    expect(cacheableEntityIds({ items, nowMs: NOW })).toEqual([]);
  });
});
