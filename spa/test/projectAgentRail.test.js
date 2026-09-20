/** @vitest-environment jsdom */
// The project's rail beside a page: its owner comes off the cached project list
// first, the bridge is asked only when the list names none, and a bridge that
// cannot be reached does not leave the page without its rail for good.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const mountAgentRail = vi.fn(() => ({ dispose: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

let mountProjectAgentRail, writeCached, scopeFor, clearCacheScope, stampProject;
let context, rpc;
const route = { name: "project", deviceId: "dev-1", projectId: "proj-1" };
const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};
const listProjects = (rows) =>
  writeCached(context.cacheScope.address({ entityId: "", kind: "projects" }), rows.map((row) => stampProject(row, "dev-1")));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="root"></div><aside id="agent-rail"></aside>';
  mountAgentRail.mockClear();
  notifyError.mockClear();
  ({ mountProjectAgentRail } = await import("../src/core/projectAgentRail.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ scopeFor, clearCacheScope } = await import("../src/core/cacheScope.js"));
  ({ stampProject } = await import("../src/core/feedMerge.js"));
  rpc = vi.fn(async () => ({ entity_id: "run-minted" }));
  context = { rpc, deviceId: "dev-1", cacheScope: scopeFor("dev-1"), chatRepository: {} };
});

afterEach(() => clearCacheScope());

describe("the project's rail", () => {
  it("mounts on the owner the cached project list names, asking the bridge nothing", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    const rail = await mountProjectAgentRail({ context, route, selection: {} });
    expect(rail).not.toBeNull();
    expect(rpc).not.toHaveBeenCalled();
    const [host, options] = mountAgentRail.mock.calls[0];
    expect(host.id).toBe("agent-rail");
    expect(options).toMatchObject({ kind: "project", projectId: "proj-1", entityId: "run-7", deviceId: "dev-1" });
  });

  it("asks the bridge for an owner when the list names none", async () => {
    await listProjects([{ project_id: "proj-1", name: "build" }]);
    await mountProjectAgentRail({ context, route, selection: {} });
    expect(rpc).toHaveBeenCalledWith("project.ensure_conversation", { project_id: "proj-1" });
    expect(mountAgentRail.mock.calls[0][1].entityId).toBe("run-minted");
  });

  // A phone whose session dropped mid-call: the reader is told, and the rail
  // still comes up when the sync layer lists the owner.
  it("mounts once the cache learns the owner after the bridge could not be asked", async () => {
    rpc.mockRejectedValue(new Error("session ended"));
    const handle = await mountProjectAgentRail({ context, route, selection: {} });
    expect(notifyError).toHaveBeenCalledWith("No conversation for this project", "session ended");
    expect(mountAgentRail).not.toHaveBeenCalled();
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    await flush();
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    expect(mountAgentRail.mock.calls[0][1].entityId).toBe("run-7");
    const rail = mountAgentRail.mock.results[0].value;
    handle.dispose();
    expect(rail.dispose).toHaveBeenCalled();
  });

  it("mounts nothing when the page went away while the owner was being found", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    const rail = await mountProjectAgentRail({ context, route, selection: {}, disposed: () => true });
    expect(rail).toBeNull();
    expect(mountAgentRail).not.toHaveBeenCalled();
  });
});
