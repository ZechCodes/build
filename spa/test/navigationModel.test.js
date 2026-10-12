// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { stampProject, stampWorkspace, mergeFeeds } from "../src/core/feedMerge.js";
import { navigationCatalog, navigationPickerModel, projectWorkspaceInventory } from "../src/core/navigationModel.js";
import { navigationDestinationOf, destinationTarget } from "../src/core/navigationRoutes.js";

const project = (deviceId, over = {}) => stampProject({ id: "proj-1", name: "Build", path: "/src", ...over }, deviceId);
const workspace = (deviceId, over = {}) => stampWorkspace({ id: "ws-1", project_id: "proj-1", name: "Navigation / 🦊",
  status: "ready", root: "/work/navigation", directories: [{ id: "checkout", source_id: "code", is_git: true }], ...over }, deviceId);
const devices = [{ id: "one", name: "Desktop" }, { id: "two", name: "Laptop" }];
const roster = { projects: { present: true, observedAt: 1 }, workspaces: { present: true, observedAt: 2 } };
const feed = (projects = [project("one")], workspaces = [workspace("one")]) => ({ projects, workspaces, listKnowledge: roster });

describe("the shared destination catalog", () => {
  it("duplicate_device_ids", () => {
    const views = new Map(devices.map(({ id }) => [id, feed([project(id)], [workspace(id)])]));
    const catalog = navigationCatalog(mergeFeeds(views), { devices, route: {
      name: "workspace", deviceId: "two", projectId: "proj-1", workspaceId: "ws-1", tab: "tasks", taskId: "task-1",
    } });
    expect(catalog.projects.map((group) => group.key)).toEqual(["one/proj-1", "two/proj-1"]);
    expect(catalog.destinations.filter((row) => row.kind === "workspace").map((row) => [row.key, row.projectKey, row.current]))
      .toEqual([["one/ws-1", "one/proj-1", false], ["two/ws-1", "two/proj-1", true]]);
    const searched = navigationPickerModel(catalog, { query: "Laptop" });
    expect(searched.groups.map((group) => group.key)).toEqual(["two/proj-1"]);
    expect(searched.groups[0].destinations.map((row) => row.key)).toEqual(["two/proj-1", "two/ws-1"]);
  });

  it("current_destination_preserves_url", () => {
    const currentHash = "#/device/one/project/proj-1/workspace/ws-1/directory/code/files?agent=agent-7&path=a%2fb&line=4";
    const route = { name: "workspace", deviceId: "one", projectId: "proj-1", workspaceId: "ws-1", sourceId: "code",
      tab: "files", file: "a/b", line: 4, agent: "agent-7" };
    const catalog = navigationCatalog(feed(), { route, currentHash });
    const current = catalog.destinations.find((row) => row.kind === "workspace");
    expect(current).toMatchObject({ current: true, hash: currentHash });
    expect(current.route).toBe(route);
    expect(destinationTarget(current, { route, currentHash })).toEqual({ current: true, route, hash: currentHash });
    const projectRow = catalog.projects[0].destination;
    expect(projectRow.current).toBe(false);
    expect(projectRow.route).toEqual({ name: "project", deviceId: "one", projectId: "proj-1" });
  });

  it("uses URL hosts for task destinations and never conversation ownership", () => {
    expect(navigationDestinationOf({ name: "trackerTask", deviceId: "one", projectId: "proj-1", taskId: "task" }))
      .toMatchObject({ kind: "project", key: "one/proj-1" });
    expect(navigationDestinationOf({ name: "workspace", deviceId: "two", projectId: "proj-1", workspaceId: "ws-1", tab: "tasks" }))
      .toMatchObject({ kind: "workspace", key: "two/ws-1", projectKey: "two/proj-1" });
    for (const name of ["branch", "task", "inbox", "account", "resolve"]) {
      expect(navigationDestinationOf({ name, deviceId: "one", projectId: "proj-1" })).toBeNull();
    }
    const route = { name: "trackerTask", deviceId: "one", projectId: "proj-1", taskId: "task", commentId: "tc" };
    expect(navigationCatalog(feed(), { route }).projects[0].destination.route).toBe(route);
  });

  it("lists unwatched, provisioning, failed and adopted destinations, excluding finished and original-source children", () => {
    const workspaces = [workspace("one", { id: "unwatched", created_by_agent: true, entity_id: "run" }),
      workspace("one", { id: "provisioning", status: "provisioning", directories: [] }),
      workspace("one", { id: "failed", status: "failed", directories: [] }),
      workspace("one", { id: "adopted", managed: false }), workspace("one", { id: "finished", status: "finished" }),
      workspace("one", { id: "original", root: "/src" })];
    const catalog = navigationCatalog({ ...feed([project("one")], workspaces), runs: [{ projectKey: "one/proj-1", entity_id: "run",
      agents: [{ id: "agent", watched: false, unread_count: 8 }] }] });
    expect(catalog.projects[0].workspaces.map((row) => row.workspaceId).sort()).toEqual(["adopted", "failed", "provisioning", "unwatched"]);
    expect(navigationPickerModel(catalog, { query: "Navigation" }).groups[0].destinations).toHaveLength(4);
    expect(projectWorkspaceInventory(catalog, "one/proj-1").workspaces).toHaveLength(4);
    expect(catalog.projects[0].workspaces.find((row) => row.workspaceId === "unwatched").unreadCount).toBe(0);
  });

  it("uses stable alphabetical workspace order, with the current child first locally", () => {
    const workspaces = [workspace("one", { id: "z", name: "Zulu", status: "ready" }),
      workspace("one", { id: "b", name: "Alpha", status: "provisioning" }),
      workspace("one", { id: "a", name: "Alpha", status: "failed" })];
    const route = { name: "workspace", deviceId: "one", projectId: "proj-1", workspaceId: "z" };
    const catalog = navigationCatalog(feed([project("one")], workspaces), { route });
    expect(catalog.projects[0].workspaces.map((row) => row.key)).toEqual(["one/a", "one/b", "one/z"]);
    const local = navigationPickerModel(catalog, { projectKey: "one/proj-1" });
    expect(local.groups[0].destinations.map((row) => row.key)).toEqual(["one/proj-1", "one/z", "one/a", "one/b"]);
    expect(local.groups[0].destinations[0].returnToProject).toBe(true);
    expect(catalog.projects[0].destination.returnToProject).toBeUndefined();
  });

  it("searches project/workspace/device context and retains the project for child matches", () => {
    const catalog = navigationCatalog(feed([project("one")], [workspace("one"), workspace("one", { id: "release", name: "Release" })]), { devices });
    expect(navigationPickerModel(catalog, { query: "Build" }).groups[0].destinations).toHaveLength(3);
    expect(navigationPickerModel(catalog, { query: "Build Navigation" }).groups[0].destinations.map((row) => row.key)).toEqual(["one/ws-1"]);
    expect(navigationPickerModel(catalog, { query: "🦊" }).groups[0].name).toBe("Build");
    expect(navigationPickerModel(catalog, { query: "does not exist" }).groups).toEqual([]);
    expect(navigationPickerModel(catalog, { deviceId: "two" }).groups).toEqual([]);
    expect(navigationPickerModel(catalog).groups).toHaveLength(1);
  });

  it("uses device IDs and project IDs when labels are unavailable, without aliasing by name", () => {
    const catalog = navigationCatalog(feed([], [workspace("unknown", { project_id: "missing" })]));
    expect(catalog.projects[0]).toMatchObject({ key: "unknown/missing", name: "missing", deviceName: "unknown" });
    expect(navigationPickerModel(catalog, { query: "unknown" }).groups).toHaveLength(1);
  });

  it("uses the existing Changes/Files factory without carrying the previous destination's context", () => {
    const catalog = navigationCatalog(feed([project("one")], [workspace("one", { directories: [
      { id: "assets", source_id: "docs", is_git: false }, { id: "repo", source_id: "api", is_git: true },
    ] }), workspace("one", { id: "plain", directories: [{ id: "assets", source_id: "docs", is_git: false }] })]));
    expect(catalog.projects[0].workspaces.find((row) => row.workspaceId === "ws-1").route)
      .toEqual({ name: "workspace", deviceId: "one", projectId: "proj-1", workspaceId: "ws-1", sourceId: "api", tab: "changes" });
    expect(catalog.projects[0].workspaces.find((row) => row.workspaceId === "plain").route.tab).toBe("files");
  });
});

