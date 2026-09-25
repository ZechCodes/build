/** @vitest-environment jsdom */
// A machine the account stops listing is retired the way an explicit removal
// retires it (#141): through the real registry, the real connection layer and
// the real cache, with only the api's answer stood in. The #136 reviewer's
// reproducer, inverted — it recorded a stand-in context outliving its machine.

import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

const spies = vi.hoisted(() => ({ fetchDevices: vi.fn() }));
vi.mock("../src/api.js", async (original) => ({ ...(await original()), fetchDevices: spies.fetchDevices }));

let ctx, cache, App, devices, surfaceContext;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  spies.fetchDevices.mockReset();
  ctx = await import("../src/core/deviceContexts.js");
  cache = await import("../src/core/localCache.js");
  ({ App } = await import("../src/app.js"));
  devices = await import("../src/devices.js");
  ({ surfaceContext } = await import("../src/core/surfaceContext.js"));
  App.devices = [{ id: "dev-a", name: "Laptop", status: "online" }];
});

afterEach(() => ctx.resetDeviceContexts());

it("retires a stand-in whose machine the account no longer lists", async () => {
  const standIn = ctx.knownDeviceContext("dev-a");
  await cache.writeCached(cache.DEVICES_ADDRESS, App.devices);
  spies.fetchDevices.mockResolvedValue([]);

  await devices.readPresence();

  expect(App.devices).toEqual([]);
  expect(ctx.contextFor("dev-a")).toBeNull();
  expect(standIn.active()).toBe(false);
  expect(standIn.cacheScope.active()).toBe(false);
  expect(() => standIn.chatRepository.currentCall()).toThrow("scope is no longer active");
  // What a surface over it stands on now: no machine at all.
  expect(surfaceContext({ deviceId: "dev-a" })).toBeNull();
});

it("keeps a machine through a list that omits it without being a read of the account", async () => {
  const held = ctx.knownDeviceContext("dev-a");
  await cache.writeCached(cache.DEVICES_ADDRESS, App.devices);

  // Another tab's write, or an eviction: disk changed, the account was not asked.
  await cache.writeCached(cache.DEVICES_ADDRESS, []);
  await devices.readCachedDevices();
  expect(App.devices).toEqual([]);

  // A read that fails says nothing about any machine.
  spies.fetchDevices.mockRejectedValueOnce(new Error("api unavailable"));
  await expect(devices.readPresence()).resolves.toBeNull();

  expect(ctx.contextFor("dev-a")).toBe(held);
  expect(held.active()).toBe(true);
  expect(held.chatRepository.currentCall()).toBeTypeOf("function");
});

it("keeps a machine this client took up while the read was in flight", async () => {
  let answer;
  spies.fetchDevices.mockImplementationOnce(() => new Promise((resolve) => { answer = resolve; }));
  const pending = devices.readPresence();
  const joined = ctx.knownDeviceContext("dev-a");

  answer([]);
  await pending;

  expect(ctx.contextFor("dev-a")).toBe(joined);
  expect(joined.active()).toBe(true);
});
