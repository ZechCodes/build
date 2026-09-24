/** @vitest-environment jsdom */
// A project page stood up over a machine's records before that machine
// answered (core/surfaceContext.js), with no conversation owner on its cached
// row: what the shell does when the machine's session lands under it.
//
// Two things the landing must not do. It must not ask a bridge that has not yet
// said which API it speaks to mint anything — `project.ensure_conversation`
// creates a conversation, and a bridge this tab cannot read is not asked to.
// And the rail the owner stands up is stood up once: the list naming the owner
// and the machine answering for it can land together, and only one of them
// mounts. These run the real session, RPC and greeting code, as the review of
// #136 did to find both.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import {
  SUPPORTED_API,
  UNSUPPORTED_API,
  asked,
  attachSession,
  landSession,
  openLandingSession,
} from "./landingSessionFixture.js";

const spies = vi.hoisted(() => ({ mountRail: vi.fn(() => ({ dispose: vi.fn() })), notify: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: spies.mountRail }));
vi.mock("../src/core/notify.js", () => ({ notifyError: spies.notify }));

let contexts, shell, cache, App, sessions;

const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};

const route = { name: "project", deviceId: "dev-a", projectId: "proj-1" };

/** The machine's cached project list, as the last session left it. */
const listProjects = (context, rows) =>
  cache.writeCached(context.cacheScope.address({ entityId: "", kind: "projects" }), rows);

/** The session modules, on this suite's own module registry. */
const sessionModules = async () => ({
  ...(await import("../src/core/session.js")),
  ...(await import("../src/connection.js")),
  adoptDeviceConnection: contexts.adoptDeviceConnection,
});

/** The machine's session landing. */
async function landOn(context) {
  const landing = await landSession("dev-a", context, await sessionModules());
  sessions.push(landing.session);
  return landing;
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="root"></div><aside id="agent-rail"></aside><div id="console-region"></div>';
  spies.mountRail.mockClear();
  spies.notify.mockClear();
  contexts = await import("../src/core/deviceContexts.js");
  shell = await import("../src/core/shell.js");
  cache = await import("../src/core/localCache.js");
  ({ App } = await import("../src/app.js"));
  App.devices = [{ id: "dev-a", name: "Laptop", status: "online" }];
  App.route = { ...route };
  sessions = [];
});

afterEach(() => {
  shell.stopShell();
  for (const session of sessions) session.close();
  contexts.resetDeviceContexts();
});

it("asks a landing machine for the project's owner only after its greeting", async () => {
  const standIn = contexts.knownDeviceContext("dev-a");
  await listProjects(standIn, [{ project_id: "proj-1", name: "Project" }]);
  shell.standShell(App.route);
  await flush();
  expect(spies.mountRail).not.toHaveBeenCalled();

  const { peer, landed } = await landOn(standIn);
  await flush();
  expect(asked(peer)).toEqual(["session.hello"]);

  peer.answer("session.hello", { api_version: SUPPORTED_API });
  await landed;
  await flush();
  expect(asked(peer)).toContain("project.ensure_conversation");
  peer.answer("project.ensure_conversation", { entity_id: "run-minted" });
  await flush();
  expect(spies.mountRail).toHaveBeenCalledOnce();
  expect(spies.mountRail.mock.calls[0][1].entityId).toBe("run-minted");
});

it("mints nothing on a landing machine whose bridge speaks an API this tab cannot", async () => {
  const standIn = contexts.knownDeviceContext("dev-a");
  await listProjects(standIn, [{ project_id: "proj-1", name: "Project" }]);
  shell.standShell(App.route);
  await flush();

  const { peer, landed } = await landOn(standIn);
  await flush();
  peer.answer("session.hello", { api_version: UNSUPPORTED_API });
  await landed;
  await flush();

  expect(standIn.unsupported).toBe("app");
  expect(asked(peer)).not.toContain("project.ensure_conversation");
  expect(spies.mountRail).not.toHaveBeenCalled();
  expect(spies.notify).not.toHaveBeenCalled();
});

it("does not mint a project owner on an older hello while a newer hello is pending", async () => {
  const context = contexts.knownDeviceContext("dev-a");
  await listProjects(context, [{ project_id: "proj-1", name: "Project" }]);
  const { peer, landed } = await landOn(context);
  peer.answer("session.hello", { api_version: SUPPORTED_API });
  await landed;

  const { greetLiveBridge } = await import("../src/connection.js");
  const older = greetLiveBridge(context);
  const newer = greetLiveBridge(context);
  shell.standShell(App.route);
  await flush();
  expect(asked(peer).filter((method) => method === "session.hello")).toHaveLength(3);
  expect(asked(peer)).not.toContain("project.ensure_conversation");

  peer.answerNth("session.hello", 1, { api_version: SUPPORTED_API });
  await older;
  await flush();
  expect(asked(peer)).not.toContain("project.ensure_conversation");

  peer.answerNth("session.hello", 2, { api_version: UNSUPPORTED_API });
  await newer;
  await flush();
  expect(context.unsupported).toBe("app");
  expect(context.adapter).toBe(null);
  expect(asked(peer)).not.toContain("project.ensure_conversation");
  expect(spies.mountRail).not.toHaveBeenCalled();
});

it("mints nothing on a session on the greeting of the session it replaced", async () => {
  const standIn = contexts.knownDeviceContext("dev-a");
  await listProjects(standIn, [{ project_id: "proj-1", name: "Project" }]);
  shell.standShell(App.route);
  await flush();
  const first = await landOn(standIn);
  await flush();
  // The replacement is adopted by a continuation of the first session's
  // greeting, queued behind the shell's own wait on it.
  const modules = await sessionModules();
  const prepared = await openLandingSession("dev-a", modules);
  sessions.push(prepared.session);
  let second = null;
  standIn.greeted.then(() => { second = attachSession(prepared, standIn, modules); });

  first.peer.answer("session.hello", { api_version: SUPPORTED_API });
  await first.landed;
  await flush();
  expect(second).not.toBeNull();
  const before = asked(second.peer);
  second.peer.answer("session.hello", { api_version: UNSUPPORTED_API });
  await second.landed;
  await flush();

  expect(before).toEqual(["session.hello"]);
  expect(standIn.unsupported).toBe("app");
});

it("stands one rail when the list names the owner as the machine lands", async () => {
  const standIn = contexts.knownDeviceContext("dev-a");
  await listProjects(standIn, [{ project_id: "proj-1", name: "Project" }]);
  shell.standShell(App.route);
  await flush();

  // Another tab learned the owner: the write announces the moment it commits,
  // so the shell's read of the list is in flight when the session lands.
  await listProjects(standIn, [{ project_id: "proj-1", name: "Project", entity_id: "run-listed" }]);
  contexts.adoptDeviceSession({ deviceId: "dev-a", call: async () => ({ entity_id: "run-listed" }) });
  contexts.adoptBridgeSelection(standIn, { version: SUPPORTED_API }, null);
  await flush();

  expect(spies.mountRail).toHaveBeenCalledOnce();
  expect(spies.mountRail.mock.calls[0][1].entityId).toBe("run-listed");
  const rail = spies.mountRail.mock.results[0].value;
  shell.stopShell();
  expect(rail.dispose).toHaveBeenCalledOnce();
});
