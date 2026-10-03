// @vitest-environment jsdom
import { beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { rememberProjectRailRoute, projectReturnRoute } = await import("../src/core/projectRailState.js");
const { projectRoute } = await import("../src/core/projectModel.js");
const { wipeUiRecords } = await import("../src/core/localUiStore.js");

beforeEach(async () => { await wipeUiRecords(); });

describe("returning to a project's rail", () => {
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
