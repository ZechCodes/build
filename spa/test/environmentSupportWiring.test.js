/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import greetingFixture from "../../fixtures/api/v1/session.hello.json";
import { capabilitiesOf } from "../src/core/bridgeApi/v1/index.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const { bridgeCapabilities, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { environmentSupportAddress, readEnvironmentSupport, rememberEnvironmentSupport, predatesEnvironments } = await import("../src/core/environmentSupport.js");
const { readCached } = await import("../src/core/localCache.js");
const unsupported = { list: false, createProject: false, create: false, addedSources: false, archive: false, projectRename: false, environmentRename: false, secrets: false, deleteEnvironment: false, deleteProject: false };
const m1 = { ...unsupported, list: true, createProject: true };
const greet = (capabilities, deviceId = "dev-1") => greetBridge(async (method) =>
  method === "session.hello" ? { ...greetingFixture.result, capabilities } : {}, { deviceId, strict: true });

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
});
afterEach(() => resetChangeEvents());

it("writes environment-support from a named greeting, independently per device", async () => {
  await greet(["environments"]);
  await vi.waitFor(async () => expect(await readEnvironmentSupport("dev-1")).toEqual(m1));
  expect((await readCached(environmentSupportAddress("dev-1"))).value).toEqual({ ...m1, bridge_epoch: greetingFixture.result.bridge_epoch });
  expect(environmentSupportAddress("dev-1")).toEqual({ deviceId: "dev-1", entityId: "", kind: "environment-support" });
  expect(await readEnvironmentSupport("dev-2")).toEqual(unsupported);
  resetChangeEvents();
  expect(bridgeCapabilities("dev-1").environments).toEqual(unsupported);
  expect(await readEnvironmentSupport("dev-1")).toEqual(m1);
});

it("replaces cached support on downgrade and never infers support from the version or record name", async () => {
  await greet(["environments"]);
  await vi.waitFor(async () => expect(await readEnvironmentSupport("dev-1")).toEqual(m1));
  await greet(["environment-support"]);
  await vi.waitFor(async () => expect(await readEnvironmentSupport("dev-1")).toEqual(unsupported));
  expect(capabilitiesOf({ api_version: "3.99.0", capabilities: [] }).environments).toEqual(unsupported);
  expect(await readEnvironmentSupport()).toEqual(unsupported);
});

it("maps later checkpoint flags only from their own named capabilities", () => {
  expect(capabilitiesOf({ api_version: "3.36.0", capabilities: ["environments.create", "projects.addedSources", "projects.archive", "environments.rename", "secrets.environmentScope"] }).environments)
    .toEqual({ ...unsupported, create: true, addedSources: true, archive: true, environmentRename: true, secrets: true });
  expect(capabilitiesOf({ api_version: "3.36.0", capabilities: ["projects.rename"] }).environments.environmentRename).toBe(false);
});

it("does not let a retired greeting change the support record", async () => {
  await rememberEnvironmentSupport("dev-1", { environments: m1 });
  await rememberEnvironmentSupport("dev-1", { environments: unsupported }, () => false);
  expect(await readEnvironmentSupport("dev-1")).toEqual(m1);
});

it.each([
  [["projects.rename"], { projectRename: true, environmentRename: false, archive: false }],
  [["environments.rename"], { projectRename: false, environmentRename: true, archive: false }],
  [["projects.archive"], { projectRename: false, environmentRename: false, archive: true }],
])("keeps project/environment rename and archive separate for %j", async (names, expected) => {
  await greet(names);
  expect(bridgeCapabilities("dev-1").environments).toMatchObject(expected);
  await vi.waitFor(async () => expect(await readEnvironmentSupport("dev-1")).toMatchObject(expected));
});

it("keeps cached ownership capability evidence when a greeting is refused", async () => {
  await greet(["environments", "projects.addedSources"]);
  await vi.waitFor(async () => expect((await readEnvironmentSupport("dev-1")).addedSources).toBe(true));
  const before = (await readCached(environmentSupportAddress("dev-1"))).value;
  await expect(greetBridge(async () => { throw new Error("unknown method: session.hello"); }, { deviceId: "dev-1", strict: true })).resolves.toBe(false);
  expect((await readCached(environmentSupportAddress("dev-1"))).value).toEqual(before);
});

it("enables archive and both rename actions from the released greeting fixture", async () => {
  await greet(greetingFixture.result.capabilities);
  await vi.waitFor(async () => expect(await readEnvironmentSupport("dev-1")).toMatchObject({
    archive: true, projectRename: true, environmentRename: true,
  }));
});

it("does not treat the old shared rename flag as complete legacy ownership evidence", () => {
  const { projectRename: _projectRename, environmentRename: _environmentRename, ...old } = unsupported;
  for (const rename of [false, true]) {
    expect(predatesEnvironments({ ...old, rename })).toBe(false);
  }
  expect(predatesEnvironments(unsupported)).toBe(true);
});

it("releases independently named delete capabilities and persists them for settings", async () => {
  await greet(["environments.delete", "projects.deleteTasks"]);
  await vi.waitFor(async () => expect(await readEnvironmentSupport("dev-1")).toMatchObject({ deleteEnvironment: true, deleteProject: true }));
  expect(capabilitiesOf({ api_version: "3.41.0", capabilities: ["project.delete", "environment.delete"] }).environments).toMatchObject({ deleteEnvironment: false, deleteProject: false });
});

it("keeps older complete legacy records usable, but a positive delete capability is environment evidence", () => {
  const { deleteEnvironment: _environment, deleteProject: _project, ...older } = unsupported;
  expect(predatesEnvironments(older)).toBe(true);
  expect(predatesEnvironments({ ...older, deleteEnvironment: true })).toBe(false);
  expect(predatesEnvironments({ ...older, deleteProject: true })).toBe(false);
});
