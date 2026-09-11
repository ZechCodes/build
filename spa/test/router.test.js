import { describe, it, expect } from "vitest";
import { routeFromHash, hashFromRoute } from "../src/core/router.js";

describe("routeFromHash", () => {
  it("parses workspace directories and their selected tab", () => {
    expect(routeFromHash("#/project/p/workspace/ws/directory/src/files?path=lib%2Fa.js&line=8")).toEqual({
      name: "workspace", projectId: "p", workspaceId: "ws", sourceId: "src", tab: "files", file: "lib/a.js", line: 8,
    });
    expect(routeFromHash("#/project/a%20b/workspace/w%2Fs/directory/source%201/changes")).toEqual({
      name: "workspace", projectId: "a b", workspaceId: "w/s", sourceId: "source 1", tab: "changes",
    });
    expect(routeFromHash("#/project/p/workspace/ws/files")).toEqual({
      name: "workspace", projectId: "p", workspaceId: "ws", tab: "files",
    });
    expect(routeFromHash("#/project/p/workspace/ws/directory/src/nope").tab).toBe("changes");
  });
  it("lands on the inbox for the empty, bare and unknown hashes", () => {
    expect(routeFromHash("")).toEqual({ name: "inbox" });
    expect(routeFromHash("#")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/inbox")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/nope")).toEqual({ name: "inbox" });
  });

  it("parses a capture's decision page, which is named by the capture alone", () => {
    expect(routeFromHash("#/capture/capture-1")).toEqual({ name: "capture", id: "capture-1" });
    expect(routeFromHash("#/capture/capture%201")).toEqual({ name: "capture", id: "capture 1" });
    // A capture route with no capture names nothing.
    expect(routeFromHash("#/capture")).toEqual({ name: "inbox" });
  });

  it("parses a branch work item with its two tabs", () => {
    expect(routeFromHash("#/project/proj-1/branch/main/changes")).toEqual({
      name: "branch", projectId: "proj-1", branch: "main", tab: "changes",
    });
    expect(routeFromHash("#/project/proj-1/branch/main/files").tab).toBe("files");
    // No tab segment is Changes — review is the product.
    expect(routeFromHash("#/project/proj-1/branch/main")).toEqual({
      name: "branch", projectId: "proj-1", branch: "main", tab: "changes",
    });
    expect(routeFromHash("#/project/proj-1/branch/main/bogus").tab).toBe("changes");
  });

  it("carries a slashed branch name, encoded or hand-typed", () => {
    const encoded = "#/project/p/branch/build%2Fui-rebuild/files";
    expect(routeFromHash(encoded)).toEqual({ name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "files" });
    // A hand-typed (unencoded) slash names the same branch.
    expect(routeFromHash("#/project/p/branch/build/ui-rebuild/files")).toEqual({
      name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "files",
    });
    expect(routeFromHash("#/project/p/branch/build/ui-rebuild")).toEqual({
      name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "changes",
    });
    expect(routeFromHash("#/project/a%20b/branch/a%20branch")).toEqual({
      name: "branch", projectId: "a b", branch: "a branch", tab: "changes",
    });
  });

  // The Files tab names a file, so a link to a file is a link somebody can send
  // — and a reload keeps the reviewer's place. It rides as a query rather than
  // as more path segments: both a branch name and a file path carry slashes,
  // and two slashed things in one path cannot be told apart.
  describe("the file the Files tab is open on", () => {
    it("carries the path and the line", () => {
      expect(routeFromHash("#/project/p/branch/main/files?path=src%2Fapp.js&line=42")).toEqual({
        name: "branch", projectId: "p", branch: "main", tab: "files", file: "src/app.js", line: 42,
      });
    });

    it("carries a path with no line", () => {
      const route = routeFromHash("#/project/p/branch/main/files?path=src%2Fapp.js");
      expect(route.file).toBe("src/app.js");
      expect(route.line).toBeUndefined();
    });

    it("names no file when the tab is open on none", () => {
      expect(routeFromHash("#/project/p/branch/main/files").file).toBeUndefined();
    });

    it("survives a slashed branch name beside a slashed path", () => {
      const route = routeFromHash("#/project/p/branch/build%2Fui/files?path=src%2Fcore%2Fapp.js&line=7");
      expect(route.branch).toBe("build/ui");
      expect(route.file).toBe("src/core/app.js");
      expect(route.line).toBe(7);
    });

    it("ignores a line that is not a line", () => {
      expect(routeFromHash("#/project/p/branch/main/files?path=a.js&line=nope").line).toBeUndefined();
    });

    it("writes the file back into the hash", () => {
      expect(hashFromRoute({ name: "branch", projectId: "p", branch: "build/ui", tab: "files", file: "src/app.js", line: 42 })).toBe(
        "#/project/p/branch/build%2Fui/files?path=src%2Fapp.js&line=42",
      );
    });

    it("writes a file with no line", () => {
      expect(hashFromRoute({ name: "branch", projectId: "p", branch: "main", tab: "files", file: "a.js" })).toBe(
        "#/project/p/branch/main/files?path=a.js",
      );
    });

    it("says nothing about a file on the Changes tab, which has none", () => {
      expect(hashFromRoute({ name: "branch", projectId: "p", branch: "main", tab: "changes", file: "a.js" })).toBe(
        "#/project/p/branch/main/changes",
      );
    });

    it("round-trips", () => {
      const route = { name: "branch", projectId: "p", branch: "build/ui", tab: "files", file: "src/a b.js", line: 3 };
      expect(routeFromHash(hashFromRoute(route))).toEqual(route);
    });
  });

  // A branch may be NAMED after a tab; a lone segment is always the branch.
  it("reads a lone segment as the branch even when it spells a tab", () => {
    expect(routeFromHash("#/project/p/branch/changes")).toEqual({
      name: "branch", projectId: "p", branch: "changes", tab: "changes",
    });
    expect(routeFromHash("#/project/p/branch/files")).toEqual({
      name: "branch", projectId: "p", branch: "files", tab: "changes",
    });
  });

  it("parses an issue with no tab segment and an optional stage suffix", () => {
    expect(routeFromHash("#/project/p/issue/plan-1")).toEqual({ name: "issue", projectId: "p", id: "plan-1" });
    expect(routeFromHash("#/project/p/issue/plan-1/stage/second-half")).toEqual({
      name: "issue", projectId: "p", id: "plan-1", stage: "second-half",
    });
    // No stage suffix → no stage key at all, so equality checks stay clean.
    expect(routeFromHash("#/project/p/issue/plan-1").stage).toBeUndefined();
    expect(routeFromHash("#/project/a%20b/issue/p%20l/stage/s%20x")).toEqual({
      name: "issue", projectId: "a b", id: "p l", stage: "s x",
    });
  });

  it("parses the account pages", () => {
    expect(routeFromHash("#/account")).toEqual({ name: "account", page: "settings" });
    expect(routeFromHash("#/account/settings")).toEqual({ name: "account", page: "settings" });
    expect(routeFromHash("#/account/devices")).toEqual({ name: "account", page: "devices" });
    expect(routeFromHash("#/account/archive")).toEqual({ name: "account", page: "archive" });
    expect(routeFromHash("#/account/bogus")).toEqual({ name: "account", page: "settings" });
  });
});

