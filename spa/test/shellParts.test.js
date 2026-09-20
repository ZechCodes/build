// What the shell shows for a route, derived from the route alone.
//
// The rail and the console are the shell's, not a page's. A page that mounts
// its own rail is a page that can forget one, and one did: the issue page
// (views/trackerIssueView.js) mounted none, so opening an issue on a phone lost
// the bubble strip and left the bar holding nothing but the status dot. This
// table is the fix — every route says what it stands on here, in one place, so
// a new page gets a rail whether or not anybody remembered to give it one.

import { describe, it, expect } from "vitest";

import { shellPartsForRoute } from "../src/core/shell.js";

describe("what a route stands on", () => {
  it("gives the tracker's issue page the project's conversation", () => {
    // The route Zech opened on his phone. It mounted no rail at all.
    //
    // It stands on the PROJECT's agent rather than on one of the issue's own: a
    // tracker issue carries no conversation — the agents its page names are
    // workspace agents it can be assigned to — and `kind: "issue"` addresses the
    // legacy multi-stage issue record, which a tracker issue id is not.
    const parts = shellPartsForRoute({ name: "trackerIssue", deviceId: "d1", projectId: "p-1", issueId: "i-1" });
    expect(parts.rail).toMatchObject({ kind: "project", projectId: "p-1", deviceId: "d1" });
    expect(parts.key).toBe("project:d1/p-1");
  });

  it("gives an issue of the project the same standing as the project page", () => {
    // So pressing an issue on the Issues tab swaps the page under a strip that
    // does not move.
    const page = shellPartsForRoute({ name: "project", deviceId: "d1", projectId: "p-1", tab: "issues" });
    const issue = shellPartsForRoute({ name: "trackerIssue", deviceId: "d1", projectId: "p-1", issueId: "i-1" });
    expect(issue.key).toBe(page.key);
  });

  it("keeps the legacy issue page on the issue's own conversation", () => {
    // The legacy multi-stage issue is the one issue that does carry one.
    const legacy = shellPartsForRoute({ name: "issue", deviceId: "d1", projectId: "p-1", id: "i-1" });
    expect(legacy.rail).toMatchObject({ kind: "issue", issueId: "i-1", projectId: "p-1", deviceId: "d1" });
    expect(legacy.key).toBe("issue:i-1");
  });

  it("stands a project page on the project's own agent, and says the conversation must be minted", () => {
    const parts = shellPartsForRoute({ name: "project", deviceId: "d1", projectId: "p-1" });
    expect(parts.rail).toMatchObject({ kind: "project", projectId: "p-1" });
    // Standing ON the project IS standing on its conversation, so the shell
    // mints it before the rail goes up. A workspace page carries the same
    // project agent as a bubble and mints nothing (see below).
    expect(parts.mintsProjectConversation).toBe(true);
    // Every machine mints a `proj-1`, so the key carries the machine too.
    expect(parts.key).toBe("project:d1/p-1");
  });

  it("carries the project's agent onto a workspace page without minting it", () => {
    const parts = shellPartsForRoute({ name: "workspace", deviceId: "d1", projectId: "p-1", workspaceId: "w-1" });
    expect(parts.rail).toMatchObject({ kind: "workspace", workspaceId: "w-1", projectAgent: { projectId: "p-1" } });
    // A rail that minted one to paint a bubble would give every workspace page
    // a project agent, a scratch directory and a run nobody asked for.
    expect(parts.mintsProjectConversation).toBe(false);
  });

  it("keeps the branch page's conversation and its console on one address", () => {
    const parts = shellPartsForRoute({ name: "branch", deviceId: "d1", projectId: "p-1", branch: "build/login" });
    expect(parts.rail).toMatchObject({ kind: "branch", branch: "build/login" });
    expect(parts.console).toMatchObject({ kind: "branch", branch: "build/login", projectId: "p-1", deviceId: "d1" });
    expect(parts.key).toBe("branch:p-1:build/login");
  });

  it("gives the workspace console the workspace rather than the directory open in it", () => {
    // Moving between a workspace's directories must not replace the sessions
    // the console is holding.
    const here = shellPartsForRoute({ name: "workspace", deviceId: "d1", projectId: "p-1", workspaceId: "w-1", sourceId: "s-1" });
    const there = shellPartsForRoute({ name: "workspace", deviceId: "d1", projectId: "p-1", workspaceId: "w-1", sourceId: "s-2" });
    expect(here.console).toEqual(there.console);
    expect(here.key).toBe(there.key);
  });

  it("opens the agent a conversation link names", () => {
    const parts = shellPartsForRoute({ name: "workspace", deviceId: "d1", projectId: "p-1", workspaceId: "w-1", agent: "a-9" });
    expect(parts.rail.openAgentId).toBe("a-9");
  });

  it("does not re-key a project page when its tab changes", () => {
    // Workspaces ⇄ Issues is a page swapping inside the shell, not a move to
    // another conversation: the strip stays exactly where it was.
    const workspaces = shellPartsForRoute({ name: "project", deviceId: "d1", projectId: "p-1" });
    const issues = shellPartsForRoute({ name: "project", deviceId: "d1", projectId: "p-1", tab: "issues" });
    expect(workspaces.key).toBe(issues.key);
  });

  it("stands nowhere on the routes that are not a place with a conversation", () => {
    for (const route of [
      { name: "inbox" },
      { name: "capture", id: "c-1" },
      { name: "resolve", kind: "run", id: "run-1" },
      { name: "account", page: "settings" },
      { name: "device", id: "d1" },
    ]) {
      expect([route.name, shellPartsForRoute(route)]).toEqual([route.name, null]);
    }
  });

  it("stands nowhere on a work route that does not say enough to name a conversation", () => {
    expect(shellPartsForRoute({ name: "workspace", deviceId: "d1", projectId: "p-1" })).toBeNull();
    expect(shellPartsForRoute({ name: "branch", deviceId: "d1", projectId: "p-1" })).toBeNull();
    expect(shellPartsForRoute({ name: "issue", deviceId: "d1", projectId: "p-1" })).toBeNull();
    expect(shellPartsForRoute({ name: "project", deviceId: "d1" })).toBeNull();
    expect(shellPartsForRoute({ name: "trackerIssue", deviceId: "d1", issueId: "i-1" })).toBeNull();
  });
});
