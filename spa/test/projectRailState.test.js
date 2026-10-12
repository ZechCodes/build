// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { rememberProjectRailRoute, projectReturnRoute } = await import("../src/core/projectRailState.js");
const { projectRoute } = await import("../src/core/projectModel.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");
const { projectReturnTarget, navigationSelectionTarget } = await import("../src/core/navigationRoutes.js");

beforeEach(async () => { await wipeUiRecords(); });

describe("returning to a project's rail", () => {
  it("uses the saved return for ancestors and local rows, while global selection opens Tasks", async () => {
    const project = { id: "p", deviceId: "d" };
    const saved = { name: "project", deviceId: "d", projectId: "p", tab: "files", sourceId: "docs", file: "a.md", line: 5 };
    await rememberProjectRailRoute(saved);
    const route = { name: "workspace", deviceId: "d", projectId: "p", workspaceId: "w", tab: "tasks", taskId: "t" };
    const destination = { kind: "project", key: "d/p", deviceId: "d", projectId: "p", route: projectRoute(project) };
    expect((await navigationSelectionTarget(destination, { route })).route).toEqual(projectRoute(project));
    expect((await navigationSelectionTarget({ ...destination, returnToProject: true }, { route })).route).toEqual(saved);
    expect((await projectReturnTarget(project, { route })).route).toEqual(saved);
    const tracker = { name: "trackerTask", deviceId: "d", projectId: "p", taskId: "t", commentId: "tc" };
    expect((await navigationSelectionTarget(destination, { route: tracker })).route).toBe(tracker);
    expect((await navigationSelectionTarget({ ...destination, returnToProject: true }, { route: tracker })).route).toBe(tracker);
    expect((await projectReturnTarget(project, { route: tracker })).route).toEqual(saved);
  });

  it("keeps a project self-link exact and does not invent a project Changes return", async () => {
    const project = { id: "p", deviceId: "d" };
    const route = { name: "project", deviceId: "d", projectId: "p", tab: "files", file: "a/b", line: 2, agent: "a" };
    const currentHash = "#/device/d/project/p/files?path=a%2fb&line=2&agent=a";
    expect(await projectReturnTarget(project, { route, currentHash })).toEqual({ current: true, route, hash: currentHash });
    await rememberProjectRailRoute({ ...route, tab: "changes" });
    expect(await projectReturnRoute(project)).toEqual({ name: "project", deviceId: "d", projectId: "p", tab: "tasks" });
  });
  it("restores the Files face and source without changing ordinary project links", async () => {
    const project = { id: "p", deviceId: "d" };
    const files = { name: "project", deviceId: "d", projectId: "p", tab: "files", sourceId: "docs", file: "README.md", line: 4 };
    await rememberProjectRailRoute(files);
    expect(await projectReturnRoute(project)).toEqual(files);
    expect(projectRoute(project)).toEqual({ name: "project", deviceId: "d", projectId: "p" });
  });

  it("isolates identical project IDs on different devices and keeps the default when never visited", async () => {
    await rememberProjectRailRoute({ name: "project", deviceId: "one", projectId: "p", tab: "files", sourceId: "docs" });
    expect(await projectReturnRoute({ id: "p", deviceId: "two" })).toEqual({ name: "project", deviceId: "two", projectId: "p" });
    expect(await projectReturnRoute({ id: "other", deviceId: "one" })).toEqual({ name: "project", deviceId: "one", projectId: "other" });
  });

  it("remembers a later Tasks face and its layout without retaining a Files location", async () => {
    await rememberProjectRailRoute({ name: "project", deviceId: "d", projectId: "p", tab: "files", sourceId: "docs", file: "a.md" });
    await rememberProjectRailRoute({ name: "project", deviceId: "d", projectId: "p", tab: "tasks", view: "board", sourceId: "docs", file: "a.md" });
    expect(await projectReturnRoute({ id: "p", deviceId: "d" })).toEqual({ name: "project", deviceId: "d", projectId: "p", tab: "tasks", view: "board" });
  });
});
