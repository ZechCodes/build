/** @vitest-environment jsdom */
// The strip over a surface stood up on a machine's records before that machine
// answered in this tab (core/surfaceContext.js).
//
// Such a machine is not "not connected" yet: nothing has been tried, and the
// cold reload that stood it up is dialling it. But the account's own answer
// about it is news. When the account says the machine is offline, the strip
// says so, exactly as it does for a machine that answered once and went — it is
// a status display, and it has to be true.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const spies = vi.hoisted(() => ({ fetchDevices: vi.fn() }));
vi.mock("../src/api.js", async (original) => ({ ...(await original()), fetchDevices: spies.fetchDevices }));

let contexts, App, host, disposeStrip;

const flush = async () => {
  for (let i = 0; i < 10; i++) await new Promise((done) => setTimeout(done, 0));
};

/** A cold reload: the page stands on the machine's records, and the strip is
 *  mounted over it. */
async function standOnRecords() {
  const standIn = contexts.knownDeviceContext("dev-a");
  const { mountDeviceStrip } = await import("../src/core/deviceNotice.js");
  host = document.getElementById("root");
  host.innerHTML = "cached content";
  disposeStrip = mountDeviceStrip(host, standIn);
  return standIn;
}

/** One read of the account's device list, answering `status` for the machine. */
async function accountSays(status) {
  spies.fetchDevices.mockResolvedValue([{ id: "dev-a", name: "Laptop", status, last_seen_at: "2026-09-01T00:00:00Z" }]);
  const { readPresence } = await import("../src/devices.js");
  await readPresence();
  await flush();
}

const strip = () => host.querySelector(".device-strip");

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  document.body.innerHTML = '<div id="root"></div>';
  spies.fetchDevices.mockReset();
  contexts = await import("../src/core/deviceContexts.js");
  ({ App } = await import("../src/app.js"));
  App.devices = [{ id: "dev-a", name: "Laptop", status: "offline" }]; // the cache's list, as boot paints it
});

afterEach(() => {
  disposeStrip?.();
  disposeStrip = null;
  contexts.resetDeviceContexts();
});

it("says nothing over a machine the account has not answered about yet", async () => {
  await standOnRecords();
  await flush();
  expect(strip()).toBeNull();
});

it("says the machine is not connected once the account says it is offline", async () => {
  const standIn = await standOnRecords();
  await accountSays("offline");

  expect(strip()).not.toBeNull();
  // …without counting the machine as tried: nothing here has dialled it, and
  // the stale-listing guess still may (connectionOffline.test.js).
  expect(contexts.awaitingFirstAnswer(standIn)).toBe(true);
  expect(host.textContent).toContain("cached content");
});
