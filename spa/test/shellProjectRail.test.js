/** @vitest-environment jsdom */
// How the shell finds the conversation a project page — and a task page of
// that project — stands on.
//
// The owner comes off the cached project list first, the bridge is asked only
// when the list names none, and a bridge that cannot be reached does not leave
// the page without its rail for good. These three were core/projectAgentRail.js
// before the rail became the shell's; they are the shell's now, and so are they.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const mountAgentRail = vi.fn(() => ({ dispose: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: (...args) => mountAgentRail(...args) }));
const notifyError = vi.fn();
vi.mock("../src/core/notify.js", () => ({ notifyError: (...args) => notifyError(...args) }));

let standShell, stopShell, writeCached, scopeFor, clearCacheScope, stampProject, adoptDeviceSession, adoptBridgeSelection, resetDeviceContexts;
let scope, rpc;

const route = { name: "project", deviceId: "dev-1", projectId: "proj-1" };
const taskRoute = { name: "trackerTask", deviceId: "dev-1", projectId: "proj-1", taskId: "i-1" };

const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};

/** The machine's session landing and greeting: the shell asks a machine to
 *  mint nothing before its bridge has said which API it speaks. */
const land = () => {
  const context = adoptDeviceSession({ deviceId: "dev-1", call: (...args) => rpc(...args), close: () => {}, peer: () => {}, onCarrier: () => {} });
  adoptBridgeSelection(context, { version: "2.0.0" }, null);
};

const listProjects = (rows) =>
  writeCached(scope.address({ entityId: "", kind: "projects" }), rows.map((row) => stampProject(row, "dev-1")));

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  mountAgentRail.mockClear();
  notifyError.mockClear();
  ({ standShell, stopShell } = await import("../src/core/shell.js"));
  ({ writeCached } = await import("../src/core/localCache.js"));
  ({ scopeFor, clearCacheScope } = await import("../src/core/cacheScope.js"));
  ({ stampProject } = await import("../src/core/feedMerge.js"));
  ({ adoptDeviceSession, adoptBridgeSelection, resetDeviceContexts } = await import("../src/core/deviceContexts.js"));
  scope = scopeFor("dev-1");
  rpc = vi.fn(async () => ({ entity_id: "run-minted" }));
  land();
});

afterEach(() => {
  stopShell();
  resetDeviceContexts();
  clearCacheScope();
});

describe("the conversation a project page stands on", () => {
  it("stands on the owner the cached project list names, asking the bridge nothing", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    standShell(route);
    await flush();
    expect(rpc).not.toHaveBeenCalled();
    const [host, options] = mountAgentRail.mock.calls[0];
    expect(host.id).toBe("agent-rail");
    expect(options).toMatchObject({ kind: "project", projectId: "proj-1", entityId: "run-7", deviceId: "dev-1" });
  });

  // The bubbles on a project page wear the project's INITIAL in place of a
  // pattern (core/agentRailModel.js, the kind "project" branch), and the name
  // that initial is taken from comes off the same cached row as the owner. With
  // no name the rail falls back to "?", which is what the project page wore
  // after the shell roll.
  it("names the project, so its bubbles wear its initial and not a question mark", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    standShell(route);
    await flush();
    expect(mountAgentRail.mock.calls[0][1].projectName).toBe("build");
  });

  it("asks the bridge for an owner when the list names none", async () => {
    await listProjects([{ project_id: "proj-1", name: "build" }]);
    standShell(route);
    await flush();
    expect(rpc).toHaveBeenCalledWith("project.ensure_conversation", { project_id: "proj-1" });
    expect(mountAgentRail.mock.calls[0][1].entityId).toBe("run-minted");
  });

  // A phone whose session dropped mid-call: the reader is told, and the rail
  // still comes up when the sync layer lists the owner.
  it("stands up once the cache learns the owner after the bridge could not be asked", async () => {
    rpc.mockRejectedValue(new Error("session ended"));
    standShell(route);
    await flush();
    expect(notifyError).toHaveBeenCalledWith("No conversation for this project", "session ended");
    expect(mountAgentRail).not.toHaveBeenCalled();

    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    await flush();
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    expect(mountAgentRail.mock.calls[0][1].entityId).toBe("run-7");

    // …and the wait is the shell's to end: leaving takes the rail it stood up.
    const rail = mountAgentRail.mock.results[0].value;
    stopShell();
    expect(rail.dispose).toHaveBeenCalled();
  });

  // A page can stand on a machine's records while that machine cannot answer
  // (core/surfaceContext.js). Asking it for an owner then can only be refused,
  // so the shell waits quietly — on the list, and on the machine — and asks
  // the moment it can.
  it("waits quietly for a machine that cannot answer, and asks it for the owner once it lands", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    await listProjects([{ project_id: "proj-1", name: "build" }]);
    setContextOffline("dev-1");
    standShell(route);
    await flush();
    expect(rpc).not.toHaveBeenCalled();
    expect(notifyError).not.toHaveBeenCalled();
    expect(mountAgentRail).not.toHaveBeenCalled();

    land();
    await flush();

    expect(rpc).toHaveBeenCalledWith("project.ensure_conversation", { project_id: "proj-1" });
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    expect(mountAgentRail.mock.calls[0][1].entityId).toBe("run-minted");
  });

  it("stops waiting on the machine once the list names the owner first", async () => {
    const { setContextOffline } = await import("../src/core/deviceContexts.js");
    await listProjects([{ project_id: "proj-1", name: "build" }]);
    setContextOffline("dev-1");
    standShell(route);
    await flush();

    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    await flush();
    land();
    await flush();

    expect(rpc).not.toHaveBeenCalled();
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
    expect(mountAgentRail.mock.calls[0][1].entityId).toBe("run-7");
  });

  it("stands nothing up when the reader left while the owner was being found", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    standShell(route);
    stopShell(); // the navigation away, before the cache read resolves
    await flush();
    expect(mountAgentRail).not.toHaveBeenCalled();
  });
});

describe("a task of the project", () => {
  // A tracker task carries no conversation of its own — the agents its page
  // names are workspace agents it can be assigned to — so its page stands on
  // the project's agent, exactly as the project page does.
  it("stands on the project's conversation, not on one of the task's own", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    standShell(taskRoute);
    await flush();
    expect(mountAgentRail.mock.calls[0][1]).toMatchObject({ kind: "project", projectId: "proj-1", entityId: "run-7" });
  });

  it("is the same standing as the project page, so opening one keeps the strip", async () => {
    await listProjects([{ project_id: "proj-1", name: "build", entity_id: "run-7" }]);
    standShell(route);
    await flush();
    expect(mountAgentRail).toHaveBeenCalledTimes(1);

    // Pressing a task on the Tasks tab: the page swaps inside the shell and
    // the bubbles beside it do not move.
    standShell(taskRoute);
    await flush();
    expect(mountAgentRail).toHaveBeenCalledTimes(1);
  });
});
