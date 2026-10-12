import { beforeEach, expect, it } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { navigationCatalog } from "../src/core/navigationModel.js";
import { globalNavigationPicker, reconcilePickerOrder } from "../src/core/navigationPickerModel.js";
import { navigationVisitAddress, mountNavigationVisits, resetNavigationVisits } from "../src/core/navigationVisits.js";
import { readUiRecord, wipeUiRecords } from "../src/core/localUiStore.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const project = (deviceId, id, name) => ({ deviceId, id, name });
const workspace = (deviceId, project_id, id, name) => ({ deviceId, project_id, id, name, status: "ready", directories: [] });
const feed = { projects: [project("one", "p", "Build"), project("two", "p", "Build")], workspaces: [workspace("one", "p", "w", "Café 文"), workspace("two", "p", "w", "Café 文")] };
const catalog = () => navigationCatalog(feed, { devices: [{ id: "one", name: "Workshop" }, { id: "two", name: "Laptop" }] });
const route = (workspaceId = "w", deviceId = "one") => ({ name: "workspace", deviceId, projectId: "p", workspaceId, tab: "files" });
const storage = () => { const held = new Map(); return { getItem: (key) => held.get(key) ?? null, setItem: (key, value) => held.set(key, value), removeItem: (key) => held.delete(key) }; };
beforeEach(async () => { resetNavigationVisits(); await wipeUiRecords(); });
it("P1 across_projects_two_activations exposes direct destination routes", () => {
  const picker = globalNavigationPicker(catalog());
  expect(picker.groups.flatMap((group) => group.destinations).find((row) => row.key === "two/w").route).toMatchObject({ name: "workspace", deviceId: "two", workspaceId: "w" });
});
it("P2 colliding_devices_and_names searches device and Unicode while retaining context", () => {
  const picker = globalNavigationPicker(catalog(), { query: "Café 文 Laptop" });
  expect(picker.groups.map((group) => group.deviceId)).toEqual(["two"]);
  expect(picker.groups[0].destinations.map((row) => row.key)).toEqual(["two/w"]);
});
it("P3 cache_tick_preserves_input freezes old rows and appends arrivals", () => {
  const rows = (keys) => keys.map((key) => ({ key }));
  expect(reconcilePickerOrder(rows(["b", "a"]), rows(["a", "c", "b"])).map((row) => row.key)).toEqual(["b", "a", "c"]);
  expect(reconcilePickerOrder(rows(["b", "a"]), rows(["a"])).map((row) => row.key)).toEqual(["a"]);
});
it("P5 visit_history_not_quiet_recent records destination transitions, reload, Back and task hosts", async () => {
  const session = storage();
  let clock = 10;
  let visits = mountNavigationVisits({ storage: session, now: () => ++clock });
  await visits.ready;
  await visits.visit(route());
  const first = (await readUiRecord(navigationVisitAddress)).value;
  await visits.visit({ ...route(), tab: "tasks", taskId: "t" });
  expect((await readUiRecord(navigationVisitAddress)).value).toEqual(first);
  await visits.dispose();
  visits = mountNavigationVisits({ storage: session, now: () => ++clock });
  await visits.ready; await visits.visit(route());
  expect((await readUiRecord(navigationVisitAddress)).value).toEqual(first);
  await visits.visit(route("w", "two")); await visits.visit(route());
  await visits.visit({ name: "trackerTask", deviceId: "one", projectId: "p", taskId: "t" });
  expect((await readUiRecord(navigationVisitAddress)).value.map((row) => row.kind + ":" + row.key)).toEqual(["project:one/p", "workspace:one/w", "workspace:two/w"]);
  await visits.dispose();
});
it("P5 visit_history_not_quiet_recent merges independent tabs, caps storage and shows five extant shortcuts", async () => {
  const left = mountNavigationVisits({ storage: storage() });
  const right = mountNavigationVisits({ storage: storage() });
  await Promise.all([left.ready, right.ready]);
  await Promise.all([left.visit(route()), right.visit(route("w", "two"))]);
  expect((await readUiRecord(navigationVisitAddress)).value).toHaveLength(2);
  for (let index = 0; index < 25; index++) await left.visit(route(String(index)));
  expect((await readUiRecord(navigationVisitAddress)).value).toHaveLength(20);
  const recent = [...catalog().destinations.map((row, index) => ({ kind: row.kind, key: row.key, at: index })), { kind: "workspace", key: "gone", at: 99 }];
  expect(globalNavigationPicker(catalog(), { visits: recent }).lastVisited).toHaveLength(4);
  expect(globalNavigationPicker(catalog(), { visits: recent, query: "Build" }).lastVisited).toEqual([]);
  await Promise.all([left.dispose(), right.dispose()]);
});
it("P5 visit_history_not_quiet_recent reset clears tab history and fences pending old-account writes", async () => {
  const session = storage();
  const visits = mountNavigationVisits({ storage: session });
  await visits.ready;
  const pending = visits.visit(route());
  resetNavigationVisits(); await wipeUiRecords(); await pending;
  expect(await readUiRecord(navigationVisitAddress)).toBeUndefined();
  const next = mountNavigationVisits({ storage: session }); await next.ready; await next.visit(route());
  expect((await readUiRecord(navigationVisitAddress)).value).toHaveLength(1);
  await next.dispose();
});
