/** @vitest-environment jsdom */
// What a machine offers to start work with, asked of it as its session lands.
//
// A surface can want a machine's catalog before that machine has answered in
// this tab (core/surfaceContext.js): it is handed what the disk holds, and the
// machine is asked later. That later ask waits for the session's greeting,
// which is what says whether this tab can read the bridge at all — and an
// answer from a bridge it cannot read never becomes the catalog, on disk or
// held. The greeting that counts is the one the ask goes out on: a session
// greeted again (a new carrier, a restored path) or replaced before the ask is
// sent is waited on afresh, and an answer that lands after its session or its
// greeting was superseded is not kept. These run the real session, RPC and
// greeting code.

import { afterEach, beforeEach, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import {
  SUPPORTED_API,
  UNSUPPORTED_API,
  asked,
  attachSession,
  carrier,
  landSession,
  openLandingSession,
} from "./landingSessionFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { adoptDeviceConnection, knownDeviceContext, loseDeviceConnection, resetDeviceContexts } = await import(
  "../src/core/deviceContexts.js"
);
const { deviceCatalog } = await import("../src/core/inboxDevices.js");
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const { deviceModelsAddress } = await import("../src/core/settingsRecords.js");
const { openSession } = await import("../src/core/session.js");
const { greetLiveBridge } = await import("../src/connection.js");

const flush = async () => {
  for (let i = 0; i < 12; i++) await new Promise((done) => setTimeout(done, 0));
};

const catalogOf = (providerId) => ({
  providers: [{ id: providerId, label: providerId, models: [{ id: `${providerId}-model`, label: "Model" }], efforts: [] }],
});

const modules = { openSession, adoptDeviceConnection, greetLiveBridge };
let sessions = [];

/** A surface asks for the machine's catalog while it is a stand-in, and the
 *  machine's session lands while that ask is still reading the disk. */
async function askAsTheMachineLands() {
  await writeCached(deviceModelsAddress("dev-a"), catalogOf("cached"));
  const standIn = knownDeviceContext("dev-a");
  await flush();
  const firstRead = deviceCatalog("dev-a");
  const landing = await land(standIn);
  await firstRead;
  await flush();
  return { standIn, ...landing };
}

async function land(context) {
  const landing = await landSession("dev-a", context, modules);
  sessions.push(landing.session);
  return landing;
}

/** A landed session's hello, answered with `version`. */
async function greet({ peer, landed }, version = SUPPORTED_API) {
  await flush();
  peer.answer("session.hello", { api_version: version });
  await landed;
  await flush();
}

/** A machine that has answered once, and whose catalog nothing has asked for
 *  yet. */
async function greetedMachine() {
  await writeCached(deviceModelsAddress("dev-a"), catalogOf("cached"));
  const context = knownDeviceContext("dev-a");
  const landing = await land(context);
  await greet(landing);
  return { context, ...landing };
}

const cachedProvider = async () => (await readCached(deviceModelsAddress("dev-a")))?.value?.providers?.[0]?.id;

beforeEach(async () => {
  resetDeviceContexts();
  await wipeCache();
});

afterEach(async () => {
  for (const session of sessions) session.close();
  sessions = [];
  resetDeviceContexts();
  await flush();
});

it("asks a landing machine for its catalog only after its greeting", async () => {
  const { standIn, peer, landed } = await askAsTheMachineLands();
  expect(asked(peer)).toEqual(["session.hello"]);

  peer.answer("session.hello", { api_version: SUPPORTED_API });
  await landed;
  await flush();
  expect(asked(peer)).toContain("models.list");

  peer.answer("models.list", catalogOf("answered"));
  await flush();
  expect(await cachedProvider()).toBe("answered");
  expect((await standIn.modelCatalog()).providers[0].id).toBe("answered");
});

it("keeps what the disk held when the landing bridge speaks an API this tab cannot", async () => {
  const { standIn, peer, landed } = await askAsTheMachineLands();

  peer.answer("session.hello", { api_version: UNSUPPORTED_API });
  await landed;
  await flush();

  expect(standIn.unsupported).toBe("app");
  expect(asked(peer)).not.toContain("models.list");
  expect(await cachedProvider()).toBe("cached");
  expect((await standIn.modelCatalog()).providers[0].id).toBe("cached");
});

it("waits for a restored path's greeting before asking a machine that greeted before", async () => {
  const { context, peer } = await greetedMachine();
  // connection.js greets the same session again when its path is restored
  // under it: the bridge answering now may have restarted as another release.
  const regreeting = greetLiveBridge(context);
  await flush();
  expect((await deviceCatalog("dev-a")).providers[0].id).toBe("cached");
  await flush();
  expect(asked(peer)).not.toContain("models.list");

  peer.answer("session.hello", { api_version: UNSUPPORTED_API });
  await regreeting;
  await flush();
  expect(context.unsupported).toBe("app");
  expect(asked(peer)).not.toContain("models.list");
  expect(await cachedProvider()).toBe("cached");
});

it("waits for a new carrier's greeting before asking a machine that greeted before", async () => {
  const { context, session } = await greetedMachine();
  const nextPeer = carrier();
  const regreeting = session.peer(nextPeer);
  await flush();
  expect((await deviceCatalog("dev-a")).providers[0].id).toBe("cached");
  await flush();
  expect(asked(nextPeer)).toEqual(["session.hello"]);

  nextPeer.answer("session.hello", { api_version: UNSUPPORTED_API });
  await regreeting;
  await flush();
  expect(context.unsupported).toBe("app");
  expect(await cachedProvider()).toBe("cached");
});

it("never asks a session on the greeting of the session it replaced", async () => {
  await writeCached(deviceModelsAddress("dev-a"), catalogOf("cached"));
  const context = knownDeviceContext("dev-a");
  const first = await land(context);
  await deviceCatalog("dev-a");
  await flush();
  // The replacement lands in the gap between the first session's greeting
  // settling and the waiting ask resuming: it is adopted by a continuation of
  // that same greeting, queued behind the ask's own.
  const prepared = await openLandingSession("dev-a", modules);
  sessions.push(prepared.session);
  let second = null;
  context.greeted.then(() => { second = attachSession(prepared, context, modules); });

  await greet(first);
  expect(second).not.toBeNull();
  const before = asked(second.peer);
  if (asked(first.peer).includes("models.list")) first.peer.answer("models.list", catalogOf("first"));
  if (before.includes("models.list")) second.peer.answer("models.list", catalogOf("unsupported"));
  await greet(second, UNSUPPORTED_API);

  expect(before).toEqual(["session.hello"]);
  expect(context.unsupported).toBe("app");
  expect(await cachedProvider()).toBe("cached");
});

it("asks the session that replaced the one whose answer never came", async () => {
  const { context, peer } = await greetedMachine();
  await deviceCatalog("dev-a");
  await flush();
  expect(asked(peer)).toContain("models.list");

  const second = await land(context);
  await greet(second);
  expect(asked(second.peer)).toContain("models.list");
  second.peer.answer("models.list", catalogOf("second"));
  await flush();
  expect(await cachedProvider()).toBe("second");
});

it("asks again once a machine that went away mid-answer is back", async () => {
  const { context, peer, session, lifetime } = await greetedMachine();
  await deviceCatalog("dev-a");
  await flush();
  expect(asked(peer)).toContain("models.list");

  loseDeviceConnection("dev-a", lifetime);
  session.close();
  await flush();
  const second = await land(context);
  await greet(second);
  expect(asked(second.peer)).toContain("models.list");
  second.peer.answer("models.list", catalogOf("second"));
  await flush();
  expect(await cachedProvider()).toBe("second");
});
