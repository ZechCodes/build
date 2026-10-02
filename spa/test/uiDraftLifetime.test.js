import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

let cache, store, lifetime;
const deviceId = "dev-1";
const workspace = (over = {}) => ({ id: "ws-1", entity_id: "run-1", conversations: [{ conversation_id: "conv-1" }], ...over });
const lists = (workspaces = [workspace()], projects = []) => ({ workspaces, projects, items: [], runs: [] });
const draft = (entityId, sub, device = deviceId) => ({ deviceId: device, entityId, kind: "ui-draft", sub });
const chat = (conversationId = "conv-1", parent = "run-1") => draft(conversationId, `chat:agent:${parent}:agent-1`);
const settings = draft("ws-1", "workspace-settings:");
const directory = draft('workspace:["ws-1","src-1"]', 'workspace:["ws-1","src-1"]');
const commit = draft("run-1", "run-1");
const value = { body: "unsent words", attachments: [{ path: "draft.png" }] };
const seed = async (addresses) => {
  for (const address of addresses) await store.writeUiRecord(address, value);
};
const expectKept = async (addresses) => {
  for (const address of addresses) expect((await store.readUiRecord(address))?.value).toEqual(value);
};
const expectGone = async (addresses) => {
  for (const address of addresses) expect(await store.readUiRecord(address)).toBeUndefined();
};
const remember = async (view = lists()) => {
  for (const kind of ["workspaces", "projects"]) {
    await cache.writeCached({ deviceId, entityId: "", kind }, view[kind]);
  }
};
const reconcile = async (view, active = () => true) => {
  const pass = await lifetime.prepareDraftPrune(deviceId);
  try { await pass.prune(view, active); } finally { pass.dispose(); }
};

beforeEach(async () => {
  vi.resetModules();
  vi.restoreAllMocks();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  store = await import("../src/core/localUiStore.js");
  lifetime = await import("../src/core/uiDraftLifetime.js");
});

describe("draft owners, independent of replica lifetime", () => {
  it("drops a deleted workspace's settings, directory, commit and conversation drafts", async () => {
    const addresses = [settings, directory, commit, chat(), draft("run-1", "changes:inline-comments")];
    await remember();
    await seed(addresses);
    await reconcile(lists([]));
    await expectGone(addresses);
  });

  it("drops only the deleted conversation of a workspace that is still live", async () => {
    const surviving = chat("conv-2");
    await remember();
    await seed([chat(), surviving, settings, commit]);
    await reconcile(lists([workspace({ conversations: [{ conversation_id: "conv-2" }] })]));
    await expectGone([chat()]);
    await expectKept([surviving, settings, commit]);
  });

  it("keeps arbitrarily old drafts for finished, quiet and unwatched live owners after replica eviction", async () => {
    const addresses = [settings, directory, commit, chat()];
    const clock = vi.spyOn(Date, "now").mockReturnValue(1);
    await seed(addresses);
    clock.mockRestore();
    await cache.evictEntity(deviceId, "run-1");
    await reconcile(lists([workspace({ status: "finished" })]));
    await expectKept(addresses);
  });

  it("cleans identifiable workspace drafts even when only build-ui survived a replica rebuild", async () => {
    await seed([settings, directory]);
    await reconcile(lists([]));
    await expectGone([settings, directory]);
  });

  it("keeps a conversation still named by another owner, including a hidden run", async () => {
    await remember();
    await seed([chat()]);
    const view = lists([]);
    view.runs = [{ run_id: "run-2", agents: [{ id: "agent-2", conversation_id: "conv-1" }] }];
    await reconcile(view);
    await expectKept([chat()]);
  });

  it("cleans a project's removed conversation without taking project or task drafts", async () => {
    const project = { id: "p1", entity_id: "run-proj", conversations: [{ conversation_id: "conv-proj" }] };
    const gone = chat("conv-proj", "run-proj");
    const kept = [draft("p1", "project-settings:"), draft("task-1", "tracker-task:p1")];
    await remember(lists([], [project]));
    await seed([gone, ...kept]);
    await reconcile(lists([], [{ ...project, conversations: [] }]));
    await expectGone([gone]);
    await expectKept(kept);
  });

  it.each([undefined, null, {}, [{ unexpected: "shape" }]])("keeps drafts when the workspace list is incomplete: %j", async (workspaces) => {
    await remember();
    const addresses = [settings, directory, commit, chat()];
    await seed(addresses);
    await reconcile({ ...lists(), workspaces });
    await expectKept(addresses);
  });

  it.each([undefined, null, {}, [{ unexpected: "shape" }]])("keeps conversations when the roster is incomplete: %j", async (conversations) => {
    await remember();
    await seed([chat()]);
    await reconcile(lists([workspace({ conversations })]));
    await expectKept([chat()]);
  });

  it("preserves unknown, provisional, global, other-device and non-draft records", async () => {
    const addresses = [
      chat("conv-unknown", "run-archived"), draft("conv-1", "chat:draft:provisional"),
      draft("", "compose:"), draft("ws-1", "workspace-settings:", "dev-2"),
      draft('project:["p1"]', "project:p1"), draft("task-1", "changes:comments"),
      { ...settings, kind: "ui-fold" }, draft("workspace:broken", "changes:inline-comments"),
    ];
    await remember();
    await seed(addresses);
    await reconcile(lists([]));
    await expectKept(addresses);
  });

  it("preserves a concurrent edit, even one written in the same clock millisecond", async () => {
    vi.spyOn(Date, "now").mockReturnValue(42);
    await seed([settings, directory]);
    const pass = await lifetime.prepareDraftPrune(deviceId);
    await store.writeUiRecord(settings, { body: "newer edit" });
    try { await pass.prune(lists([]), () => true); } finally { pass.dispose(); }
    expect((await store.readUiRecord(settings)).value).toEqual({ body: "newer edit" });
    await expectGone([directory]);
  });

  it("preserves drafts created after a pass captured its candidates", async () => {
    const pass = await lifetime.prepareDraftPrune(deviceId);
    await seed([settings]);
    try { await pass.prune(lists([]), () => true); } finally { pass.dispose(); }
    await expectKept([settings]);
  });

  it("preserves drafts when another tab writes ownership while the lists are being read", async () => {
    await remember();
    await seed([settings, chat()]);
    const pass = await lifetime.prepareDraftPrune(deviceId);
    await remember();
    try { await pass.prune(lists([]), () => true); } finally { pass.dispose(); }
    await expectKept([settings, chat()]);
  });

  it("checks a cancelled sync inside the deletion transaction", async () => {
    await seed([settings]);
    const pass = await lifetime.prepareDraftPrune(deviceId);
    let active = true;
    const pruning = pass.prune(lists([]), () => active);
    active = false;
    try { await pruning; } finally { pass.dispose(); }
    await expectKept([settings]);
  });
});
