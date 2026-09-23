// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const address = (kind) => ({ deviceId: "dev-1", entityId: "", kind });
const old = { session_started_ms: 100, last_activity_ms: 200 };
const fresh = { session_started_ms: 50_000_000, last_activity_ms: 50_000_000 };
const row = (kind, id, session = old) => ({
  [kind === "projects" ? "project_id" : "workspace_id"]: id,
  name: id,
  ...session,
});

let cache;
let lists;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  lists = await import("../src/core/sessionListCache.js");
});

const read = async (kind) => (await cache.readCached(address(kind)))?.value;

describe("all session list writers", () => {
  it.each([
    ["project list", "projects", "replace"],
    ["workspace list", "workspaces", "replace"],
    ["project tip", "projects", "tip"],
    ["workspace tip", "workspaces", "tip"],
    ["workspace create or result", "workspaces", "upsert"],
  ])("keeps a newer summary through %s and preserves unrelated rows", async (_label, kind, operation) => {
    await cache.writeCached(address(kind), [row(kind, "A", fresh), row(kind, "B", old)]);
    if (operation === "replace") {
      await lists.replaceSessionList(address(kind), kind, [row(kind, "A"), row(kind, "B")]);
    } else if (operation === "tip") {
      await lists.updateSessionSummary(address(kind), kind, "A", old);
    } else {
      await lists.upsertSessionRow(address(kind), kind, row(kind, "A"));
    }
    expect(await read(kind)).toEqual([row(kind, "A", fresh), row(kind, "B", old)]);
  });

  it("serializes two tab writers through IndexedDB instead of a tab-local queue", async () => {
    await cache.writeCached(address("workspaces"), [row("workspaces", "A", old)]);
    // Resetting modules gives the second writer its own module state and cache
    // connection, as another browser tab has, while sharing this IndexedDB.
    vi.resetModules();
    const otherTab = await import("../src/core/sessionListCache.js");
    const writes = [
      lists.replaceSessionList(address("workspaces"), "workspaces", [row("workspaces", "A", old)]),
      otherTab.updateSessionSummary(address("workspaces"), "workspaces", "A", fresh),
    ];
    await Promise.all(writes);
    expect((await read("workspaces"))[0]).toMatchObject(fresh);
    await Promise.all([
      lists.upsertSessionRow(address("workspaces"), "workspaces", row("workspaces", "B", old)),
      otherTab.replaceSessionList(address("workspaces"), "workspaces", [row("workspaces", "A", old), row("workspaces", "B", old)]),
    ]);
    expect((await read("workspaces")).find((entry) => entry.workspace_id === "A")).toMatchObject(fresh);
  });

  it("the real create flow keeps another workspace and its newer summary", async () => {
    const { cacheCreatedWorkspace } = await import("../src/core/createWork.js");
    await cache.writeCached(address("workspaces"), [row("workspaces", "A", fresh)]);
    await cacheCreatedWorkspace("dev-1", {
      workspace_id: "B", project_id: "project-1", ...old,
    });
    expect(await read("workspaces")).toMatchObject([
      row("workspaces", "A", fresh),
      { workspace_id: "B", ...old },
    ]);
    await cacheCreatedWorkspace("dev-1", {
      workspace_id: "A", project_id: "project-1", ...old,
    });
    expect((await read("workspaces"))[0]).toMatchObject(fresh);
  });
});
