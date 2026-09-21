// Watched issues in the inbox (#65).
//
// The rows arrive on the same push as the conversation rows and are the same
// kind of thing to the reader: something that moved, how long ago, and how much
// of it they have not seen. So they interleave by activity rather than sitting
// in a section of their own — with one exception the brief asks for, below.
//
// The fixture is the shape proposed on #64. Nothing downstream of it is shaped
// by these names: if the bridge lands something different, this file changes
// and `trackerIssueItem` is the only thing that has to.

import { describe, it, expect, vi } from "vitest";

// These rows only exist on a bridge that carries watching (#65), so the fixture
// says what the machine they are on can do.
//
// Mocking the capability read rather than a version: the gate moved onto
// `capabilities.issues.watching`, and a fixture that kept answering with a
// version would have gone on passing while the gate read `undefined` — which
// looks exactly like the gate working. railWatchSwitch.test.js is where a real
// greeting proves the wiring.
const capabilities = vi.fn(() => ({ issues: { watching: true } }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: (...args) => capabilities(...args) }));

const { inboxEntries, entryRoute, entryKeyOf } = await import("../src/core/inbox.js");

/** One watched issue, as #64 will push it. */
const issueRow = (over = {}) => ({
  kind: "tracker_issue",
  issue_id: "issue-53",
  number: 53,
  project_id: "proj-1",
  projectKey: "dev-1/proj-1",
  deviceId: "dev-1",
  project: "Build",
  title: "Configure agent choices",
  status: "in_review",
  assignee: null,
  assigned_to_user: false,
  last_event: { text: "New comment from issues-board · Rail scroll", actor: "issues-board", at: "2026-09-21T00:06:00Z" },
  anchor: "2026-09-21T00:06:00Z",
  last_activity: "2026-09-21T00:06:00Z",
  unread: 3,
  muted: false,
  done_until_next: false,
  ...over,
});

/** A conversation row, as the inbox already receives them. */
const branchRow = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  project_id: "proj-1",
  projectKey: "dev-1/proj-1",
  branch: "build/login",
  run_id: "run-1",
  state: "building",
  anchor: "2026-09-21T00:05:00Z",
  last_activity: "2026-09-21T00:05:00Z",
  ...over,
});

const NOW = Date.parse("2026-09-21T00:10:00Z");
const listed = (items) => inboxEntries({ items, nowMs: NOW }).entries;

describe("a watched issue as a row", () => {
  it("is titled by its number and its words", () => {
    const [row] = listed([issueRow()]);
    expect(row.name).toBe("#53 Configure agent choices");
    expect(row.kind).toBe("tracker_issue");
  });

  it("says what last happened, in the words the bridge composed", () => {
    // Composed on the bridge (#61's form). The SPA re-deriving it would be a
    // second implementation of the same sentence, free to drift from it.
    const [row] = listed([issueRow()]);
    expect(row.facts).toBe("New comment from issues-board · Rail scroll");
  });

  it("carries its unread, which the inbox total counts", () => {
    const [row] = listed([issueRow({ unread: 3 })]);
    expect(row.unreadCount).toBe(3);
    expect(row.state).toBe("unread");
  });

  it("opens the tracker's issue page, not the legacy one", () => {
    // `kind: "issue"` already means the legacy multi-stage issue and opens the
    // plan/stages page. A watched issue is the tracker's (#64).
    expect(entryRoute(issueRow())).toEqual({
      name: "trackerIssue", deviceId: "dev-1", projectId: "proj-1", issueId: "issue-53",
    });
  });

  it("is named by its issue, so two projects' rows cannot collide", () => {
    expect(entryKeyOf(issueRow())).toBe("tracker_issue:issue-53");
  });

  it("carries Mute and Done through as the inbox already means them", () => {
    const [muted] = listed([issueRow({ muted: true })]);
    expect(muted.muted).toBe(true);
    // Done clears until the next event — the same word conversation rows use.
    const cleared = inboxEntries({ items: [issueRow({ done_until_next: true })], nowMs: NOW });
    expect(cleared.entries).toHaveLength(0);
    expect(cleared.recent.map((row) => row.dismissed)).toEqual([true]);
  });
});