// Every URL the pre-redesign client could mint still opens something, and it
// opens the nearest thing in the new shell. The ones that name an entity whose
// branch a pure function cannot know park on a `resolve` route for the app to
// look up in the feed.
describe("legacy routes canonicalize to the nearest new route", () => {
  it("sends the retired global surfaces to the inbox", () => {
    expect(routeFromHash("#/notifications")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/board")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/task")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/plan")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/issue")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/worktree")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/worktree/proj-1")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/main")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/project")).toEqual({ name: "inbox" });
  });

  it("sends settings to its account page", () => {
    expect(routeFromHash("#/settings")).toEqual({ name: "account", page: "settings" });
  });

  it("parks a run URL on a resolve route carrying the canonical tab", () => {
    expect(routeFromHash("#/task/run-1")).toEqual({ name: "resolve", kind: "run", id: "run-1", tab: "changes" });
    expect(routeFromHash("#/task/run-1/files")).toEqual({ name: "resolve", kind: "run", id: "run-1", tab: "files" });
    expect(routeFromHash("#/project/p/task/run-1/files")).toEqual({
      name: "resolve", kind: "run", projectId: "p", id: "run-1", tab: "files",
    });
    expect(routeFromHash("#/task/a%20b/files").id).toBe("a b");
  });

  it("parks a worktree URL on a resolve route", () => {
    expect(routeFromHash("#/worktree/p/wt-1/files")).toEqual({
      name: "resolve", kind: "worktree", projectId: "p", id: "wt-1", tab: "files",
    });
    expect(routeFromHash("#/project/p/worktree/wt-1")).toEqual({
      name: "resolve", kind: "worktree", projectId: "p", id: "wt-1", tab: "changes",
    });
    expect(routeFromHash("#/worktree/a%20b/wt%201")).toEqual({
      name: "resolve", kind: "worktree", projectId: "a b", id: "wt 1", tab: "changes",
    });
  });

  it("parks the primary-checkout URLs on a resolve route", () => {
    expect(routeFromHash("#/project/p")).toEqual({ name: "resolve", kind: "primary", projectId: "p", tab: "changes" });
    expect(routeFromHash("#/project/p/files")).toEqual({ name: "resolve", kind: "primary", projectId: "p", tab: "files" });
    expect(routeFromHash("#/main/p")).toEqual({ name: "resolve", kind: "primary", projectId: "p", tab: "changes" });
    expect(routeFromHash("#/main/p/files")).toEqual({ name: "resolve", kind: "primary", projectId: "p", tab: "files" });
  });

  it("parks a project-less issue URL on a resolve route, stage and all", () => {
    expect(routeFromHash("#/issue/pl-1")).toEqual({ name: "resolve", kind: "issue", id: "pl-1" });
    expect(routeFromHash("#/plan/pl-1")).toEqual({ name: "resolve", kind: "issue", id: "pl-1" });
    expect(routeFromHash("#/plan/pl-1/stages/s2")).toEqual({ name: "resolve", kind: "issue", id: "pl-1", stage: "s2" });
    expect(routeFromHash("#/plan/pl-1/review/s2")).toEqual({ name: "resolve", kind: "issue", id: "pl-1", stage: "s2" });
    expect(routeFromHash("#/plan/pl-1/conversation")).toEqual({ name: "resolve", kind: "issue", id: "pl-1" });
  });

  it("resolves a project-scoped plan URL straight to its issue", () => {
    expect(routeFromHash("#/project/p/plan/pl-1")).toEqual({ name: "issue", projectId: "p", id: "pl-1" });
    expect(routeFromHash("#/project/p/plan/pl-1/stages/s2")).toEqual({ name: "issue", projectId: "p", id: "pl-1", stage: "s2" });
    expect(routeFromHash("#/project/p/issue/pl-1/review/s2")).toEqual({ name: "issue", projectId: "p", id: "pl-1", stage: "s2" });
    expect(routeFromHash("#/project/p/issue/pl-1/conversation")).toEqual({ name: "issue", projectId: "p", id: "pl-1" });
    expect(routeFromHash("#/project/p/issue/pl-1/agent")).toEqual({ name: "issue", projectId: "p", id: "pl-1" });
  });

  // Conversation and Agent are the agent rail now; Stages is the issue view;
  // Diff merged into Changes; terminals moved to the console. Every one of them
  // lands on the entity's Changes tab.
  it("folds the retired entity tabs into Changes", () => {
    for (const tab of ["conversation", "agent", "stages", "diff", "plan", "term-1", "term-12", "bogus"]) {
      expect([tab, routeFromHash(`#/task/r/${tab}`).tab]).toEqual([tab, "changes"]);
      expect([tab, routeFromHash(`#/worktree/p/w/${tab}`).tab]).toEqual([tab, "changes"]);
      expect([tab, routeFromHash(`#/project/p/branch/main/${tab}`).tab]).toEqual([tab, "changes"]);
      // `plan` under a bare project named the plan COLLECTION, not a tab.
      if (tab !== "plan") expect([tab, routeFromHash(`#/project/p/${tab}`).tab]).toEqual([tab, "changes"]);
    }
    expect(routeFromHash("#/project/p/plan")).toEqual({ name: "inbox" });
    // Files is the one entity tab that survived under its own name.
    for (const hash of ["#/task/r/files", "#/worktree/p/w/files", "#/project/p/files", "#/project/p/branch/main/files"]) {
      expect([hash, routeFromHash(hash).tab]).toEqual([hash, "files"]);
    }
  });

  // A terminal tab named a terminal, and the terminal outlived the tab: the
  // surface is the entity's Changes, with the console open on that terminal.
  it("carries the terminal a term-<n> tab named", () => {
    expect(routeFromHash("#/project/p/branch/main/term-3")).toEqual({
      name: "branch", projectId: "p", branch: "main", tab: "changes", term: "term-3",
    });
    expect(routeFromHash("#/task/r/term-1")).toEqual({ name: "resolve", kind: "run", id: "r", tab: "changes", term: "term-1" });
    expect(routeFromHash("#/worktree/p/w/term-12")).toEqual({
      name: "resolve", kind: "worktree", projectId: "p", id: "w", tab: "changes", term: "term-12",
    });
    expect(routeFromHash("#/main/p/term-2")).toEqual({ name: "resolve", kind: "primary", projectId: "p", tab: "changes", term: "term-2" });
    // Every other tab names no terminal.
    expect(routeFromHash("#/project/p/branch/main/diff").term).toBeUndefined();
    // A branch NAMED like a terminal tab is still a branch.
    expect(routeFromHash("#/project/p/branch/term-3")).toEqual({
      name: "branch", projectId: "p", branch: "term-3", tab: "changes",
    });
  });

  // The old tab bar's right cluster named project-wide panes, not the entity's
  // work: they belong to the global surfaces that own them now.
  it("sends the cluster tabs to the global surface that owns them", () => {
    for (const base of ["#/task/r", "#/worktree/p/w", "#/project/p", "#/project/p/branch/main", "#/project/p/issue/i", "#/main/p"]) {
      expect([base, routeFromHash(`${base}/inbox`)]).toEqual([base, { name: "inbox" }]);
      expect([base, routeFromHash(`${base}/issues`)]).toEqual([base, { name: "inbox" }]);
      expect([base, routeFromHash(`${base}/archive`)]).toEqual([base, { name: "account", page: "archive" }]);
    }
  });
});

