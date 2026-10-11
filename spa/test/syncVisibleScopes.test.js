// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { registerSyncTree, registerSyncDiff, visibleTreePaths, visibleWorkingDiff } = await import("../src/core/syncVisibleScopes.js");
const { mountFileTree } = await import("../src/core/fileTree.js");
const { createReviewPlug } = await import("../src/core/changesReview.js");
const { scopeFor } = await import("../src/core/cacheScope.js");
const { wipeCache, writeCached } = await import("../src/core/localCache.js");
const { writeUiRecord, wipeUiRecords } = await import("../src/core/localUiStore.js");

const address = (kind = "tree", entityId = "run-1") => ({ deviceId: "dev-1", entityId, kind });
let disposers = [];
beforeEach(async () => {
  await wipeCache();
  await wipeUiRecords();
  document.body.innerHTML = "";
});
afterEach(() => {
  disposers.forEach((dispose) => dispose());
  disposers = [];
  document.body.innerHTML = "";
});

describe("mounted sync scopes", () => {
  it("unions live tree paths, follows changed scopes, and removes disposed mounts", () => {
    let paths = ["", "src"];
    let active = true;
    let entityId = "run-1";
    const first = registerSyncTree(() => address("tree", entityId), () => paths, () => active);
    const second = registerSyncTree(address(), () => ["", "test"]);
    disposers.push(first, second);
    expect(visibleTreePaths("dev-1", "run-1")).toEqual(["", "src", "test"]);
    expect(visibleTreePaths("dev-2", "run-1")).toEqual([]);
    paths = ["", "src/core"];
    entityId = "run-2";
    expect(visibleTreePaths("dev-1", "run-1")).toEqual(["", "test"]);
    expect(visibleTreePaths("dev-1", "run-2")).toEqual(paths);
    active = false;
    expect(visibleTreePaths("dev-1", "run-2")).toEqual([]);
    second();
    expect(visibleTreePaths("dev-1", "run-1")).toEqual([]);
  });

  it("only promotes a working diff and ignores missing cache addresses", () => {
    disposers.push(registerSyncDiff(null), registerSyncDiff(address("commit")), registerSyncDiff(address("unpushed")));
    expect(visibleWorkingDiff("dev-1", "run-1")).toBe(false);
    let active = true;
    disposers.push(registerSyncDiff(address("diff"), () => active));
    expect(visibleWorkingDiff("dev-1", "run-1")).toBe(true);
    active = false;
    expect(visibleWorkingDiff("dev-1", "run-1")).toBe(false);
  });
});

describe("production mounts", () => {
  it("tracks only reachable expanded tree directories while the tree is shown", async () => {
    const stateAddress = { ...address("ui-expanded"), sub: "tree" };
    await writeUiRecord(stateAddress, { expanded: ["src", "src/core", "closed/deep"] });
    const host = document.createElement("div");
    const list = document.createElement("div");
    host.append(list);
    document.body.append(host);
    const tree = mountFileTree(list, {
      listingAddress: (sub) => ({ ...address(), sub }), stateAddress,
      readsForItself: () => false, listDirectory: async () => ({ entries: [] }),
      finePointer: () => true, onOpen: () => {},
    });
    disposers.push(tree.dispose);
    await tree.ready;
    expect(visibleTreePaths("dev-1", "run-1")).toEqual(["", "src", "src/core"]);
    host.hidden = true;
    expect(visibleTreePaths("dev-1", "run-1")).toEqual([]);
    host.hidden = false;
    host.style.display = "none";
    expect(visibleTreePaths("dev-1", "run-1")).toEqual([]);
    host.style.display = "";
    list.remove();
    expect(visibleTreePaths("dev-1", "run-1")).toEqual([]);
    host.append(list);
    tree.dispose();
    expect(visibleTreePaths("dev-1", "run-1")).toEqual([]);
  });

  it("tracks a mounted Changes plug's visibility, adopted entity, and unmount", async () => {
    await writeCached(address("diff"), { patch: "", commentable: false });
    let entityId = "run-1";
    const host = document.createElement("div");
    document.body.append(host);
    const plug = createReviewPlug({ cacheScope: scopeFor("dev-1"), cacheEntity: () => entityId, fetchDiff: async () => null });
    disposers.push(plug.unmount);
    plug.mount(host);
    expect(visibleWorkingDiff("dev-1", "run-1")).toBe(true);
    plug.setVisible(false);
    expect(visibleWorkingDiff("dev-1", "run-1")).toBe(false);
    plug.setVisible(true);
    entityId = "run-2";
    expect(visibleWorkingDiff("dev-1", "run-1")).toBe(false);
    expect(visibleWorkingDiff("dev-1", "run-2")).toBe(true);
    const replacement = document.createElement("div");
    plug.mount(replacement);
    expect(visibleWorkingDiff("dev-1", "run-2")).toBe(false);
    document.body.append(replacement);
    expect(visibleWorkingDiff("dev-1", "run-2")).toBe(true);
    plug.unmount();
    expect(visibleWorkingDiff("dev-1", "run-2")).toBe(false);
  });
});
