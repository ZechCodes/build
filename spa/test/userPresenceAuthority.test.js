/** @vitest-environment jsdom */
// Arrivals use the greeting that still owns the session when user.present is sent.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { SUPPORTED_API, UNSUPPORTED_API, asked, landSession } from "./landingSessionFixture.js";

const { adoptDeviceConnection, knownDeviceContext, resetDeviceContexts } = await import("../src/core/deviceContexts.js");
const { openSession } = await import("../src/core/session.js");
const { greetLiveBridge } = await import("../src/connection.js");
const { resetUserPresence, startUserPresence } = await import("../src/core/userPresence.js");
const { readUserSession } = await import("../src/core/userSessionCache.js");
const { wipeCache } = await import("../src/core/localCache.js");

const modules = { openSession, adoptDeviceConnection, greetLiveBridge };
const flush = async () => {
  for (let i = 0; i < 12; i++) await new Promise((done) => setTimeout(done, 0));
};
const compatible = { api_version: SUPPORTED_API, capabilities: ["tasks.doneSinceLeft"] };
const sessionAnswer = { user_session: {
  session_started_ms: 50, last_activity_ms: 50, previous_session_ended_ms: 10, gap_ms: 21_600_000, now_ms: 50,
} };

let session;
let focused;

beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  resetDeviceContexts();
  await wipeCache();
  focused = vi.spyOn(document, "hasFocus").mockReturnValue(true);
});

afterEach(() => {
  resetUserPresence();
  session?.close();
  session = null;
  resetDeviceContexts();
  focused.mockRestore();
});

async function greetedSession() {
  const context = knownDeviceContext("dev-a");
  const landing = await landSession("dev-a", context, modules);
  session = landing.session;
  await flush();
  landing.peer.answer("session.hello", compatible);
  await landing.landed;
  await flush();
  return { context, peer: landing.peer };
}

it("does not send an arrival on an older compatible hello while the newer hello is pending", async () => {
  const { context, peer } = await greetedSession();
  const older = greetLiveBridge(context);
  startUserPresence();
  const newer = greetLiveBridge(context);
  await flush();

  peer.answerNth("session.hello", 1, compatible);
  await older;
  await flush();
  expect(asked(peer)).not.toContain("user.present");

  peer.answerNth("session.hello", 2, { api_version: UNSUPPORTED_API });
  await newer;
  await flush();
  expect(asked(peer)).not.toContain("user.present");
  expect(await readUserSession("dev-a")).toBe(null);
});

it("does not cache an arrival answer after a newer unsupported hello", async () => {
  const { context, peer } = await greetedSession();
  startUserPresence();
  await flush();
  expect(asked(peer)).toContain("user.present");

  const newer = greetLiveBridge(context);
  await flush();
  peer.answerNth("session.hello", 1, { api_version: UNSUPPORTED_API });
  await newer;
  peer.answer("user.present", sessionAnswer);
  await flush();
  expect(await readUserSession("dev-a")).toBe(null);
});
