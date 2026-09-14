import { describe, it, expect } from "vitest";
import { pickDevice, resolveLegacyRoute } from "../src/core/routeResolve.js";

// One feed's worth of rows in the new items[] shape (bridge board.list).
const items = [
  { kind: "branch", project_id: "p1", branch: "main", primary: true, worktree_id: "wt-main", run_id: null, issue_id: null },
  { kind: "branch", project_id: "p1", branch: "build/login", run_id: "run-1", worktree_id: "wt-1", issue_id: "issue-9" },
  { kind: "branch", project_id: "p2", branch: "detached-head", run_id: "run-2", worktree_id: "wt-2", issue_id: null },
  { kind: "issue", project_id: "p2", branch: null, issue_id: "issue-3", run_id: null, worktree_id: null },
];

describe("resolveLegacyRoute", () => {
  it("resolves a run id to its branch, carrying the canonical tab", () => {
    expect(resolveLegacyRoute({ kind: "run", id: "run-1", tab: "files" }, { items })).toEqual({
      name: "branch", projectId: "p1", branch: "build/login", tab: "files",
    });
    expect(resolveLegacyRoute({ kind: "run", id: "run-2" }, { items })).toEqual({
      name: "branch", projectId: "p2", branch: "detached-head", tab: "changes",
    });
  });

  it("resolves a worktree id to its branch", () => {
    expect(resolveLegacyRoute({ kind: "worktree", id: "wt-1", tab: "changes" }, { items })).toEqual({
      name: "branch", projectId: "p1", branch: "build/login", tab: "changes",
    });
  });

  it("resolves a project's primary checkout to its branch row", () => {
    expect(resolveLegacyRoute({ kind: "primary", projectId: "p1", tab: "files" }, { items })).toEqual({
      name: "branch", projectId: "p1", branch: "main", tab: "files",
    });
  });

  it("resolves an issue id to its issue, keeping the stage deep-link", () => {
    expect(resolveLegacyRoute({ kind: "issue", id: "issue-3" }, { items })).toEqual({
      name: "issue", projectId: "p2", id: "issue-3",
    });
    expect(resolveLegacyRoute({ kind: "issue", id: "issue-3", stage: "s2" }, { items })).toEqual({
      name: "issue", projectId: "p2", id: "issue-3", stage: "s2",
    });
  });

  // Dedup: an issue whose implementation is in flight has no row of its own —
  // its branch row carries the issue id. The branch IS the nearest surface.
  it("resolves an issue being implemented to the branch that carries it", () => {
    expect(resolveLegacyRoute({ kind: "issue", id: "issue-9", stage: "s1" }, { items })).toEqual({
      name: "branch", projectId: "p1", branch: "build/login", tab: "changes",
    });
  });

  it("prefers the row in the project the URL named", () => {
    const ambiguous = [
      { kind: "branch", project_id: "p2", branch: "same-name", worktree_id: "wt-x" },
      { kind: "branch", project_id: "p1", branch: "the-one", worktree_id: "wt-x" },
    ];
    expect(resolveLegacyRoute({ kind: "worktree", projectId: "p1", id: "wt-x" }, { items: ambiguous })).toEqual({
      name: "branch", projectId: "p1", branch: "the-one", tab: "changes",
    });
  });

  it("answers null when nothing in the feed carries that id", () => {
    expect(resolveLegacyRoute({ kind: "run", id: "gone" }, { items })).toBeNull();
    expect(resolveLegacyRoute({ kind: "primary", projectId: "p9" }, { items })).toBeNull();
    expect(resolveLegacyRoute({ kind: "issue", id: "nope" }, { items })).toBeNull();
    expect(resolveLegacyRoute({ kind: "run", id: "run-1" }, { items: [] })).toBeNull();
    expect(resolveLegacyRoute({ kind: "run", id: "run-1" }, { items: undefined })).toBeNull();
    expect(resolveLegacyRoute({ kind: "run", id: "run-1" }, {})).toBeNull();
  });

  // A row with no branch name cannot address a branch URL; the inbox is where
  // an unaddressable entity belongs.
  it("answers null for a row that has no branch to open", () => {
    const nameless = [{ kind: "branch", project_id: "p1", branch: null, run_id: "run-7" }];
    expect(resolveLegacyRoute({ kind: "run", id: "run-7" }, { items: nameless })).toBeNull();
  });

  it("answers null for a ref it has no rule for", () => {
    expect(resolveLegacyRoute({ kind: "mystery", id: "x" }, { items })).toBeNull();
    expect(resolveLegacyRoute(null, { items })).toBeNull();
  });
});

// Every device mints a `proj-1`, so a legacy id can name work on more than one
// machine at once. Which one the reader meant is a policy, not a lookup: the
// home device first, then whichever device the account lists first.
describe("pickDevice", () => {
  const onA = { deviceId: "dev-a", branch: "a" };
  const onB = { deviceId: "dev-b", branch: "b" };

  it("prefers the candidate on the home device", () => {
    expect(pickDevice([onB, onA], { homeDeviceId: "dev-a", deviceOrder: ["dev-b", "dev-a"] })).toBe(onA);
  });

  it("falls back to the first online device in the account's order", () => {
    expect(pickDevice([onB, onA], { homeDeviceId: null, deviceOrder: ["dev-b", "dev-a"] })).toBe(onB);
    expect(pickDevice([onB, onA], { homeDeviceId: null, deviceOrder: ["dev-a", "dev-b"] })).toBe(onA);
    // A home device with nothing among the candidates says nothing about them.
    expect(pickDevice([onB, onA], { homeDeviceId: "dev-z", deviceOrder: ["dev-a"] })).toBe(onA);
  });

  it("falls back to the first candidate when no policy names any of them", () => {
    expect(pickDevice([onB, onA], { homeDeviceId: null, deviceOrder: [] })).toBe(onB);
    expect(pickDevice([onB, onA], {})).toBe(onB);
    expect(pickDevice([onB, onA])).toBe(onB);
  });

  it("answers null when there is nothing to pick", () => {
    expect(pickDevice([], { homeDeviceId: "dev-a", deviceOrder: ["dev-a"] })).toBeNull();
    expect(pickDevice(null, {})).toBeNull();
  });
});