describe("hashFromRoute", () => {
  it("is the inverse of routeFromHash", () => {
    for (const route of [
      { name: "inbox" },
      { name: "account", page: "settings" },
      { name: "account", page: "devices" },
      { name: "account", page: "archive" },
      { name: "branch", projectId: "p", branch: "main", tab: "changes" },
      { name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "files" },
      { name: "branch", projectId: "a b", branch: "a branch", tab: "changes" },
      { name: "issue", projectId: "p", id: "pl-1" },
      { name: "issue", projectId: "a b", id: "pl 1", stage: "stage 2" },
      { name: "capture", id: "capture-1" },
      { name: "capture", id: "capture 1" },
      { name: "workspace", projectId: "p", workspaceId: "ws", sourceId: "src", tab: "changes" },
      { name: "workspace", projectId: "a b", workspaceId: "w/s", sourceId: "source 1", tab: "files", file: "src/a b.js", line: 3 },
      { name: "workspace", projectId: "p", workspaceId: "ws", tab: "files" },
    ]) {
      expect([route, routeFromHash(hashFromRoute(route))]).toEqual([route, route]);
    }
  });

  it("writes the canonical hashes", () => {
    expect(hashFromRoute({ name: "inbox" })).toBe("#/inbox");
    expect(hashFromRoute({ name: "account", page: "archive" })).toBe("#/account/archive");
    expect(hashFromRoute({ name: "branch", projectId: "p", branch: "main", tab: "files" })).toBe("#/project/p/branch/main/files");
    expect(hashFromRoute({ name: "branch", projectId: "p", branch: "build/x" })).toBe("#/project/p/branch/build%2Fx/changes");
    expect(hashFromRoute({ name: "issue", projectId: "p", id: "i-1" })).toBe("#/project/p/issue/i-1");
    expect(hashFromRoute({ name: "issue", projectId: "p", id: "i-1", stage: "s2" })).toBe("#/project/p/issue/i-1/stage/s2");
    expect(hashFromRoute({ name: "workspace", projectId: "p", workspaceId: "ws", sourceId: "src", tab: "files" })).toBe(
      "#/project/p/workspace/ws/directory/src/files",
    );
    expect(hashFromRoute({ name: "capture", id: "capture-1" })).toBe("#/capture/capture-1");
    expect(hashFromRoute({ name: "capture" })).toBe("#/inbox");
    expect(hashFromRoute({ name: "account" })).toBe("#/account/settings");
  });

  it("falls back to the inbox for routes that name no surface", () => {
    expect(hashFromRoute({ name: "board" })).toBe("#/inbox");
    expect(hashFromRoute({ name: "whatever" })).toBe("#/inbox");
    // A resolve route is a parse result, never a destination: it keeps the
    // legacy hash on screen until the app rewrites it, so asking for its hash
    // asks for the landing surface.
    expect(hashFromRoute({ name: "resolve", kind: "run", id: "r" })).toBe("#/inbox");
    // An incomplete work-item route cannot address anything.
    expect(hashFromRoute({ name: "branch", projectId: "p" })).toBe("#/inbox");
    expect(hashFromRoute({ name: "issue", id: "i" })).toBe("#/inbox");
    expect(hashFromRoute({ name: "workspace", projectId: "p" })).toBe("#/inbox");
  });
});

describe("device settings routes", () => {
  it("round-trips a device id separately from the active workspace", () => {
    const route = { name: "device", id: "device / 2" };
    expect(hashFromRoute(route)).toBe("#/device/device%20%2F%202/settings");
    expect(routeFromHash(hashFromRoute(route))).toEqual(route);
  });
});
