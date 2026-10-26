// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { called, passesSettled } from "./waits.js";

const App = { route: { name: "inbox" }, devices: [{ id: "dev-1" }] };
vi.mock("../src/appState.js", () => ({ App }));
vi.mock("../src/app.js", () => ({ App }));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ agents: { projectWorkers: true },
    changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "tasks"] } }),
  subscriptionsSettledFor: async () => {},
  onSubscriptionHeld: () => () => {},
  watchChanges: () => ({ dispose() {} }),
}));
let context;
vi.mock("../src/core/deviceContexts.js", () => ({
  contextFor: () => context,
  liveContexts: () => [context],
  onDeviceStateChanged: () => () => {},
}));

const projects = [1, 2, 3].map((number) => ({ project_id: `p${number}`, name: `Project ${number}`, entity_id: `owner-${number}` }));
const pin = (entityId) => ({ id: `${entityId}-pin`, conversation_id: `${entityId}-pin-chat`, watched: true });
const worker = { id: "new-worker", conversation_id: "new-worker-chat", project_role: "worker", watched: true };
const roster = (entityId, workers = []) => ({ entity_id: entityId, pinned_agent_id: pin(entityId).id,
  agents: [pin(entityId), ...workers] });
const old = new Date(Date.now() - 30 * 3600 * 1000).toISOString();
const recent = { run_id: "recent", project_id: "p1", kind: "branch", branch: "build/recent",
  state: "review", anchor: old, last_activity: old, agents: [] };
const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};

let cache, sync, lifetime, rpc, script, releases;
beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  delete navigator.locks;
  App.route = { name: "inbox" };
  releases = [];
  script = {};
  const answers = {
    "board.list": () => ({ items: [recent] }),
    "project.list": () => ({ projects }),
    "workspace.list": () => ({ workspaces: [] }),
    "agent.list": ({ entity_id }) => roster(entity_id, entity_id === "owner-1" ? [worker] : []),
    "thread.page": () => ({ items: [], has_more: false }),
    "term.list": () => ({ terminals: [] }),
    "tasks.list": () => ({ tasks: [] }),
    "tasks.columns": () => ({ columns: [] }),
  };
  rpc = vi.fn(async (method, params = {}) => (script[method] || answers[method] || (() => ({})))(params));
  context = { deviceId: "dev-1", rpc, session: {}, greeted: Promise.resolve(), active: () => true,
    cacheScope: { deviceId: "dev-1", active: () => true } };
  cache = await import("../src/core/localCache.js");
  lifetime = await import("../src/core/cacheLifetime.js");
  sync = await import("../src/core/cacheSync.js");
  await cache.writeCached({ deviceId: "dev-1", entityId: "", kind: "project-agents-support" }, { projectWorkers: true });
});

afterEach(async () => {
  releases.forEach((release) => release());
  sync.stopCacheSync();
  await passesSettled(sync, "dev-1");
  vi.restoreAllMocks();
});

it("expires Recent once and projects roster records once per pass", async () => {
  const expires = vi.spyOn(lifetime, "expireWorkspacesData");
  const agents = await import("../src/core/projectAgentsCache.js");
  const projectRosters = vi.spyOn(agents, "projectsWithAgentRosters");
  sync.startCacheSync();
  await passesSettled(sync, "dev-1");
  expect(expires.mock.calls.map(([deviceId, entityIds]) => [deviceId, entityIds]))
    .toEqual([["dev-1", ["recent"]]]);
  expect(projectRosters).toHaveBeenCalledTimes(1);
  expect(rpc.mock.calls.filter(([method]) => method === "thread.page").map(([, params]) => params.agent_id))
    .toEqual(expect.arrayContaining([worker.id, ...projects.map((project) => pin(project.entity_id).id)]));
  const { syncDiagnostics } = await import("../src/core/syncDiagnostics.js");
  const passes = syncDiagnostics.report().periods.flatMap((period) => period.passes);
  expect(passes.at(-1).selected).toMatchObject({ active: 4, recent: 0, projectOwner: 4 });
});

