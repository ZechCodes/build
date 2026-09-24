import { describe, expect, it } from "vitest";
import { workspaceKey } from "../src/core/deviceKey.js";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { activeEntryKey, inboxRowHtml, routedEntityId, workspaceEntries, workspaceIsRecent } from "../src/core/inbox.js";

const DEVICE = "dev-1";

// The inbox never sees a bare row: every workspace, project and board item it
// folds has been through one machine's snapshot, which stamps the device and
// mints the account-wide keys (core/deviceKey.js). The fixtures go through the
// same door, so the rows carry `deviceId`, `projectKey` and `workspaceKey`.
const entriesOf = (workspaces, items = []) => {
  const view = liveFeedSnapshot(
    { items },
    { projects: [{ id: "project-1", name: "Payments" }] },
    { workspaces },
    DEVICE,
  );
  return workspaceEntries(view.workspaces, view.projects, view.items);
};

describe("workspace inbox rows", () => {
  it("orders bridge workspace sessions without fetching a thread", () => {
    const hour = 60 * 60 * 1000;
    const workspace = (id, started, last = started) => ({
      id, project_id: "project-1", session_started_ms: started, last_activity_ms: last,
    });
    const earlier = workspace("earlier", 10 * hour);
    const later = workspace("later", 20 * hour);
    expect(entriesOf([later, earlier]).map((row) => row.workspaceId)).toEqual(["earlier", "later"]);

    earlier.last_activity_ms = 21 * hour;
    expect(entriesOf([later, earlier]).map((row) => row.workspaceId)).toEqual(["earlier", "later"]);

    earlier.session_started_ms = 34 * hour;
    earlier.last_activity_ms = 34 * hour;
    expect(entriesOf([later, earlier]).map((row) => row.workspaceId)).toEqual(["later", "earlier"]);
  });

  it("ages a workspace into Recent at 24 hours using the newest message", () => {
    const day = 24 * 60 * 60 * 1000;
    const [entry] = entriesOf([{ id: "quiet", project_id: "project-1",
      session_started_ms: 100, last_activity_ms: 200 }]);
    expect(workspaceIsRecent(entry, 200 + day - 1)).toBe(false);
    expect(workspaceIsRecent(entry, 200 + day)).toBe(false);
    expect(workspaceIsRecent(entry, 200 + day + 1)).toBe(true);
  });

  it("anchors an old bridge's workspace today", () => {
    const now = Date.now();
    const [entry] = entriesOf([{ id: "empty", project_id: "project-1",
      created_at: "2026-08-01T00:00:00Z", updated_at: "2026-08-03T00:00:00Z",
    }]);
    expect(entry.anchorMs).toBeGreaterThanOrEqual(now);
    expect(entry.lastActivityMs).toBe(entry.anchorMs);
  });

  it("anchors a new bridge's empty workspace at creation", () => {
    const [entry] = entriesOf([{ id: "empty", project_id: "project-1",
      created_at: "2026-08-01T00:00:00Z", session_started_ms: null, last_activity_ms: null,
    }]);
    expect(entry.anchorMs).toBe(Date.parse("2026-08-01T00:00:00Z"));
    expect(entry.lastActivityMs).toBe(entry.anchorMs);
  });

  it("sorts every device's workspaces together by the bridge anchor", () => {
    const workspaces = [
      { id: "new", project_id: "project-1", session_started_ms: 3, last_activity_ms: 3 },
      { id: "old", project_id: "project-1", session_started_ms: 1, last_activity_ms: 1 },
      { id: "middle", project_id: "project-1", session_started_ms: 2, last_activity_ms: 2 },
    ];
    expect(entriesOf(workspaces).map((row) => row.workspaceId)).toEqual(["old", "middle", "new"]);
  });

  it("uses the bridge session anchor ahead of a row's original anchor", () => {
    const rows = entriesOf([
      { id: "run-1", project_id: "project-1", session_started_ms: 3, last_activity_ms: 3 },
      { id: "run-2", project_id: "project-1", session_started_ms: 2, last_activity_ms: 2 },
    ], [
      { kind: "branch", project_id: "project-1", run_id: "run-1", anchor: "2026-08-03T00:00:00Z" },
      { kind: "branch", project_id: "project-1", run_id: "run-2", anchor: "2026-08-02T00:00:00Z" },
    ]);
    expect(rows.map((row) => row.workspaceId)).toEqual(["run-2", "run-1"]);
  });

  it.each(["", "  ", "Bridge wire interface / 🦊"])("keeps the chosen workspace display name %j", (name) => {
    const [entry] = entriesOf([{ id: "workspace-1", project_id: "project-1", name, root: "/normalized/path" }]);
    expect(entry.name).toBe(name);
  });

  // The bridge keeps the typed name byte for byte and derives the CHECKOUT's
  // folder and the branch it cuts from a bounded slug of it. Neither derivative
  // is what the workspace is called, so neither may reach the row, its tooltip
  // or anything else the reader looks at.
  it("says the name the workspace was given, never the slug its folder and branch took", () => {
    const [entry] = entriesOf([{
      id: "workspace-1",
      project_id: "project-1",
      name: "Bridge wire interface",
      root: "/home/ada/.build/workspaces/proj-1/bridge-wire-interface",
      directories: [{ id: "api", source_id: "api", is_git: true, branch: "build/bridge-wire-interface" }],
    }]);
    expect(entry.name).toBe("Bridge wire interface");
    expect(entry.title).toBe("Bridge wire interface");
    for (const html of [inboxRowHtml(entry), inboxRowHtml(entry, { quiet: true })]) {
      expect(html).toContain("Bridge wire interface");
      expect(html).not.toContain("bridge-wire-interface");
      expect(html).not.toContain(".build/workspaces");
    }
  });

  // A checkout no bridge ever named — an older bridge, an adopted worktree —
  // has only its folder to be called after. That is a last resort, and it is
  // the folder rather than the path: a row is one line, and a path is not a
  // name.
  it("falls back to a nameless workspace's folder rather than its whole path", () => {
    const [entry] = entriesOf([{ id: "external-1", project_id: "project-1", root: "/work/checkouts/build-login" }]);
    expect(entry.name).toBe("build-login");
    const [rootless] = entriesOf([{ id: "external-2", project_id: "project-1" }]);
    expect(rootless.name).toBe("Workspace");
  });

  it("shows the owning conversation's unread and running state without matching branch names", () => {
    const workspaces = [
      { id: "run-1", project_id: "project-1", status: "ready" },
      { id: "run-2", project_id: "project-1", status: "ready" },
      { id: "run-1", project_id: "other-project", status: "ready" },
    ];
    const items = [{ kind: "branch", project_id: "project-1", run_id: "run-1", branch: "main", unread: true, unread_count: 3, working: true }];
    const [owned, unrelated, otherProject] = entriesOf(workspaces, items);
    expect(owned).toMatchObject({ entityId: "run-1", unreadCount: 3, state: "unread", working: true });
    expect(unrelated).toMatchObject({ entityId: null, unreadCount: 0, working: false });
    expect(otherProject).toMatchObject({ entityId: null, unreadCount: 0, working: false });
  });

  it("reconnects an adopted workspace alias to its exact checkout's conversation", () => {
    const [entry] = entriesOf([{ id: "external-1", project_id: "project-1", root: "/work/checkout" }], [
      { kind: "branch", project_id: "project-1", run_id: "run-wrong", worktree_path: "/work/another", branch: "main", unread: true, unread_count: 9 },
      { kind: "branch", project_id: "project-1", run_id: "run-1", worktree_path: "/work/checkout", branch: "main", unread: true, unread_count: 2 },
    ]);
    expect(entry).toMatchObject({ entityId: "run-1", unreadCount: 2, state: "unread" });
  });

  it("opens a workspace at its first Git directory and never renders a destructive action", () => {
    const [entry] = entriesOf([
      {
        id: "workspace-1",
        project_id: "project-1",
        name: "Checkout",
        root: "/work/checkout",
        status: "active",
        work_summary: { pushes: 3, behind: 2, additions: 42, deletions: 7 },
        directories: [
          { id: "docs", source_id: "source-docs", is_git: false },
          { id: "api", source_id: "source-api", is_git: true },
        ],
      },
    ]);

    expect(entry.route).toEqual({
      name: "workspace",
      deviceId: DEVICE,
      projectId: "project-1",
      workspaceId: "workspace-1",
      sourceId: "source-api",
      tab: "changes",
    });
    expect(entry.facts).toBe("↑3 ↓2 +42 −7");
    // No destructive verb on a workspace row that is not clean: the Done
    // action is `data-done="…"` (the menu item) or `data-workspace-done`; the
    // always-present `data-done-error` slot is where a refusal is painted, not
    // a control, so the rule is stated against the action attributes.
    expect(inboxRowHtml(entry)).not.toMatch(/data-done=|data-workspace-done|branch\.finish|Delete/);
  });

  it("marks a workspace route active and supports a workspace with no directories", () => {
    const [entry] = entriesOf([{ id: "workspace-1", project_id: "project-1", name: "Empty" }]);

    expect(entry.route).toEqual({
      name: "workspace",
      deviceId: DEVICE,
      projectId: "project-1",
      workspaceId: "workspace-1",
      tab: "changes",
    });
    expect(entry.facts).toBe("Work summary unavailable");
    expect(activeEntryKey(entry.route, [entry])).toBe(`workspace:${workspaceKey(DEVICE, "workspace-1")}`);
  });

  // The project's own checkout is the template every workspace is cut from,
  // not a place to work; a bridge that still lists it (older ones did) gets it
  // kept out of the rail here, by the one fact that names it: its root is the
  // project's own path.
  it("never lists a project's own checkout, whatever the machine answered", () => {
    const view = liveFeedSnapshot(
      { items: [] },
      { projects: [{ id: "project-1", name: "Payments", path: "/repos/payments" }] },
      { workspaces: [
        { id: "legacy-project-1", project_id: "project-1", name: "Payments", root: "/repos/payments" },
        { id: "workspace-1", project_id: "project-1", name: "Real work", root: "/w/project-1/real-work" },
      ] },
      DEVICE,
    );
    expect(workspaceEntries(view.workspaces, view.projects, view.items).map((row) => row.workspaceId)).toEqual(["workspace-1"]);
  });

  // A checkout Build only adopted is somebody else's folder: it has no
  // conversation, so there is no work to summarize and nothing for Done to
  // remove. Saying the summary is unavailable made a row the user never asked
  // for look like a reporting fault; the row says what it is, and the branch
  // it is standing on.
  it("says an adopted checkout and its branch rather than an unavailable summary", () => {
    const [adopted] = entriesOf([
      {
        id: "wt-abc123",
        project_id: "project-1",
        name: "build-issue-rail",
        root: "/work/checkouts/build-issue-rail",
        status: "ready",
        managed: false,
        directories: [{ id: "root", source_id: "source-1", is_git: true, branch: "build-issue-rail" }],
      },
    ]);
    expect(adopted.facts).toBe("Adopted checkout · build-issue-rail");
    expect(inboxRowHtml(adopted)).not.toMatch(/data-workspace-done/);

    const [nameless] = entriesOf([
      { id: "wt-def456", project_id: "project-1", root: "/work/checkouts/loose", status: "ready", managed: false },
    ]);
    expect(nameless.facts).toBe("Adopted checkout");
  });

  // Only a row with no conversation: an adopted checkout an agent is working
  // in has a conversation to speak for it, a work summary and a Done, and is
  // an ordinary row.
  it("leaves an adopted checkout with a conversation an ordinary row", () => {
    const [entry] = entriesOf(
      [
        {
          id: "wt-abc123",
          project_id: "project-1",
          root: "/work/checkout",
          status: "ready",
          managed: false,
          can_finish: true,
          work_summary: { pushes: 1, additions: 2, deletions: 3 },
        },
      ],
      [{ kind: "branch", project_id: "project-1", run_id: "run-1", worktree_path: "/work/checkout", branch: "main" }],
    );
    expect(entry.facts).toBe("↑1 +2 −3");
    expect(inboxRowHtml(entry)).toMatch(/data-workspace-done/);
  });

  // A bridge too old to say which workspaces it manages says nothing about
  // any of them, and a row is never called adopted on a guess.
  it("does not call a workspace adopted when the bridge says nothing about it", () => {
    const [entry] = entriesOf([{ id: "workspace-1", project_id: "project-1", status: "ready" }]);
    expect(entry.facts).toBe("Work summary unavailable");
  });

  it("does not turn an unknown workspace work summary into zero work", () => {
    const [entry] = entriesOf([{ id: "workspace-1", project_id: "project-1", work_summary: null }]);
    expect(entry.facts).toBe("Work summary unavailable");
    const [partial] = entriesOf([{ id: "workspace-2", project_id: "project-1", work_summary: { pushes: 1 } }]);
    expect(partial.facts).toBe("Work summary unavailable");
    for (const invalid of ["1", -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      const [malformed] = entriesOf([
        { id: "workspace-3", project_id: "project-1", work_summary: { pushes: invalid, additions: 0, deletions: 0 } },
      ]);
      expect(malformed.facts).toBe("Work summary unavailable");
    }
    const [legacy] = entriesOf([
      { id: "workspace-4", project_id: "project-1", work_summary: { pushes: 0, additions: 0, deletions: 0 } },
    ]);
    expect(legacy.facts).toBe("↑0 +0 −0");
    const [withBehind] = entriesOf([
      { id: "workspace-5", project_id: "project-1", work_summary: { pushes: 0, behind: 4, additions: 0, deletions: 0 } },
    ]);
    expect(withBehind.facts).toBe("↑0 ↓4 +0 −0");
    for (const invalid of ["1", -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      const [malformed] = entriesOf([
        { id: "workspace-6", project_id: "project-1", work_summary: { pushes: 0, behind: invalid, additions: 0, deletions: 0 } },
      ]);
      expect(malformed.facts).toBe("Work summary unavailable");
    }
  });

  // A checkout the bridge could not build has no work to summarize, and saying
  // the summary is unavailable reads as a hiccup in the reporting rather than
  // as the thing that went wrong. The row wears the word the switcher wears for
  // the same workspace, and the first line of the reason when there is one.
  it("says a workspace failed, with the first line of why, rather than an unavailable summary", () => {
    const [failed] = entriesOf([
      {
        id: "workspace-1",
        project_id: "project-1",
        status: "failed",
        directories: [
          { source_id: "api", is_git: true, status: "ready" },
          { source_id: "web", status: "failed", error: "failed to make directory '/src/.git/worktrees/web': directory exists\nhint: reuse it" },
        ],
      },
    ]);
    expect(failed.facts).toBe("Failed: failed to make directory '/src/.git/worktrees/web': directory exists");

    const [silent] = entriesOf([{ id: "workspace-2", project_id: "project-1", status: "failed" }]);
    expect(silent.facts).toBe("Failed");
  });
});

// The sync layer's `s-active` subscription names ONE entity: the workspace the
// reader is standing in. It has a route and a device's snapshot and nothing
// else, so the derivation is here, beside the entries the same route marks.
describe("the entity a route is standing on", () => {
  const view = (workspaces, items) => liveFeedSnapshot(
    { items },
    { projects: [{ id: "project-1", name: "Payments" }] },
    { workspaces },
    DEVICE,
  );

  it("is the branch row's entity", () => {
    const snapshot = view([], [{ kind: "branch", project_id: "project-1", branch: "build/login", run_id: "run-7" }]);
    const route = { name: "branch", deviceId: DEVICE, projectId: "project-1", branch: "build/login" };
    expect(routedEntityId(route, snapshot)).toBe("run-7");
  });

  it("is the conversation a workspace row holds, for a workspace route", () => {
    const snapshot = view(
      [{ id: "run-7", project_id: "project-1", name: "wire" }],
      [{ kind: "branch", project_id: "project-1", branch: "build/wire", run_id: "run-7" }],
    );
    const route = { name: "workspace", deviceId: DEVICE, projectId: "project-1", workspaceId: "run-7" };
    expect(routedEntityId(route, snapshot)).toBe("run-7");
  });

  it("uses the workspace's cached owner while its roster row is still landing", () => {
    const snapshot = view(
      [{ id: "workspace-1", project_id: "project-1", entity_id: "run-7", name: "wire" }],
      [],
    );
    const route = { name: "workspace", deviceId: DEVICE, projectId: "project-1", workspaceId: "workspace-1" };
    expect(routedEntityId(route, snapshot)).toBe("run-7");
  });

  it("is that machine's row, never another machine's copy of the same branch", () => {
    const snapshot = view([], [{ kind: "branch", project_id: "project-1", branch: "build/login", run_id: "run-7" }]);
    const elsewhere = { name: "branch", deviceId: "dev-2", projectId: "project-1", branch: "build/login" };
    expect(routedEntityId(elsewhere, snapshot)).toBeNull();
  });

  it("is nothing for a route that names no work item, and nothing for no route", () => {
    const snapshot = view([], []);
    expect(routedEntityId({ name: "inbox" }, snapshot)).toBeNull();
    expect(routedEntityId(null, snapshot)).toBeNull();
  });
});
