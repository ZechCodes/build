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

const spies = vi.hoisted(() => ({ mountRail: vi.fn(() => ({ dispose: vi.fn() })), notify: vi.fn() }));
vi.mock("../src/core/agentRail.js", () => ({ mountAgentRail: spies.mountRail }));
vi.mock("../src/core/notify.js", () => ({ notifyError: spies.notify }));

let contexts, shell, cache, App, session;

const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};

// An in-memory transport: the envelope carries the frame as it is.
const transport = {
  encryptFrame: async ({ outerFields, frameFields }) => ({ outerFields, frameFields }),
  decryptEnvelope: async ({ envelope }) => ({ payload: envelope.frameFields.payload }),
};

/** A carrier that records every request sent over it and answers on cue. */
function carrier() {
  const readers = new Set();
  return {
    sent: [],
    onClose: () => () => {},
    onEnvelope: (fn) => {
      readers.add(fn);
      return () => readers.delete(fn);
    },
    close() {},
    send(envelope) {
      this.sent.push(envelope.frameFields.payload);
    },
    answer(method, result) {
      const asked = this.sent.find((request) => request.method === method);
      for (const fn of readers) fn({ frameFields: { payload: { id: asked.id, ok: true, result } } });
    },
  };
}

/** What this tab was asked over the carrier, in the order it asked. */
const asked = (peer) => peer.sent.map((request) => request.method);

/** An API version this tab has an adapter for (core/bridgeApi). */
const SUPPORTED_API = "1.22.0";

const route = { name: "project", deviceId: "dev-a", projectId: "proj-1" };

/** The machine's cached project list, as the last session left it. */
const listProjects = (context, rows) =>
  cache.writeCached(context.cacheScope.address({ entityId: "", kind: "projects" }), rows);

/** The machine's session landing the way connection.js lands one: adopted
 *  first, its greeting armed on the carrier, then the peer attached. */
async function landSession(context) {
  const { openSession } = await import("../src/core/session.js");
  const { greetLiveBridge } = await import("../src/connection.js");
  const signal = carrier();
  const peer = carrier();
  session = await openSession({
    deviceId: "dev-a",
    transport,
    rendezvous: {
      mint: async () => ({ sessionId: "s", deviceId: "dev-a", sessionKeyB64: "key" }),
      signalCarrier: () => signal,
    },
  });
  contexts.adoptDeviceConnection(session);
  session.onCarrier(() => greetLiveBridge(context));
  const landed = session.peer(peer);
  return { peer, landed };
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
});

afterEach(() => {
  shell.stopShell();
  session?.close();
  session = null;
  contexts.resetDeviceContexts();
});

it("asks a landing machine for the project's owner only after its greeting", async () => {
  const standIn = contexts.knownDeviceContext("dev-a");
  await listProjects(standIn, [{ project_id: "proj-1", name: "Project" }]);
  shell.standShell(App.route);
  await flush();
  expect(spies.mountRail).not.toHaveBeenCalled();

  const { peer, landed } = await landSession(standIn);
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

  const { peer, landed } = await landSession(standIn);
  await flush();
  peer.answer("session.hello", { api_version: "99.0.0" });
  await landed;
  await flush();

  expect(standIn.unsupported).toBe("app");
  expect(asked(peer)).not.toContain("project.ensure_conversation");
  expect(spies.mountRail).not.toHaveBeenCalled();
  expect(spies.notify).not.toHaveBeenCalled();
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
