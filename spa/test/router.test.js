import { describe, it, expect } from "vitest";
import { conversationRoute, routeFromHash, hashFromRoute, withDeviceOrResolve } from "../src/core/router.js";

// A work URL that names no device names no machine: every device mints a
// `proj-1`, so `#/project/proj-1/branch/main` parks on a resolve route carrying
// the route it meant, and the app looks the project up across the account. The
// cases below are about that inner route; the parking itself has its own
// section at the bottom.
const workRoute = (hash) => {
  const route = routeFromHash(hash);
  return route.name === "resolve" && route.route ? route.route : route;
};

describe("routeFromHash", () => {
  it("round-trips a workspace request to create an agent", () => {
    const route = { name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "changes", newAgent: true };
    expect(routeFromHash(hashFromRoute(route))).toEqual(route);
  });
  it("round-trips a routed task comment and a workspace commit", () => {
    const comment = { name: "trackerTask", deviceId: "d1", projectId: "p", taskId: "task-42", commentId: "tc-7" };
    expect(hashFromRoute(comment)).toBe("#/device/d1/project/p/tasks/task-42/c/tc-7");
    expect(routeFromHash(hashFromRoute(comment))).toEqual(comment);
    const commit = { name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", sourceId: "src",
      tab: "changes", commit: "b8ce4ee9" };
    expect(routeFromHash(hashFromRoute(commit))).toEqual(commit);
    expect(hashFromRoute(commit)).toContain("?commit=b8ce4ee9");
  });

  it("parses workspace directories and their selected tab", () => {
    const withDirectory = "#/device/d1/project/p/workspace/ws/directory/src/files?path=lib%2Fa.js&line=8";
    expect(routeFromHash(withDirectory)).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", sourceId: "src", tab: "files",
      file: "lib/a.js", line: 8,
    });
    expect(hashFromRoute(routeFromHash(withDirectory))).toBe(withDirectory);

    expect(routeFromHash("#/device/d%201/project/a%20b/workspace/w%2Fs/directory/source%201/changes")).toEqual({
      name: "workspace", deviceId: "d 1", projectId: "a b", workspaceId: "w/s", sourceId: "source 1", tab: "changes",
    });
    expect(routeFromHash("#/device/d1/project/p/workspace/ws/files")).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "files",
    });
    expect(routeFromHash("#/device/d1/project/p/workspace/ws/directory/src/nope").tab).toBe("changes");
  });

  // #29. The tab is the workspace's, not the checkout's: a directory never
  // scopes it, and a branch has no tasks tab at all.
  it("parses the workspace tasks tab, its board and one task inside it", () => {
    expect(routeFromHash("#/device/d1/project/p/workspace/ws/tasks")).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "tasks",
    });
    expect(routeFromHash("#/device/d1/project/p/workspace/ws/tasks?view=board")).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "tasks", view: "board",
    });
    expect(routeFromHash("#/device/d1/project/p/workspace/ws/tasks/task-7")).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "tasks", taskId: "task-7",
    });
  });

  // A branch is a checkout and has no agents to hold tasks, so `tasks` there
  // is still the retired right-cluster tab that named the inbox.
  it("gives a branch no tasks tab", () => {
    expect(routeFromHash("#/device/d1/project/p/branch/main/tasks").name).toBe("inbox");
  });

  // A workspace is one machine's checkout, so a workspace URL with no device in
  // it is the same question a device-less branch URL is, and parks in the same
  // place — carrying the whole workspace route, directory and file included.
  it("parks a device-less workspace URL on the resolve route", () => {
    expect(routeFromHash("#/project/p/workspace/ws/files")).toEqual({
      name: "resolve", kind: "project", projectId: "p",
      route: { name: "workspace", projectId: "p", workspaceId: "ws", tab: "files" },
    });
    expect(routeFromHash("#/project/p/workspace/ws/directory/src/files?path=lib%2Fa.js&line=8")).toEqual({
      name: "resolve", kind: "project", projectId: "p",
      route: {
        name: "workspace", projectId: "p", workspaceId: "ws", sourceId: "src", tab: "files",
        file: "lib/a.js", line: 8,
      },
    });
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
    expect(workRoute("#/project/proj-1/branch/main/changes")).toEqual({
      name: "branch", projectId: "proj-1", branch: "main", tab: "changes",
    });
    expect(workRoute("#/project/proj-1/branch/main/files").tab).toBe("files");
    // No tab segment is Changes — review is the product.
    expect(workRoute("#/project/proj-1/branch/main")).toEqual({
      name: "branch", projectId: "proj-1", branch: "main", tab: "changes",
    });
    expect(workRoute("#/project/proj-1/branch/main/bogus").tab).toBe("changes");
  });

  it("carries a slashed branch name, encoded or hand-typed", () => {
    const encoded = "#/project/p/branch/build%2Fui-rebuild/files";
    expect(workRoute(encoded)).toEqual({ name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "files" });
    // A hand-typed (unencoded) slash names the same branch.
    expect(workRoute("#/project/p/branch/build/ui-rebuild/files")).toEqual({
      name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "files",
    });
    expect(workRoute("#/project/p/branch/build/ui-rebuild")).toEqual({
      name: "branch", projectId: "p", branch: "build/ui-rebuild", tab: "changes",
    });
    expect(workRoute("#/project/a%20b/branch/a%20branch")).toEqual({
      name: "branch", projectId: "a b", branch: "a branch", tab: "changes",
    });
  });

  // The Files tab names a file, so a link to a file is a link somebody can send
  // — and a reload keeps the reviewer's place. It rides as a query rather than
  // as more path segments: both a branch name and a file path carry slashes,
  // and two slashed things in one path cannot be told apart.
  describe("the file the Files tab is open on", () => {
    it("carries the path and the line", () => {
      expect(workRoute("#/project/p/branch/main/files?path=src%2Fapp.js&line=42")).toEqual({
        name: "branch", projectId: "p", branch: "main", tab: "files", file: "src/app.js", line: 42,
      });
    });

    it("carries a path with no line", () => {
      const route = workRoute("#/project/p/branch/main/files?path=src%2Fapp.js");
      expect(route.file).toBe("src/app.js");
      expect(route.line).toBeUndefined();
    });

    it("names no file when the tab is open on none", () => {
      expect(workRoute("#/project/p/branch/main/files").file).toBeUndefined();
    });

    it("survives a slashed branch name beside a slashed path", () => {
      const route = workRoute("#/project/p/branch/build%2Fui/files?path=src%2Fcore%2Fapp.js&line=7");
      expect(route.branch).toBe("build/ui");
      expect(route.file).toBe("src/core/app.js");
      expect(route.line).toBe(7);
    });

    it("ignores a line that is not a line", () => {
      expect(workRoute("#/project/p/branch/main/files?path=a.js&line=nope").line).toBeUndefined();
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
      expect(workRoute(hashFromRoute(route))).toEqual(route);
    });
  });

  // A branch may be NAMED after a tab; a lone segment is always the branch.
  it("reads a lone segment as the branch even when it spells a tab", () => {
    expect(workRoute("#/project/p/branch/changes")).toEqual({
      name: "branch", projectId: "p", branch: "changes", tab: "changes",
    });
    expect(workRoute("#/project/p/branch/files")).toEqual({
      name: "branch", projectId: "p", branch: "files", tab: "changes",
    });
  });

  it("parses a task with no tab segment and an optional stage suffix", () => {
    expect(workRoute("#/project/p/task/plan-1")).toEqual({ name: "task", projectId: "p", id: "plan-1" });
    expect(workRoute("#/project/p/task/plan-1/stage/second-half")).toEqual({
      name: "task", projectId: "p", id: "plan-1", stage: "second-half",
    });
    // No stage suffix → no stage key at all, so equality checks stay clean.
    expect(workRoute("#/project/p/task/plan-1").stage).toBeUndefined();
    expect(workRoute("#/project/a%20b/task/p%20l/stage/s%20x")).toEqual({
      name: "task", projectId: "a b", id: "p l", stage: "s x",
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
    expect(routeFromHash("#/task")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/worktree")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/worktree/proj-1")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/main")).toEqual({ name: "inbox" });
    // `#/main/<id>` was the base checkout under an older spelling. That checkout
    // has no surface, and the spelling named no machine, so it is gone whole.
    expect(routeFromHash("#/main/p")).toEqual({ name: "inbox" });
    expect(routeFromHash("#/main/p/files")).toEqual({ name: "inbox" });
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

  // A project has a page of its own: its workspaces in the main pane, its agent
  // in the rail. Its base checkout has no surface — that is what workspaces are
  // cut from — so every URL that used to open the checkout opens the page.
  it("lands every base-project URL on the project's own page", () => {
    for (const hash of ["#/project/p", "#/project/p/files", "#/project/p/changes", "#/project/p/bogus"]) {
      expect([hash, workRoute(hash)]).toEqual([hash, { name: "project", projectId: "p", tab: "tasks" }]);
    }
    // A collection under a project with nothing named in it says the project and
    // no more, so it lands there too.
    for (const hash of ["#/project/p/workspace", "#/project/p/branch", "#/project/p/task", "#/project/p/worktree"]) {
      expect([hash, workRoute(hash)]).toEqual([hash, { name: "project", projectId: "p", tab: "tasks" }]);
    }
    expect(routeFromHash("#/device/d1/project/p")).toEqual({ name: "project", deviceId: "d1", projectId: "p", tab: "tasks" });
  });

  // The project is one machine's, so a project URL with no device in it is the
  // same question every other work URL asks: which machine holds this `proj-1`.
  it("parks a device-less project URL on the resolve hop", () => {
    expect(routeFromHash("#/project/p")).toEqual({
      name: "resolve", kind: "project", projectId: "p", route: { name: "project", projectId: "p", tab: "tasks" },
    });
  });

  it("parks a project-less task URL on a resolve route, stage and all", () => {
    expect(routeFromHash("#/task/pl-1")).toEqual({ name: "resolve", kind: "task", id: "pl-1" });
    expect(routeFromHash("#/plan/pl-1")).toEqual({ name: "resolve", kind: "task", id: "pl-1" });
    expect(routeFromHash("#/plan/pl-1/stages/s2")).toEqual({ name: "resolve", kind: "task", id: "pl-1", stage: "s2" });
    expect(routeFromHash("#/plan/pl-1/review/s2")).toEqual({ name: "resolve", kind: "task", id: "pl-1", stage: "s2" });
    expect(routeFromHash("#/plan/pl-1/conversation")).toEqual({ name: "resolve", kind: "task", id: "pl-1" });
  });

  it("resolves a project-scoped plan URL straight to its task", () => {
    expect(workRoute("#/project/p/plan/pl-1")).toEqual({ name: "task", projectId: "p", id: "pl-1" });
    expect(workRoute("#/project/p/plan/pl-1/stages/s2")).toEqual({ name: "task", projectId: "p", id: "pl-1", stage: "s2" });
    expect(workRoute("#/project/p/task/pl-1/review/s2")).toEqual({ name: "task", projectId: "p", id: "pl-1", stage: "s2" });
    expect(workRoute("#/project/p/task/pl-1/conversation")).toEqual({ name: "task", projectId: "p", id: "pl-1" });
    expect(workRoute("#/project/p/task/pl-1/agent")).toEqual({ name: "task", projectId: "p", id: "pl-1" });
  });

  // Conversation and Agent are the agent rail now; Stages is the task view;
  // Diff merged into Changes; terminals moved to the console. Every one of them
  // lands on the entity's Changes tab.
  it("folds the retired entity tabs into Changes", () => {
    for (const tab of ["conversation", "agent", "stages", "diff", "plan", "term-1", "term-12", "bogus"]) {
      expect([tab, routeFromHash(`#/task/r/${tab}`).tab]).toEqual([tab, "changes"]);
      expect([tab, routeFromHash(`#/worktree/p/w/${tab}`).tab]).toEqual([tab, "changes"]);
      expect([tab, workRoute(`#/project/p/branch/main/${tab}`).tab]).toEqual([tab, "changes"]);
      // Under a BARE project these named the base checkout's tabs, and that
      // checkout has no surface any more: they land on the project's own page,
      // which has no tab at all.
      expect([tab, workRoute(`#/project/p/${tab}`)]).toEqual([tab, { name: "project", projectId: "p", tab: "tasks" }]);
    }
    expect(workRoute("#/project/p/plan")).toEqual({ name: "project", projectId: "p", tab: "tasks" });
    // Files is the one entity tab that survived under its own name.
    for (const hash of ["#/task/r/files", "#/worktree/p/w/files", "#/project/p/branch/main/files"]) {
      expect([hash, workRoute(hash).tab]).toEqual([hash, "files"]);
    }
  });

  // A terminal tab named a terminal, and the terminal outlived the tab: the
  // surface is the entity's Changes, with the console open on that terminal.
  it("carries the terminal a term-<n> tab named", () => {
    expect(workRoute("#/project/p/branch/main/term-3")).toEqual({
      name: "branch", projectId: "p", branch: "main", tab: "changes", term: "term-3",
    });
    expect(routeFromHash("#/task/r/term-1")).toEqual({ name: "resolve", kind: "run", id: "r", tab: "changes", term: "term-1" });
    expect(routeFromHash("#/worktree/p/w/term-12")).toEqual({
      name: "resolve", kind: "worktree", projectId: "p", id: "w", tab: "changes", term: "term-12",
    });
    // A terminal named on a base-project URL had nothing to open it on: the
    // project's checkout is not a surface, so there is no console to put it in.
    expect(workRoute("#/project/p/term-2")).toEqual({ name: "project", projectId: "p", tab: "tasks" });
    // Every other tab names no terminal.
    expect(routeFromHash("#/project/p/branch/main/diff").term).toBeUndefined();
    // A branch NAMED like a terminal tab is still a branch.
    expect(workRoute("#/project/p/branch/term-3")).toEqual({
      name: "branch", projectId: "p", branch: "term-3", tab: "changes",
    });
  });

  // The old tab bar's right cluster named project-wide panes, not the entity's
  // work: they belong to the global surfaces that own them now.
  it("sends the cluster tabs to the global surface that owns them", () => {
    for (const base of ["#/task/r", "#/worktree/p/w", "#/project/p/branch/main", "#/project/p/task/i"]) {
      expect([base, routeFromHash(`${base}/inbox`)]).toEqual([base, { name: "inbox" }]);
      expect([base, routeFromHash(`${base}/tasks`)]).toEqual([base, { name: "inbox" }]);
      expect([base, routeFromHash(`${base}/archive`)]).toEqual([base, { name: "account", page: "archive" }]);
    }
    // On a bare project the inbox cluster keeps the project it was read on: the
    // rail standing there is a surface of its own, beside the project's page.
    // Archive is still the account's one archive.
    expect(routeFromHash("#/project/p/inbox")).toEqual({ name: "inbox", projectId: "p" });
    expect(routeFromHash("#/project/p/archive")).toEqual({ name: "account", page: "archive" });
  });

  // The rail standing in a project needs a URL of its own: the app rewrites the
  // hash to whatever the route it took up writes, so without one every project
  // link would land scoped and re-parse as the whole account's inbox.
  it("round-trips the inbox standing in a project, device and all", () => {
    for (const hash of ["#/project/p/inbox", "#/device/d1/project/p1/inbox"]) {
      expect([hash, hashFromRoute(routeFromHash(hash))]).toEqual([hash, hash]);
    }
    expect(hashFromRoute({ name: "inbox" })).toBe("#/inbox");
    expect(hashFromRoute(workRoute("#/project/p"))).toBe("#/project/p");
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
      { name: "task", projectId: "p", id: "pl-1" },
      { name: "task", projectId: "a b", id: "pl 1", stage: "stage 2" },
      { name: "capture", id: "capture-1" },
      { name: "capture", id: "capture 1" },
      { name: "workspace", projectId: "p", workspaceId: "ws", sourceId: "src", tab: "changes" },
      { name: "workspace", projectId: "a b", workspaceId: "w/s", sourceId: "source 1", tab: "files", file: "src/a b.js", line: 3 },
      { name: "workspace", projectId: "p", workspaceId: "ws", tab: "files" },
      // #29: the workspace's own tasks tab, its board, and one task opened
      // inside it — all still `name: "workspace"`, which is what keeps the
      // shell's rail standing across the switch.
      { name: "workspace", projectId: "p", workspaceId: "ws", tab: "tasks" },
      { name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "tasks", view: "board" },
      { name: "workspace", projectId: "p", workspaceId: "ws", tab: "tasks", taskId: "task-1" },
      { name: "workspace", projectId: "a b", workspaceId: "w/s", tab: "tasks", taskId: "task 1" },
      { name: "project", projectId: "p", tab: "tasks" },
      { name: "project", deviceId: "d1", projectId: "p", tab: "tasks" },
      { name: "project", projectId: "a b", tab: "tasks" },
      { name: "project", projectId: "p", tab: "workspaces" },
      { name: "project", deviceId: "d1", projectId: "a b", tab: "workspaces" },
    ]) {
      expect([route, workRoute(hashFromRoute(route))]).toEqual([route, route]);
    }
  });

  it("writes the canonical hashes", () => {
    expect(hashFromRoute({ name: "inbox" })).toBe("#/inbox");
    expect(hashFromRoute({ name: "account", page: "archive" })).toBe("#/account/archive");
    expect(hashFromRoute({ name: "branch", projectId: "p", branch: "main", tab: "files" })).toBe("#/project/p/branch/main/files");
    expect(hashFromRoute({ name: "branch", projectId: "p", branch: "build/x" })).toBe("#/project/p/branch/build%2Fx/changes");
    expect(hashFromRoute({ name: "task", projectId: "p", id: "i-1" })).toBe("#/project/p/task/i-1");
    expect(hashFromRoute({ name: "task", projectId: "p", id: "i-1", stage: "s2" })).toBe("#/project/p/task/i-1/stage/s2");
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
    expect(hashFromRoute({ name: "task", id: "i" })).toBe("#/inbox");
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

// A route names the machine the work is on, in front of the project: the same
// project id lives on every device the account has.
describe("device-bearing routes", () => {
  it("parses and round-trips a branch on a named device", () => {
    const changes = "#/device/d1/project/p1/branch/main/changes";
    expect(routeFromHash(changes)).toEqual({ name: "branch", deviceId: "d1", projectId: "p1", branch: "main", tab: "changes" });
    expect(hashFromRoute(routeFromHash(changes))).toBe(changes);

    const files = "#/device/d%201/project/p1/branch/build%2Fui/files?path=src%2Fapp.js&line=42";
    expect(routeFromHash(files)).toEqual({
      name: "branch", deviceId: "d 1", projectId: "p1", branch: "build/ui", tab: "files", file: "src/app.js", line: 42,
    });
    expect(hashFromRoute(routeFromHash(files))).toBe(files);

    // A hand-typed slash in the branch name still reads as the branch.
    expect(routeFromHash("#/device/d1/project/p1/branch/build/ui/files")).toEqual({
      name: "branch", deviceId: "d1", projectId: "p1", branch: "build/ui", tab: "files",
    });
  });

  it("parses and round-trips a task on a named device, stage and all", () => {
    const task = "#/device/d1/project/p1/task/i-1";
    expect(routeFromHash(task)).toEqual({ name: "task", deviceId: "d1", projectId: "p1", id: "i-1" });
    expect(hashFromRoute(routeFromHash(task))).toBe(task);

    const staged = "#/device/d1/project/p1/task/i-1/stage/s2";
    expect(routeFromHash(staged)).toEqual({ name: "task", deviceId: "d1", projectId: "p1", id: "i-1", stage: "s2" });
    expect(hashFromRoute(routeFromHash(staged))).toBe(staged);
  });

  it("keeps the device's own settings page, which names no project", () => {
    expect(routeFromHash("#/device/d1")).toEqual({ name: "device", id: "d1" });
    expect(routeFromHash("#/device/d1/settings")).toEqual({ name: "device", id: "d1" });
  });

  it("stamps the device onto a legacy project URL that carries one", () => {
    expect(routeFromHash("#/device/d1/project/p1")).toEqual({ name: "project", projectId: "p1", deviceId: "d1", tab: "tasks" });
    expect(routeFromHash("#/device/d1/project/p1/inbox")).toEqual({ name: "inbox", projectId: "p1", deviceId: "d1" });
    expect(routeFromHash("#/device/d1/project/p1/worktree/wt-1")).toEqual({
      name: "resolve", kind: "worktree", projectId: "p1", id: "wt-1", tab: "changes", deviceId: "d1",
    });
  });
});

// Until a device is named, a work URL is a question: which machine's `proj-1`?
// It parks on the same resolve route every legacy URL parks on, carrying the
// route it meant so the answer only has to supply the device.
describe("a work URL with no device parks on a resolve route", () => {
  it("carries the branch it meant, tab and file included", () => {
    expect(routeFromHash("#/project/p1/branch/main/changes")).toEqual({
      name: "resolve", kind: "project", projectId: "p1",
      route: { name: "branch", projectId: "p1", branch: "main", tab: "changes" },
    });
    expect(routeFromHash("#/project/p1/branch/main/files?path=a%2Fb&line=7")).toEqual({
      name: "resolve", kind: "project", projectId: "p1",
      route: { name: "branch", projectId: "p1", branch: "main", tab: "files", file: "a/b", line: 7 },
    });
  });

  it("carries the task it meant", () => {
    expect(routeFromHash("#/project/p1/task/i-1/stage/s2")).toEqual({
      name: "resolve", kind: "project", projectId: "p1",
      route: { name: "task", projectId: "p1", id: "i-1", stage: "s2" },
    });
  });

  // The terminal a `term-<n>` URL named is read where the URL is read, so it
  // has to survive the parking as well as the resolve hop.
  it("keeps the terminal a legacy tab named within reach", () => {
    expect(routeFromHash("#/project/p/branch/main/term-3").term).toBe("term-3");
  });

  it("is what withDeviceOrResolve makes of a route built by hand", () => {
    const branch = { name: "branch", projectId: "p1", branch: "main", tab: "changes" };
    expect(withDeviceOrResolve(branch)).toEqual({ name: "resolve", kind: "project", projectId: "p1", route: branch });
    // Anything that already names a device, names no project, or is not a work
    // surface is already where it belongs.
    const onDevice = { ...branch, deviceId: "d1" };
    expect(withDeviceOrResolve(onDevice)).toBe(onDevice);
    const inbox = { name: "inbox" };
    expect(withDeviceOrResolve(inbox)).toBe(inbox);
    const legacy = { name: "resolve", kind: "run", id: "r" };
    expect(withDeviceOrResolve(legacy)).toBe(legacy);
    const nameless = { name: "branch", branch: "main" };
    expect(withDeviceOrResolve(nameless)).toBe(nameless);
  });

  // A device is never invented: a route that names none is written without one,
  // and reading it back asks the question again.
  it("is written device-less, never with a device made up for it", () => {
    expect(hashFromRoute({ name: "branch", projectId: "p1", branch: "main", tab: "changes" })).toBe("#/project/p1/branch/main/changes");
    expect(hashFromRoute({ name: "task", projectId: "p1", id: "i-1" })).toBe("#/project/p1/task/i-1");
  });
});

// A conversation has a URL: the page the agent belongs to, with the rail
// standing on that agent. It is what a link from a message to "the
// conversation it came from" is written from — the agent rides as a query,
// where the Files tab's file already rides, because the path segments are the
// surface and nothing else.
describe("the conversation a URL names", () => {
  it("round-trips a workspace and a project with and without an agent", () => {
    for (const hash of [
      "#/device/d1/project/p/workspace/ws/changes",
      "#/device/d1/project/p/workspace/ws/changes?agent=ag-1",
      "#/device/d1/project/p/workspace/ws/directory/src/files?path=lib%2Fa.js&line=8&agent=ag-1",
      "#/device/d1/project/p",
      "#/device/d1/project/p?agent=ag-1",
    ]) {
      expect([hash, hashFromRoute(routeFromHash(hash))]).toEqual([hash, hash]);
    }
  });

  it("parses the agent onto the route the page mounts from", () => {
    expect(routeFromHash("#/device/d1/project/p/workspace/ws/changes?agent=ag-1")).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "changes", agent: "ag-1",
    });
    expect(routeFromHash("#/device/d1/project/p?agent=ag-1")).toEqual({
      name: "project", deviceId: "d1", projectId: "p", tab: "tasks", agent: "ag-1",
    });
    // An id with a character a query has to escape survives both ways.
    expect(routeFromHash("#/device/d1/project/p?agent=ag%202").agent).toBe("ag 2");
    // A URL that names no agent says nothing about one.
    expect(routeFromHash("#/device/d1/project/p").agent).toBeUndefined();
    expect(routeFromHash("#/device/d1/project/p?agent=").agent).toBeUndefined();
    // Only the two surfaces with a rail of their own carry one.
    expect(routeFromHash("#/device/d1/project/p/branch/main/changes?agent=ag-1").agent).toBeUndefined();
  });

  // A device-less workspace URL is still a question for the feed, and the
  // conversation it named has to survive the parking.
  it("keeps the agent through the resolve a device-less URL parks on", () => {
    expect(routeFromHash("#/project/p/workspace/ws/changes?agent=ag-1")).toEqual({
      name: "resolve", kind: "project", projectId: "p",
      route: { name: "workspace", projectId: "p", workspaceId: "ws", tab: "changes", agent: "ag-1" },
    });
  });

  it("is what conversationRoute writes, for both kinds of page", () => {
    const workspace = conversationRoute({
      kind: "workspace", projectId: "p", deviceId: "d1", workspaceId: "ws", agentId: "ag-1",
    });
    expect(workspace).toEqual({
      name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "changes", agent: "ag-1",
    });
    expect(hashFromRoute(workspace)).toBe("#/device/d1/project/p/workspace/ws/changes?agent=ag-1");

    const project = conversationRoute({ kind: "project", projectId: "p", deviceId: "d1", agentId: "ag-1" });
    expect(project).toEqual({ name: "project", deviceId: "d1", projectId: "p", agent: "ag-1" });
    expect(hashFromRoute(project)).toBe("#/device/d1/project/p?agent=ag-1");
  });

  // A route is written with what it was given and nothing invented: no device,
  // no agent, and the page still opens.
  it("leaves out the device and the agent it was not given", () => {
    expect(conversationRoute({ kind: "project", projectId: "p" })).toEqual({ name: "project", projectId: "p" });
    expect(conversationRoute({ kind: "workspace", projectId: "p", workspaceId: "ws" })).toEqual({
      name: "workspace", projectId: "p", workspaceId: "ws", tab: "changes",
    });
  });
});

// The tracker's own routes. The singular `task` beside them belongs to the
// retired plan flow and is left exactly as it was: the two are different things
// that happen to share the English word.
describe("the project's two tabs", () => {
  // #46. Tasks is the first tab and the one a project link opens on, so it is
  // the tab that writes nothing: `#/project/p` IS the Tasks tab. Workspaces is
  // second, and names itself because it is the one that is not the default.
  it("opens Tasks on a bare project link, and writes no tab for it", () => {
    expect(workRoute("#/project/p")).toEqual({ name: "project", projectId: "p", tab: "tasks" });
    expect(hashFromRoute({ name: "project", projectId: "p", tab: "tasks" })).toBe("#/project/p");
  });

  it("names the Workspaces tab in the URL", () => {
    expect(workRoute("#/project/p/workspaces")).toEqual({ name: "project", projectId: "p", tab: "workspaces" });
    expect(hashFromRoute({ name: "project", projectId: "p", tab: "workspaces" })).toBe("#/project/p/workspaces");
  });

  // Every link written before the flip says `/tasks`, and they are the same
  // place — so they still open, and settle onto the canonical form.
  it("keeps /tasks working as an alias for the default", () => {
    expect(workRoute("#/project/p/tasks")).toEqual({ name: "project", projectId: "p", tab: "tasks" });
    expect(hashFromRoute(workRoute("#/project/p/tasks"))).toBe("#/project/p");
  });

  it("carries the board on the default URL", () => {
    const board = "#/device/d1/project/p?view=board";
    expect(routeFromHash(board)).toEqual({ name: "project", deviceId: "d1", projectId: "p", tab: "tasks", view: "board" });
    expect(hashFromRoute(routeFromHash(board))).toBe(board);
  });

  it("round-trips both tabs on the machine the project is on", () => {
    for (const hash of ["#/device/d1/project/p", "#/device/d1/project/p/workspaces"]) {
      expect([hash, hashFromRoute(routeFromHash(hash))]).toEqual([hash, hash]);
    }
  });
});

describe("the task tracker under a project", () => {
  it("opens the project page on its Tasks tab", () => {
    expect(workRoute("#/project/p/tasks")).toEqual({ name: "project", projectId: "p", tab: "tasks" });
  });

  it("opens one task's page", () => {
    expect(workRoute("#/project/p/tasks/task-01K5Z")).toEqual({
      name: "trackerTask", projectId: "p", taskId: "task-01K5Z",
    });
  });

  it("uses Dashboard as the project default and gives List and Board stable URLs", () => {
    const board = "#/device/d1/project/p/tasks?view=board";
    expect(routeFromHash(board)).toEqual({ name: "project", deviceId: "d1", projectId: "p", tab: "tasks", view: "board" });
    // …and writes back as the default tab's own URL (#46).
    expect(hashFromRoute(routeFromHash(board))).toBe("#/device/d1/project/p?view=board");
    expect(hashFromRoute({ name: "project", deviceId: "d1", projectId: "p", tab: "tasks", view: "list" }))
      .toBe("#/device/d1/project/p?view=list");
    expect(routeFromHash("#/device/d1/project/p?view=list").view).toBe("list");
    expect(hashFromRoute({ name: "project", deviceId: "d1", projectId: "p", tab: "tasks", view: "dashboard" }))
      .toBe("#/device/d1/project/p");
    expect(routeFromHash("#/device/d1/project/p?view=dashboard").view).toBeUndefined();
  });

  it("keeps List as the workspace Tasks default and links Dashboard explicitly", () => {
    const base = "#/device/d1/project/p/workspace/ws/tasks";
    expect(hashFromRoute({ name: "workspace", deviceId: "d1", projectId: "p", workspaceId: "ws", tab: "tasks", view: "list" })).toBe(base);
    expect(routeFromHash(`${base}?view=dashboard`).view).toBe("dashboard");
    expect(hashFromRoute(routeFromHash(`${base}?view=dashboard`))).toBe(`${base}?view=dashboard`);
  });

  it("round-trips both, on the machine the project is on", () => {
    // The tab's own URL is the bare project link now; one task's page is
    // untouched by the flip.
    for (const hash of ["#/device/d1/project/p", "#/device/d1/project/p/tasks/task-1"]) {
      expect(hashFromRoute(routeFromHash(hash))).toBe(hash);
    }
  });

  it("encodes a task id that carries a separator", () => {
    expect(hashFromRoute({ name: "trackerTask", deviceId: "d1", projectId: "p", taskId: "a/b" }))
      .toBe("#/device/d1/project/p/tasks/a%2Fb");
  });

  // Both machines mint a `proj-1`, so a tracker URL with no device on it is a
  // question — and the answer must still know which task it was about.
  it("parks on the resolve hop when the URL names no machine", () => {
    expect(routeFromHash("#/project/p/tasks/task-1")).toEqual({
      name: "resolve", kind: "project", projectId: "p",
      route: { name: "trackerTask", projectId: "p", taskId: "task-1" },
    });
  });

  // There is no tracker of a branch or of a plan task to open, so the retired
  // cluster tab still means what it meant on those.
  it("leaves the retired Tasks cluster tab alone on every other surface", () => {
    for (const base of ["#/task/r", "#/worktree/p/w", "#/project/p/branch/main", "#/project/p/task/i"]) {
      expect([base, routeFromHash(`${base}/tasks`)]).toEqual([base, { name: "inbox" }]);
    }
  });

  // #46 flipped which tab is the default, so the bare project URL is the
  // Tasks tab now — and a route that names no tab at all still writes it,
  // because "no tab" and "the default" are the same URL.
  it("leaves the default tab writing no tab at all", () => {
    expect(routeFromHash("#/device/d1/project/p")).toEqual({ name: "project", deviceId: "d1", projectId: "p", tab: "tasks" });
    expect(hashFromRoute({ name: "project", deviceId: "d1", projectId: "p" })).toBe("#/device/d1/project/p");
  });

  it("keeps the rail's agent beside the tab", () => {
    const hash = "#/device/d1/project/p/tasks?agent=agent-7&view=board";
    expect(routeFromHash(hash)).toEqual({
      name: "project", deviceId: "d1", projectId: "p", tab: "tasks", agent: "agent-7", view: "board",
    });
    // …and settles onto the default tab's own URL (#46).
    expect(hashFromRoute(routeFromHash(hash))).toBe("#/device/d1/project/p?agent=agent-7&view=board");
  });
});
