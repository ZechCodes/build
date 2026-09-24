/** @vitest-environment jsdom */
// What a machine offers to start work with, asked of it as its session lands.
//
// A surface can want a machine's catalog before that machine has answered in
// this tab (core/surfaceContext.js): it is handed what the disk holds, and the
// machine is asked later. That later ask waits for the session's greeting,
// which is what says whether this tab can read the bridge at all — and an
// answer from a bridge it cannot read never becomes the catalog, on disk or
// held. These run the real session, RPC and greeting code.

import { afterEach, beforeEach, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { SUPPORTED_API, UNSUPPORTED_API, asked, landSession } from "./landingSessionFixture.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { adoptDeviceConnection, knownDeviceContext, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
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

let session = null;

/** A surface asks for the machine's catalog while it is a stand-in, and the
 *  machine's session lands while that ask is still reading the disk. */
async function askAsTheMachineLands() {
  await writeCached(deviceModelsAddress("dev-a"), catalogOf("cached"));
  const standIn = knownDeviceContext("dev-a");
  await flush();
  const firstRead = deviceCatalog("dev-a");
  const landing = await landSession("dev-a", standIn, { openSession, adoptDeviceConnection, greetLiveBridge });
  session = landing.session;
  await firstRead;
  await flush();
  return { standIn, ...landing };
}

const cachedProvider = async () => (await readCached(deviceModelsAddress("dev-a")))?.value?.providers?.[0]?.id;

beforeEach(async () => {
  resetDeviceContexts();
  await wipeCache();
});

afterEach(async () => {
  session?.close();
  session = null;
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
