// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { passesSettled } from "./waits.js";

const fake = vi.hoisted(() => ({ lazy: true, context: null, watchers: [], route: { name: "inbox" } }));
vi.mock("../src/appState.js", () => ({ App: { devices: [{ id: "lazy-device" }], get route() { return fake.route; } } }));
vi.mock("../src/app.js", async () => ({ App: (await import("../src/appState.js")).App }));
vi.mock("../src/core/scopedSyncRuntime.js", async (original) => ({
  ...await original(), lazyViewReadsEnabled: () => fake.lazy,
}));
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => fake.context, liveContexts: () => [fake.context],
  whenGreeted: async (_context, dispatch) => ({ sent: dispatch(() => true), stands: () => true }),
  onDeviceStateChanged: () => () => {},
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } }),
  subscriptionsSettledFor: async () => {}, onSubscriptionHeld: () => () => {},
  watchChanges: (registration) => {
    const held = { ...registration, disposed: false };
    fake.watchers.push(held);
    return { dispose: () => { held.disposed = true; } };
  },
}));
let sync;
const row = { run_id: "lazy-run", kind: "branch", state: "building", project_id: "lazy-project",
  agents: [{ id: "lazy-agent", conversation_id: "lazy-conversation" }] };
beforeEach(async () => {
  vi.resetModules(); fake.lazy = true; fake.watchers = []; fake.route = { name: "inbox" };
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  const answers = {
    "board.list": { items: [row] }, "project.list": { projects: [] }, "workspace.list": { workspaces: [] },
    "thread.page": { items: [], has_more: false }, "git.status": { files: [] }, "fs.tree": { entries: [] },
    "term.list": { terminals: [] }, "git.log": { commits: [] }, "git.unpushed": { commits: [] },
  };
  fake.context = { deviceId: "lazy-device", session: {}, greeted: Promise.resolve(), active: () => true,
    rpc: vi.fn(async (method) => answers[method] || {}) };
  fake.context.cacheScope = { deviceId: "lazy-device", active: () => true };
  sync = await import("../src/core/cacheSync.js");
});
afterEach(async () => { sync?.stopCacheSync(); await passesSettled(sync, "lazy-device"); });

it("removes all-scope body subscriptions and unopened push repairs, then restores legacy behavior on downgrade", async () => {
  sync.startCacheSync(); await passesSettled(sync, "lazy-device");
  const calls = () => fake.context.rpc.mock.calls.map(([method]) => method);
  expect(calls().filter((method) => /^(thread\.|git\.|fs\.)/.test(method))).toEqual([]);
  const allKinds = fake.watchers.filter((held) => !held.disposed && held.scope === "all").flatMap((held) => held.kinds);
  expect(allKinds).not.toContain("thread"); expect(allKinds).not.toContain("git"); expect(allKinds).not.toContain("files");
  fake.context.rpc.mockClear();
  await fake.watchers.find((held) => held.id === "s-inbox" && !held.disposed).onChanges([{ entity_id: "lazy-run", thread: [{ agent_id: "lazy-agent", conversation_id: "lazy-conversation", last_sequence: 10 }],
    git: { log: { commits: [{ hash: "synthetic-commit", ahead_of_base: true }] } }, files: { paths: ["src/a.js"] } }]);
  expect(calls().filter((method) => /^(thread\.|git\.|fs\.)/.test(method))).toEqual([]);
  fake.lazy = false;
  await sync.syncDevice("lazy-device");
  expect(calls()).toContain("thread.page"); expect(calls()).toContain("git.status");
  expect(fake.watchers.filter((held) => !held.disposed && held.scope === "all").flatMap((held) => held.kinds)).toContain("thread");
});