describe("cache-backed roster knowledge", () => {
  let cachedFeedView, readCached, writeCached;
  const address = (kind) => ({ deviceId: "one", entityId: "", kind });
  beforeEach(async () => {
    vi.resetModules();
    globalThis.indexedDB = new IDBFactory();
    globalThis.IDBKeyRange = IDBKeyRange;
    ({ cachedFeedView } = await import("../src/core/cachedRows.js"));
    ({ readCached, writeCached } = await import("../src/core/localCache.js"));
    await writeCached(address("projects"), [project("one")]);
  });

  it("unknown_roster_then_empty", async () => {
    const unknown = navigationCatalog(await cachedFeedView("one"));
    expect(projectWorkspaceInventory(unknown, "one/proj-1")).toMatchObject({ present: false, observedAt: null, workspaces: [] });
    await writeCached(address("workspaces"), []);
    const record = await readCached(address("workspaces"));
    const empty = navigationCatalog(await cachedFeedView("one"));
    expect(projectWorkspaceInventory(empty, "one/proj-1")).toEqual({ present: true, observedAt: record.at, workspaces: [] });
    expect(navigationPickerModel(empty, { projectKey: "one/proj-1" }).roster.present).toBe(true);
  });

  it("failed_refresh_retains_rows", async () => {
    const { readSyncLists } = await import("../src/core/syncLists.js");
    const context = { deviceId: "one", active: () => true };
    let listed = [workspace("one")];
    const rpc = vi.fn(async (method) => {
      if (method === "workspace.list") {
        if (listed === null) throw new Error("refresh failed");
        return { workspaces: listed };
      }
      return method === "project.list" ? { projects: [project("one")] } : { items: [] };
    });
    const refresh = () => readSyncLists(context, {
      // The ordinary sync asks safely: a refused RPC has no list to land.
      ask: (_context, method) => rpc(method).catch(() => null),
      landBoard: async () => {},
      landList: (kind, rows) => writeCached(address(kind), rows),
    });
    await refresh();
    const before = navigationCatalog(await cachedFeedView("one"));
    listed = null;
    await refresh();
    const after = navigationCatalog(await cachedFeedView("one"));
    expect(projectWorkspaceInventory(after, "one/proj-1")).toEqual(projectWorkspaceInventory(before, "one/proj-1"));
    expect(after.destinations.map((row) => row.key)).toEqual(["one/proj-1", "one/ws-1"]);
    listed = [];
    await refresh();
    expect(projectWorkspaceInventory(navigationCatalog(await cachedFeedView("one")), "one/proj-1"))
      .toMatchObject({ present: true, workspaces: [] });
  });
});