describe("where the rows sit", () => {
  it("interleaves with conversation rows by when each last moved", () => {
    const rows = listed([
      branchRow({ branch: "build/late", anchor: "2026-09-21T00:07:00Z", last_activity: "2026-09-21T00:07:00Z" }),
      issueRow({ anchor: "2026-09-21T00:06:00Z", last_activity: "2026-09-21T00:06:00Z" }),
      branchRow({ branch: "build/early", anchor: "2026-09-21T00:05:00Z", last_activity: "2026-09-21T00:05:00Z" }),
    ]);
    expect(rows.map((row) => row.name)).toEqual([
      "build/early",
      "#53 Configure agent choices",
      "build/late",
    ]);
  });

  // The one exception to activity order the brief asks for: an issue the user
  // was handed outranks the issues that merely moved.
  it("puts an issue assigned to the reader above the other issue rows", () => {
    const rows = listed([
      issueRow({ issue_id: "issue-1", number: 1, title: "Older", anchor: "2026-09-21T00:01:00Z", last_activity: "2026-09-21T00:01:00Z" }),
      issueRow({ issue_id: "issue-2", number: 2, title: "Yours", assigned_to_user: true,
                 anchor: "2026-09-21T00:09:00Z", last_activity: "2026-09-21T00:09:00Z" }),
      issueRow({ issue_id: "issue-3", number: 3, title: "Newer", anchor: "2026-09-21T00:08:00Z", last_activity: "2026-09-21T00:08:00Z" }),
    ]);
    expect(rows.map((row) => row.number)).toEqual([2, 1, 3]);
  });

  it("leaves conversation rows alone when an assigned issue is pinned", () => {
    const rows = listed([
      branchRow({ branch: "build/first", anchor: "2026-09-21T00:01:00Z", last_activity: "2026-09-21T00:01:00Z" }),
      issueRow({ assigned_to_user: true, anchor: "2026-09-21T00:09:00Z", last_activity: "2026-09-21T00:09:00Z" }),
    ]);
    // The pin orders the ISSUE rows among themselves; it does not lift the
    // issue over a conversation that moved more recently.
    expect(rows.map((row) => row.kind)).toEqual(["branch", "tracker_issue"]);
  });
});

describe("when the row says it moved", () => {
  // Settled on #64: the row carries `anchor` and `last_activity` as ISO
  // strings, the way every other row in this feed does.
  it("takes the row's own dates", () => {
    const [row] = listed([issueRow({ anchor: "2026-09-21T00:02:00Z", last_activity: "2026-09-21T00:03:00Z" })]);
    expect(row.anchorMs).toBe(Date.parse("2026-09-21T00:02:00Z"));
    expect(row.lastActivityMs).toBe(Date.parse("2026-09-21T00:03:00Z"));
  });

  // A row that dated neither would sort under everything rather than where it
  // belongs, so the event it carries is the fallback.
  it("falls back to the event it carries", () => {
    const row = issueRow();
    delete row.anchor;
    delete row.last_activity;
    const [entry] = listed([row]);
    expect(entry.anchorMs).toBe(Date.parse("2026-09-21T00:06:00Z"));
    expect(entry.lastActivityMs).toBe(Date.parse("2026-09-21T00:06:00Z"));
  });
});

describe("the bridge that has never heard of watching", () => {
  // Such a machine pushes no such row, and one arriving from anywhere else is
  // not something this client can act on: Mute and Done on it would call verbs
  // that bridge refuses.
  const withoutWatching = () => capabilities.mockReturnValue({ issues: { attachments: true, watching: false } });

  it("lists no issue rows at all", () => {
    withoutWatching();
    expect(listed([issueRow()])).toEqual([]);
  });

  it("leaves its conversation rows alone", () => {
    withoutWatching();
    expect(listed([branchRow(), issueRow()]).map((row) => row.kind)).toEqual(["branch"]);
  });

  // An adapter older than the flag has no `issues` group at all, which must
  // read as no rather than throw on the way past.
  it("lists none for an adapter that predates the flag entirely", () => {
    capabilities.mockReturnValue({});
    expect(listed([issueRow()])).toEqual([]);
  });

  it("lists them again on a bridge that does carry it", () => {
    capabilities.mockReturnValue({ issues: { watching: true } });
    expect(listed([issueRow()]).map((row) => row.kind)).toEqual(["tracker_issue"]);
  });
});