describe("resolveLegacyRoute across devices", () => {
  const primaryOn = (deviceId, branch) => ({
    kind: "branch", project_id: "proj-1", branch, primary: true, worktree_id: `wt-${deviceId}`, deviceId,
  });
  const collision = { items: [primaryOn("dev-b", "their-main"), primaryOn("dev-a", "main")] };
  const policy = { homeDeviceId: "dev-a", deviceOrder: ["dev-b", "dev-a"] };

  it("picks the home device's copy when a project id collides", () => {
    expect(resolveLegacyRoute({ kind: "primary", projectId: "proj-1" }, collision, policy)).toEqual({
      name: "branch", deviceId: "dev-a", projectId: "proj-1", branch: "main", tab: "changes",
    });
  });

  it("picks the first online device when there is no home device", () => {
    expect(resolveLegacyRoute({ kind: "primary", projectId: "proj-1" }, collision, { homeDeviceId: null, deviceOrder: ["dev-b", "dev-a"] })).toEqual({
      name: "branch", deviceId: "dev-b", projectId: "proj-1", branch: "their-main", tab: "changes",
    });
  });

  it("ignores the policy when only one device carries the id", () => {
    const only = { items: [primaryOn("dev-b", "their-main")] };
    expect(resolveLegacyRoute({ kind: "primary", projectId: "proj-1" }, only, policy)).toEqual({
      name: "branch", deviceId: "dev-b", projectId: "proj-1", branch: "their-main", tab: "changes",
    });
  });

  // A URL that names a machine is not asking which one: `#/device/<d>/project/
  // <p>` says it outright, and the home device's `proj-1` is not what it meant.
  it("opens the device the URL named rather than asking the policy", () => {
    expect(resolveLegacyRoute({ kind: "primary", projectId: "proj-1", deviceId: "dev-b" }, collision, policy)).toEqual({
      name: "branch", deviceId: "dev-b", projectId: "proj-1", branch: "their-main", tab: "changes",
    });
  });

  it("answers nothing when the device the URL named carries no such row", () => {
    expect(resolveLegacyRoute({ kind: "primary", projectId: "proj-1", deviceId: "dev-z" }, collision, policy)).toBeNull();
  });

  it("carries the device onto a resolved issue too", () => {
    const issues = { items: [{ kind: "issue", project_id: "proj-1", issue_id: "i-1", branch: null, deviceId: "dev-b" }] };
    expect(resolveLegacyRoute({ kind: "issue", id: "i-1", stage: "s2" }, issues, policy)).toEqual({
      name: "issue", deviceId: "dev-b", projectId: "proj-1", id: "i-1", stage: "s2",
    });
  });
});

// A work URL with no device in it asks which machine holds that project. The
// answer is the same policy, read off the whole account's feed — and a plain
// folder has no work row at all, so the projects are searched beside the rows.
describe("resolveLegacyRoute for a project with no device named", () => {
  const branchRoute = { name: "branch", projectId: "proj-1", branch: "main", tab: "changes" };
  const ref = { name: "resolve", kind: "project", projectId: "proj-1", route: branchRoute };
  const policy = { homeDeviceId: "dev-a", deviceOrder: ["dev-b", "dev-a"] };

  it("stamps the picked device onto the route the URL meant", () => {
    const feed = {
      items: [
        { kind: "branch", project_id: "proj-1", branch: "their-main", deviceId: "dev-b" },
        { kind: "branch", project_id: "proj-1", branch: "main", deviceId: "dev-a" },
      ],
      projects: [],
    };
    expect(resolveLegacyRoute(ref, feed, policy)).toEqual({ ...branchRoute, deviceId: "dev-a" });
    expect(resolveLegacyRoute(ref, feed, { homeDeviceId: null, deviceOrder: ["dev-b", "dev-a"] })).toEqual({
      ...branchRoute, deviceId: "dev-b",
    });
  });

  it("finds a plain folder through the projects when no row carries it", () => {
    const feed = { items: [], projects: [{ id: "proj-1", name: "notes", deviceId: "dev-b" }] };
    expect(resolveLegacyRoute(ref, feed, policy)).toEqual({ ...branchRoute, deviceId: "dev-b" });
    // project.list rows name the project the wire's way; both halves read.
    const wire = { items: [], projects: [{ project_id: "proj-1", deviceId: "dev-a" }] };
    expect(resolveLegacyRoute(ref, wire, policy)).toEqual({ ...branchRoute, deviceId: "dev-a" });
  });

  it("answers null when no device carries that project at all", () => {
    expect(resolveLegacyRoute(ref, { items: [], projects: [] }, policy)).toBeNull();
  });
});