it("discovers a new owner's worker and follows its route while another roster is stalled", async () => {
  const stalled = gate();
  releases.push(stalled.release);
  script["agent.list"] = async ({ entity_id }) => {
    if (entity_id === "owner-2") await stalled.promise;
    return roster(entity_id, entity_id === "owner-1" ? [worker] : []);
  };
  const agents = await import("../src/core/projectAgentsCache.js");
  const original = agents.syncProjectAgents;
  const firstLanded = gate();
  vi.spyOn(agents, "syncProjectAgents").mockImplementation(async (options) => {
    const result = await original(options);
    if (options.entityId === "owner-1") firstLanded.release();
    return result;
  });
  sync.startCacheSync();
  await firstLanded.promise;
  expect(sync.passInFlight("dev-1")).toBeTruthy();
  App.route = { name: "project", deviceId: "dev-1", projectId: "p1", agent: worker.id };
  const selected = called(rpc, (mock) => mock.mock.calls.some(([method, params]) =>
    method === "thread.page" && params.agent_id === worker.id));
  sync.routeChanged();
  await selected;
  expect((await cache.readCached({ deviceId: "dev-1", entityId: "owner-1", kind: "project-agents" })).value.agents)
    .toContainEqual(worker);
  expect(sync.passInFlight("dev-1")).toBeTruthy();
  stalled.release();
  await passesSettled(sync, "dev-1");
});

it("updates only the changed owner's selection and preserves its board metadata", async () => {
  const { updateRosterSelection } = await import("../src/core/syncRosterSelection.js");
  const owner = { ...recent, run_id: "owner-1", entity_id: "owner-1", dismissed: true, agents: [pin("owner-1")] };
  const other = { run_id: "other", agents: [{ id: "other-worker" }] };
  const view = { projects: projects.map((project) => ({ ...project, projectKey: `dev-1:${project.project_id}` })) };
  const pass = { order: ["owner-1", "other"], rows: new Map([["owner-1", owner], ["other", other]]),
    rosterBaseRows: new Map([["owner-1", owner], ["other", other]]),
    activeIds: new Set(["other"]), selection: { active: 1, recent: 7, projectOwner: 1 } };
  const otherProject = view.projects[1];
  updateRosterSelection(view, pass, 0, roster("owner-1", [worker]));
  expect(pass.rows.get("owner-1")).toMatchObject({ branch: recent.branch, dismissed: true,
    project_id: "p1", projectKey: "dev-1:p1", agents: [pin("owner-1"), worker] });
  expect(pass.rows.get("other")).toBe(other);
  expect(view.projects[1]).toBe(otherProject);
  expect(pass.order).toEqual(["owner-1", "other"]);
  expect(pass.selection).toEqual({ active: 1, recent: 7, projectOwner: 2 });
  updateRosterSelection(view, pass, 0, roster("owner-1"));
  expect(pass.selection).toEqual({ active: 1, recent: 7, projectOwner: 1 });
  expect(pass.order).toEqual(["owner-1", "other"]);
});

it("does not project hidden run metadata onto a newly discovered owner row", async () => {
  const { updateRosterSelection } = await import("../src/core/syncRosterSelection.js");
  const hidden = { ...recent, run_id: "owner-1", agents: [pin("owner-1")] };
  const view = { projects: [projects[0]], items: [], runs: [hidden] };
  const pass = { order: ["owner-1"], rows: new Map([["owner-1", hidden]]), rosterBaseRows: new Map(),
    activeIds: new Set(), selection: { active: 0, recent: 0, projectOwner: 1 } };
  updateRosterSelection(view, pass, 0, roster("owner-1", [worker]));
  expect(pass.rows.get("owner-1").branch).toBeUndefined();
  expect(pass.rows.get("owner-1").last_activity).toBeUndefined();
  expect(pass.selection).toEqual({ active: 2, recent: 0, projectOwner: 2 });
  expect(pass.order).toEqual(["owner-1"]);
});
