// The inbox's pure model: one list across every project, three states, and the
// copy that says why an entry needs you.

import { describe, it, expect } from "vitest";
import {
  INBOX_MINIMUM,
  activeEntryKey,
  branchDoneConfirm,
  entryRoute,
  entryState,
  inboxEntries,
  inboxListHtml,
  inboxRowHtml,
  issueDoneConfirm,
  unlinkDisclosure,
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
  stat: { files_changed: 3, insertions: 42, deletions: 7, uncommitted: { files_changed: 0, insertions: 0, deletions: 0 }, ahead: 0, behind: 0, upstream: "origin/build/login" },
  resume_at: ago(1),
  can_finish: false,
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
  resume_at: ago(2),
  can_finish: false,
  muted: false,
  worktree_path: null,
  worktree_id: null,
  run_id: null,
  issue_id: "iss-1",
  primary: false,
  ...over,
});

describe("an entry's state", () => {
  it("is unread when an attention event is waiting, whatever else is true", () => {
    expect(entryState(branch({ unread: true, working: true }))).toBe("unread");
  });

  it("is working while the agent has the message and has not reported", () => {
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

describe("the entry list", () => {
  it("is one list across projects — no per-project blocks, ordered unread, working, then quiet", () => {
    const entries = inboxEntries({
      items: [
        branch({ branch: "build/quiet", run_id: "run-quiet", title: "quiet", resume_at: ago(3) }),
        issue({ unread: true, unread_reason: "done", unread_count: 2 }),
        branch({ branch: "build/busy", run_id: "run-busy", title: "busy", working: true, resume_at: ago(4) }),
      ],
      nowMs: NOW,
    });
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1", "run-busy", "run-quiet"]);
    expect(entries.map((entry) => entry.project)).toEqual(["dotfiles", "relaydb", "relaydb"]);
  });

  it("keeps everything alive, waiting, finishable or recent, and drops the rest once the list is full", () => {
    const stale = Array.from({ length: INBOX_MINIMUM + 4 }, (_, index) =>
      branch({ branch: `build/old-${index}`, run_id: `run-old-${index}`, resume_at: ago(48 + index) }),
    );
    const entries = inboxEntries({
      items: [...stale, branch({ branch: "build/done", run_id: "run-done", can_finish: true, resume_at: ago(72) })],
      nowMs: NOW,
    });
    expect(entries.some((entry) => entry.entityId === "run-done")).toBe(true);
    expect(entries.length).toBe(INBOX_MINIMUM);
    // The backfill takes the most recently picked up of what is left.
    expect(entries.some((entry) => entry.entityId === "run-old-0")).toBe(true);
    expect(entries.some((entry) => entry.entityId === `run-old-${INBOX_MINIMUM + 3}`)).toBe(false);
  });

  it("always carries a project's primary checkout — it is the project's own row", () => {
    const entries = inboxEntries({
      items: [branch({ branch: "main", run_id: null, worktree_id: null, primary: true, resume_at: null })],
      nowMs: NOW,
    });
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

  it("leaves out what the user just said Done to", () => {
    const entries = inboxEntries({ items: [branch(), issue()], nowMs: NOW, dismissed: new Set(["run-1"]) });
    expect(entries.map((entry) => entry.entityId)).toEqual(["iss-1"]);
  });

  it("carries the reason, the metadata line and the route on the entry", () => {
    const [entry] = inboxEntries({
      items: [branch({ unread: true, unread_reason: "blocked", working_time: { since: ago(0.25), seconds: 900 } })],
      nowMs: NOW,
    });
    expect(entry.reason).toBe("Blocked — the agent needs you");
    expect(entry.facts).toContain("3 files");
    expect(entry.facts).toContain("+42 −7");
    expect(entry.facts).toContain("working 15m");
    expect(entry.route).toEqual({ name: "branch", projectId: "p1", branch: "build/login", tab: "changes" });
  });

  it("routes an issue to its own surface", () => {
    expect(entryRoute(issue())).toEqual({ name: "issue", projectId: "p2", id: "iss-1" });
  });

  it("has nowhere to send a detached checkout — it has no branch to name", () => {
    expect(entryRoute(branch({ branch: null, run_id: null }))).toBeNull();
  });
});

describe("the active entry", () => {
  const entries = inboxEntries({ items: [branch(), issue()], nowMs: NOW });

  it("is the one the route is standing on", () => {
    expect(activeEntryKey({ name: "branch", projectId: "p1", branch: "build/login" }, entries)).toBe("run-1");
    expect(activeEntryKey({ name: "issue", projectId: "p2", id: "iss-1" }, entries)).toBe("iss-1");
  });

  it("is nothing on a route that names no work item", () => {
    expect(activeEntryKey({ name: "inbox" }, entries)).toBeNull();
  });
});

describe("the rendered rows", () => {
  it("carry the state dot, the title, the project tag, the branch and the reason", () => {
    const [entry] = inboxEntries({
      items: [branch({ unread: true, unread_reason: "done", unread_count: 3 })],
      nowMs: NOW,
    });
    const html = inboxRowHtml(entry, {});
    expect(html).toContain("sdot-unread");
    expect(html).toContain("Fix the login flow");
    expect(html).toContain("relaydb");
    expect(html).toContain("build/login");
    expect(html).toContain("The agent finished — review the work");
    expect(html).toContain('data-entity="run-1"');
  });

  it("mark an issue as an issue, since it has no branch to say it", () => {
    const [entry] = inboxEntries({ items: [issue()], nowMs: NOW });
    expect(inboxRowHtml(entry, {})).toContain("Issue");
  });

  it("offer Done only when the work item can be finished", () => {
    const [quiet] = inboxEntries({ items: [branch()], nowMs: NOW });
    expect(inboxRowHtml(quiet, {})).not.toContain("data-done=");
    const [finishable] = inboxEntries({ items: [branch({ can_finish: true })], nowMs: NOW });
    expect(inboxRowHtml(finishable, {})).toContain('data-done="run-1"');
  });

  it("offer mute in the entry's own menu, and say so when it is already muted", () => {
    const [entry] = inboxEntries({ items: [branch({ muted: true })], nowMs: NOW });
    const html = inboxRowHtml(entry, { openMenuKey: "run-1" });
    expect(html).toContain('data-mute="run-1"');
    expect(html).toContain("Unmute");
    expect(html).toContain("inbox-muted");
    // The open menu is the one whose row was asked for; a shut one is hidden.
    expect(html).toMatch(/class="splitmenu inbox-menu">/);
    expect(inboxRowHtml(entry, {})).toMatch(/class="splitmenu inbox-menu" hidden>/);
  });

  it("escape everything the repo named", () => {
    const [entry] = inboxEntries({ items: [branch({ title: '<img src=x onerror="alert(1)">' })], nowMs: NOW });
    const html = inboxRowHtml(entry, {});
    expect(html).not.toContain("<img");
    expect(html).toContain("&lt;img");
  });

  it("say so when there is nothing waiting at all", () => {
    expect(inboxListHtml([], {})).toContain("Nothing needs you");
  });
});

describe("the Done confirmations", () => {
  it("outline what finishing a branch does, and name the issue it also archives", () => {
    const [entry] = inboxEntries({ items: [branch({ can_finish: true, issue_id: "iss-7" })], nowMs: NOW });
    const confirm = branchDoneConfirm(entry);
    expect(confirm.actions.join(" ")).toContain("build/login");
    expect(confirm.actions.some((action) => action.toLowerCase().includes("issue"))).toBe(true);
  });

  it("say nothing about an issue when the branch implements none", () => {
    const [entry] = inboxEntries({ items: [branch({ can_finish: true })], nowMs: NOW });
    expect(branchDoneConfirm(entry).actions.some((action) => action.toLowerCase().includes("issue"))).toBe(false);
  });

  it("archive an issue with its stage plans", () => {
    const [entry] = inboxEntries({ items: [issue({ can_finish: true })], nowMs: NOW });
    expect(issueDoneConfirm(entry).actions.join(" ")).toContain("stage plans");
  });

  it("disclose the unlink override in the bridge's own words", () => {
    const [entry] = inboxEntries({ items: [branch({ can_finish: true, issue_id: "iss-7" })], nowMs: NOW });
    const disclosure = unlinkDisclosure(entry, "branch.finish: … pass unlink to finish the branch alone");
    expect(disclosure.intro).toContain("pass unlink");
    expect(disclosure.confirmLabel.toLowerCase()).toContain("branch");
  });
});

// ---- captures ----------------------------------------------------------------
// A capture is unfinished business until the router reaches a destination, so
// it is a row of the inbox like anything else — with its own states, and the
// two things a user can do about a route: answer, and send it somewhere else.

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
  resume_at: ago(0),
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

describe("capture rows", () => {
  const entryOf = (item) => inboxEntries({ items: [item], nowMs: NOW })[0];

  it("name themselves by the capture, never by the destination it was routed to", () => {
    const entry = entryOf(captureItem({ state: "routed", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" }, issue_id: "iss-9" }));
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
      entryOf(captureItem({ state: "routed", project_id: "p1", issue_id: "iss-9", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }))
        .route,
    ).toEqual({ name: "issue", projectId: "p1", id: "iss-9" });
  });

  it("mark the row the decision page is standing on", () => {
    const entries = inboxEntries({ items: [captureItem({ state: "unrouted" }), branch()], nowMs: NOW });
    expect(activeEntryKey({ name: "capture", id: "capture-1" }, entries)).toBe("capture:capture-1");
    expect(activeEntryKey({ name: "capture", id: "capture-9" }, entries)).toBeNull();
  });

  it("open the destination picker on the row that asked for it", () => {
    const entry = entryOf(captureItem({ state: "routed", project: "relaydb", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));
    const projects = [{ id: "p1", name: "relaydb" }, { id: "p2", name: "dotfiles" }];
    const html = inboxRowHtml(entry, { rerouteKey: entry.key, projects });
    expect(html).toContain('data-reroute-project="p2"');
    expect(html).toContain('data-reroute-kind="issue"');
    expect(html).toContain('data-reroute-branch-open="p2"');
    expect(inboxRowHtml(entry, { projects })).not.toContain("data-reroute-project");
  });

  it("take the branch's name in the picker, from the branches the project has", () => {
    const entry = entryOf(captureItem({ state: "routed", project: "relaydb", routing: { project_id: "p1", kind: "issue", target_id: "iss-9" } }));
    const projects = [{ id: "p1", name: "relaydb" }, { id: "p2", name: "dotfiles" }];
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
    const entries = inboxEntries({
      items: [
        captureItem({ state: "routed", project_id: "p1", branch: "build/login", routing: { project_id: "p1", kind: "branch", target_id: "build/login" }, question: { text: "which?", asked_at: ago(0), answer: null }, unread: true, unread_reason: "router_question" }),
        branch(),
      ],
      nowMs: NOW,
    });
    expect(activeEntryKey({ name: "branch", projectId: "p1", branch: "build/login" }, entries)).toBe("run-1");
  });

  it("escape what the user said and what the router asked", () => {
    const entry = entryOf(
      captureItem({
        title: '<img src=x onerror="alert(1)">',
        state: "unrouted",
        unread: true,
        unread_reason: "router_question",
        question: { text: '<script>alert(2)</script>', asked_at: ago(0), answer: null },
      }),
    );
    const html = inboxRowHtml(entry, {});
    expect(html).not.toContain("<img");
    expect(html).not.toContain("<script>");
  });
});
