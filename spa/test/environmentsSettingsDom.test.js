// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
const state = vi.hoisted(() => ({ devices: [{ id: "laptop", name: "Laptop" }, { id: "desktop", name: "Desktop" }], support: new Map(), calls: new Map(), changes: new Set(), greetings: new Set(), deviceOwned: false }));
vi.mock("../src/app.js", () => ({ go: vi.fn(), App: { get devices() { return state.devices; } } }));
vi.mock("../src/devices.js", () => ({ readCachedDevices: async () => state.devices, onDevicesChanged: fn => { state.changes.add(fn); return () => state.changes.delete(fn); } }));
vi.mock("../src/core/inboxDevices.js", () => ({ deviceCall: id => (...args) => state.calls.get(id)(...args) }));
vi.mock("../src/core/createWorkspaceWizard.js", () => ({ openCreateWorkspaceWizard: vi.fn() }));
vi.mock("../src/sheets/newRepo.js", () => ({ openNewRepo: vi.fn() }));
vi.mock("../src/sheets/projectSettings.js", () => ({ openProjectSettings: vi.fn() }));
vi.mock("../src/core/changeEvents.js", () => ({ bridgeCapabilities: id => ({ environments: { list: state.support.get(id), archive: true, projectRename: true, environmentRename: true, deleteEnvironment: true } }),
  onBridgeGreeted: fn => { state.greetings.add(fn); return () => state.greetings.delete(fn); } }));
vi.mock("../src/core/scopedSyncRuntime.js", () => ({ deviceScopedSyncOwns: () => state.deviceOwned, wakeScopedSync: vi.fn(async () => []) }));
const confirmDelete = vi.hoisted(() => vi.fn());
vi.mock("../src/core/confirm.js", () => ({ confirmAction: (...args) => confirmDelete(...args) }));
import { wipeUiRecords } from "../src/core/localUiStore.js";
import { renderEnvironmentsSettings } from "../src/views/environmentsSettings.js";
import { openProjectSettings } from "../src/sheets/projectSettings.js";
import { readCached, writeCached, wipeCache } from "../src/core/localCache.js";
import { routeFromHash, hashFromRoute } from "../src/core/router.js";
const address = (deviceId, kind) => ({ deviceId, entityId: "", kind });
const environment = (name = "Sources") => ({ environment_id: "env-1", name, home_path: "/work/source", is_git: true, sources: [{ id: "home" }, { id: "extra" }], project_ids: ["proj-1"] });
const project = { project_id: "proj-1", environment_id: "env-1", name: "Release", is_git: false, added_sources: [{ id: "added" }], workspaces: 2, open_tasks: 3, total_tasks: 5 };
import { createScopedDeviceSyncCache } from "../src/core/scopedDeviceSyncCache.js";
import { deviceRecords } from "./fixtures/deviceSync.js";
import { identityBadgeHtml } from "../src/core/environmentIdentity.js";
let dispose;
async function seed(id, environments = [environment()], projects = [project], supported = true, settingsCached = true) {
  state.support.set(id, supported);
  await writeCached(address(id, "environment-support"), { list: supported });
  await writeCached(address(id, "environments"), environments);
  await writeCached(address(id, "projects"), projects);
  if (settingsCached) await writeCached(address(id, "environment-settings-projects"), projects);
}
const mount = () => renderEnvironmentsSettings({ root: document.body, registerDispose: fn => { dispose = fn; } });
beforeEach(async () => {
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  await wipeCache(); await wipeUiRecords(); confirmDelete.mockResolvedValue(true);
  state.deviceOwned = false;
  document.body.innerHTML = ""; vi.clearAllMocks();
  for (const device of state.devices) state.calls.set(device.id, vi.fn(() => new Promise(() => {})));
});
afterEach(() => { dispose?.(); dispose = null; });
it("routes the account environments destination", () => {
  expect(routeFromHash("#/account/environments")).toEqual({ name: "account", page: "environments" });
  expect(hashFromRoute({ name: "account", page: "environments" })).toBe("#/account/environments");
});
it("paints read-only cached cards per device while pulls are outstanding", async () => {
  await seed("laptop"); await seed("desktop", [environment("Desktop sources")], [{ ...project, open_tasks: 0, total_tasks: 7 }]);
  await mount();
  await vi.waitFor(() => expect(document.querySelectorAll("[data-environment]")).toHaveLength(2));
  for (const text of ["Laptop", "Desktop", "/work/source", "2 sources · 1 projects · 2 workspaces", "+1 folders", "2 workspaces · 3 open tasks", "2 workspaces · 7 tasks"]) expect(document.body.textContent).toContain(text);
  expect(document.body.textContent).not.toMatch(/loading…|Resume|New environment|New project/);
  expect([...document.querySelectorAll("[data-environment-project]")].every(button => button.textContent === "Settings…")).toBe(true);
  document.querySelector('[data-environments-device="desktop"] button').click();
  expect(openProjectSettings).toHaveBeenCalledWith("proj-1", expect.objectContaining({ deviceId: "desktop", callRpc: expect.any(Function) }));
});
it("shows empty devices and environments without projects without Resume", async () => {
  await seed("laptop", [], []); await seed("desktop", [{ ...environment(), project_ids: [] }], []);
  await mount();
  await vi.waitFor(() => expect(document.body.textContent).toContain("No environments on this device yet"));
  expect(document.body.textContent).toContain("no projects yet"); expect(document.body.textContent).not.toContain("Resume");
});
it("retains cached lists after errors and reacts to cache writes safely", async () => {
  await seed("laptop"); await seed("desktop", [], []);
  state.calls.set("laptop", vi.fn(async () => { throw new Error("device unavailable"); }));
  await mount();
  await vi.waitFor(() => expect(document.body.textContent).toContain("Release"));
  expect(document.body.textContent).not.toContain("device unavailable");
  await writeCached(address("laptop", "environments"), [environment("Renamed <source>")]);
  await vi.waitFor(() => expect(document.body.textContent).toContain("Renamed <source>"));
  expect(document.querySelector("source")).toBeNull();
});
it("shows loading before an empty cache resolves and errors only there", async () => {
  state.support.set("laptop", true);
  await writeCached(address("laptop", "environment-support"), { list: true }); await seed("desktop", [], []);
  let fail;
  state.calls.set("laptop", vi.fn(() => new Promise((_resolve, reject) => { fail = reject; })));
  await mount();
  await vi.waitFor(() => expect(fail).toBeTypeOf("function"));
  expect(document.querySelector('[data-environments-device="laptop"]').textContent).toContain("loading…");
  fail(new Error("read refused"));
  await vi.waitFor(() => expect(document.body.textContent).toContain("read refused"));
});
it("shows old-bridge projects and settings without requesting environment.list", async () => {
  await seed("laptop", [], [project], false); await seed("desktop", [], [], false);
  await mount();
  await vi.waitFor(() => expect(document.body.textContent).toContain("Projects (this device needs an update to use environments)"));
  expect(document.body.textContent).toContain("Release"); document.querySelector("button").click();
  expect(openProjectSettings).toHaveBeenCalledWith("proj-1", expect.objectContaining({ deviceId: "laptop" }));
  expect(state.calls.get("laptop").mock.calls.map(([method]) => method)).not.toContain("environment.list");
});
it("pulls each device's lists independently into cache", async () => {
  await seed("laptop", [], []); await seed("desktop", [], []);
  for (const device of state.devices) state.calls.set(device.id, vi.fn(async method => method === "environment.list"
    ? { environments: [environment(device.name + " sources")] } : { projects: [project] }));
  await mount();
  await vi.waitFor(() => expect(document.querySelectorAll("[data-environment]")).toHaveLength(2));
  for (const device of state.devices) expect(state.calls.get(device.id).mock.calls.map(([method]) => method)).toEqual(expect.arrayContaining(["environment.list", "project.list"]));
});
it("updates device labels and removes deactivated devices from cached cards", async () => {
  await seed("laptop"); await seed("desktop"); await mount();
  const original = state.devices;
  try {
    state.devices = [{ id: "laptop", name: "Travel laptop" }];
    state.changes.forEach(listener => listener());
    await vi.waitFor(() => expect(document.querySelectorAll("[data-environments-device]")).toHaveLength(1));
    expect(document.body.textContent).toContain("Travel laptop");
    expect(document.body.textContent).not.toContain("Desktop");
  } finally { state.devices = original; }
});

async function publishJournalProjects(revision = "1", snapshot = true) {
  // project_rows clones config, replaces its chosen name with the path leaf,
  // and publishes sources separately; counts are not part of that journal row.
  const configProject = { id: "proj-1", project_id: "proj-1", project_key: "project:opaque", environment_id: "env-1",
    name: "Release", path: "/work/source", base_branch: "main", next_source: 2 };
  const records = deviceRecords().map(record => record.kind === "project"
    ? { ...record, revision, row: { ...configProject, name: "source" } } : { ...record, revision });
  const writer = createScopedDeviceSyncCache({ deviceId: "laptop", scope: { kind: "device" }, bridgeIdentity: "laptop" });
  const cursor = await writer.readCursor();
  await writer.publish({ records: snapshot ? records : records.filter(record => record.kind === "inbox_task"),
    capture: await writer.capture(), snapshot, expectedCursor: cursor?.cursor,
    response: { status: "ok", scope: { kind: "device" }, incarnation: "journal", cursor: "cursor-" + revision,
      watermark: revision, head: revision, bridge_epoch: "epoch", consistency: "leased" } });
  return (await readCached(address("laptop", "projects"))).value;
}

it("reads chosen names and counts separately from a real device-scoped snapshot without inventing zeroes", async () => {
  state.deviceOwned = true;
  await seed("laptop", [environment()], [], true, false); await seed("desktop", [], []);
  const journal = await publishJournalProjects();
  expect(journal[0].name).toBe("source");
  expect(journal[0]).not.toHaveProperty("workspaces");
  let finish;
  state.calls.set("laptop", vi.fn(method => method === "project.list"
    ? new Promise(resolve => { finish = resolve; }) : new Promise(() => {})));
  await mount();
  const host = document.querySelector('[data-environments-device="laptop"]');
  expect(host.querySelector(".pname").textContent).toContain("source");
  expect(host.textContent).not.toMatch(/0 workspaces|0 tasks/);
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  finish({ projects: [project] });
  await vi.waitFor(() => expect(host.textContent).toContain("2 workspaces · 3 open tasks"));
  expect(host.querySelector(".pname").textContent).toContain("Release");
  expect((await readCached(address("laptop", "projects"))).value).toEqual(journal);
  expect((await readCached(address("laptop", "environment-settings-projects"))).value).toEqual([project]);
});

it("refreshes settings-owned names and counts on reconnect and journal wake while keeping prior paint", async () => {
  state.deviceOwned = true;
  await seed("laptop"); await seed("desktop", [], []);
  await writeCached(address("laptop", "environment-settings-projects"), [project]);
  await publishJournalProjects();
  const answers = [];
  state.calls.set("laptop", vi.fn(method => method === "project.list"
    ? new Promise(resolve => answers.push(resolve)) : new Promise(() => {})));
  await mount();
  await vi.waitFor(() => expect(answers).toHaveLength(1));
  answers.shift()({ projects: [project] });
  await vi.waitFor(() => expect(document.body.textContent).toContain("3 open tasks"));
  state.greetings.forEach(listener => listener("laptop"));
  await vi.waitFor(() => expect(answers).toHaveLength(1));
  expect(document.body.textContent).toContain("3 open tasks");
  answers.shift()({ projects: [{ ...project, name: "Chosen name", workspaces: 4, open_tasks: 7 }] });
  await vi.waitFor(() => expect(document.body.textContent).toContain("4 workspaces · 7 open tasks"));
  await publishJournalProjects("2", false);
  await vi.waitFor(() => expect(answers).toHaveLength(1));
  expect(document.body.textContent).toContain("Chosen name");
  answers.shift()({ projects: [{ ...project, workspaces: 5, open_tasks: 8 }] });
  await vi.waitFor(() => expect(document.body.textContent).toContain("5 workspaces · 8 open tasks"));
  const calls = state.calls.get("laptop").mock.calls.length;
  dispose(); dispose = null;
  state.greetings.forEach(listener => listener("laptop"));
  await publishJournalProjects("3");
  expect(state.calls.get("laptop").mock.calls.length).toBe(calls);
});

it("omits unavailable counts and totals only known workspace counts", async () => {
  await seed("laptop", [environment()], [project, { project_id: "proj-2", environment_id: "env-1", name: "Unknown counts" }]);
  await seed("desktop", [], []); await mount();
  const card = document.querySelector('[data-environment="env-1"]');
  expect(card.textContent).toContain("2 projects · 2 workspaces");
  const unknown = [...card.querySelectorAll(".projrow")].find(row => row.textContent.includes("Unknown counts"));
  expect(unknown.textContent).not.toMatch(/workspaces|tasks/);
});

it("uses shared device tags on every environment and project row, including old bridges and Offline", async () => {
  const original = state.devices;
  try {
    state.devices = [{ id: "laptop", name: "Laptop", status: "online" }, { id: "desktop", name: "Desktop", status: "offline" }];
    await seed("laptop"); await seed("desktop", [], [project], false); await mount();
    expect(document.querySelector('[data-environment] .inbox-device').textContent).toBe("Laptop");
    expect(document.querySelector('[data-environments-device="laptop"] .projrow .inbox-device').textContent).toBe("Laptop");
    expect(document.querySelector('[data-environments-device="desktop"] .projrow .inbox-device').textContent).toBe("Offline");
  } finally { state.devices = original; }
});

it("omits device tags on a single online device", async () => {
  const original = state.devices;
  try {
    state.devices = [{ id: "laptop", name: "Laptop", status: "online" }]; await seed("laptop"); await mount();
    expect(document.querySelector('[data-environment] h3').textContent).not.toContain("Laptop");
    expect(document.querySelectorAll(".inbox-device")).toHaveLength(0);
  } finally { state.devices = original; }
});

it("keeps identity badges in the settings environment and project glyph slots", async () => {
  await seed("laptop"); await seed("desktop", [], []); await mount();
  const card = document.querySelector('[data-environment]');
  for (const [selector,kind,name] of [["h3", "environment",environment().name],[".pname","project",project.name]]) {
    const svg = card.querySelector(selector).firstElementChild;
    expect(svg.outerHTML).toBe(identityBadgeHtml({key:"laptop/env-1",name},kind));
    expect(svg.textContent).toBe("");
    expect(card.querySelector(selector).textContent.trim().startsWith(name)).toBe(true);
  }
});

it.each([
  { workspaces: 23, open_tasks: 17, total_tasks: 19 },
  { workspaces: 0, open_tasks: 0, total_tasks: 0 },
])("ignores retained legacy counts in fallback rows after a lean snapshot: %j", async counts => {
  state.deviceOwned = true;
  await seed("laptop", [environment()], [{ ...project, ...counts }], true, false);
  await seed("desktop", [], []);
  const journal = await publishJournalProjects();
  expect(journal[0]).toMatchObject(counts); // the real projection retains the legacy enrichment
  await mount(); // the settings-owned project.list call remains unresolved
  const card = document.querySelector('[data-environment="env-1"]');
  const row = card.querySelector(".projrow");
  expect(row.querySelector(".pname").textContent).toContain("source");
  expect(row.textContent).not.toMatch(/workspaces|tasks|folders/);
  expect(card.querySelector("p.dim").textContent).toBe("2 sources");
  expect(await readCached(address("laptop", "environment-settings-projects"))).toBeUndefined();
});

it.each([true, false])("keeps absent project lists unknown with cached environment sources known=%s", async sourcesKnown => {
  state.support.set("laptop", true);
  await writeCached(address("laptop", "environment-support"), { list: true });
  const cached = environment();
  if (!sourcesKnown) delete cached.sources;
  await writeCached(address("laptop", "environments"), [cached]);
  await seed("desktop", [], []);
  let refuseProjects;
  state.calls.set("laptop", vi.fn(method => method === "project.list"
    ? new Promise((_resolve, reject) => { refuseProjects = reject; }) : new Promise(() => {})));
  await mount();
  const host = document.querySelector('[data-environments-device="laptop"]');
  expect(host.textContent).not.toMatch(/0 projects|no projects yet|0 workspaces|0 sources/);
  expect(host.querySelector("p.dim").textContent).toBe(sourcesKnown ? "2 sources" : "");
  expect(host.textContent).toContain("loading…");
  await vi.waitFor(() => expect(refuseProjects).toBeTypeOf("function"));
  refuseProjects(new Error("project list refused"));
  await vi.waitFor(() => expect(host.textContent).toContain("project list refused"));
  expect(host.textContent).toContain("Sources"); // retain the cached environment card
  expect(host.textContent).not.toMatch(/loading…|0 projects|no projects yet/);
});


it.each([true, false])("does not claim an empty fallback is authoritative (environments=%s)", async supported => {
  await seed("laptop", [environment()], [], supported, false); await seed("desktop", [], []);
  let finish;
  state.calls.set("laptop", vi.fn(method => method === "project.list"
    ? new Promise(resolve => { finish = resolve; }) : new Promise(() => {})));
  await mount();
  const host = document.querySelector('[data-environments-device="laptop"]');
  expect(host.textContent).not.toMatch(/0 projects|no projects yet/i);
  expect(host.textContent).toContain("loading…");
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  finish({ projects: [] });
  await vi.waitFor(() => expect(host.textContent).toMatch(/no projects yet/i));
  expect(host.textContent).not.toContain("loading…");
  if (supported) expect(host.textContent).toContain("0 projects");
});

async function seedUnknownProjects() {
  state.support.set("laptop", true);
  await writeCached(address("laptop", "environment-support"), { list: true });
  await writeCached(address("laptop", "environments"), [environment()]);
  await seed("desktop", [], []);
}
const projectWake = () => writeCached({ ...address("laptop", "sync-cursor"), entityId: "proj-1", sub: '{"kind":"project_tasks","project_id":"proj-1"}' }, { cursor: "next" });

it.each(["greeting", "journal wake"])("clears a failed empty-list error during a delayed retry from %s", async retry => {
  await seedUnknownProjects();
  let finish;
  let attempts = 0;
  state.calls.set("laptop", vi.fn(method => {
    if (method !== "project.list") return new Promise(() => {});
    if (++attempts === 1) return Promise.reject(new Error("first attempt failed"));
    return new Promise(resolve => { finish = resolve; });
  }));
  await mount();
  const host = document.querySelector('[data-environments-device="laptop"]');
  await vi.waitFor(() => expect(host.textContent).toContain("first attempt failed"));
  if (retry === "greeting") state.greetings.forEach(listener => listener("laptop"));
  else await projectWake();
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  expect(host.textContent).toContain("loading…");
  expect(host.textContent).not.toContain("first attempt failed");
  finish({ projects: [] });
  await vi.waitFor(() => expect(host.textContent).toContain("no projects yet"));
  expect(host.textContent).not.toMatch(/loading…|failed/);
});

it("ignores a superseded pull's late failure while the current retry is pending", async () => {
  await seedUnknownProjects();
  const attempts = [];
  state.calls.set("laptop", vi.fn(method => method === "project.list"
    ? new Promise((resolve, reject) => attempts.push({ resolve, reject })) : new Promise(() => {})));
  await mount();
  await vi.waitFor(() => expect(attempts).toHaveLength(1));
  state.greetings.forEach(listener => listener("laptop"));
  await vi.waitFor(() => expect(attempts).toHaveLength(2));
  attempts[0].reject(new Error("superseded failure"));
  await vi.waitFor(() => expect(state.calls.get("laptop")).toHaveBeenCalled());
  const host = document.querySelector('[data-environments-device="laptop"]');
  // Drain the failed request's cache/read continuations before checking paint.
  await new Promise(resolve => setTimeout(resolve, 0));
  expect(host.textContent).toContain("loading…");
  expect(host.textContent).not.toContain("superseded failure");
  attempts[1].reject(new Error("current failure"));
  await vi.waitFor(() => expect(host.textContent).toContain("current failure"));
});

it("opens cached-capability creation shortcuts and empty-environment Resume with workspace copy", async()=>{
  const {openCreateWorkspaceWizard}=await import("../src/core/createWorkspaceWizard.js");
  await seed("laptop",[{...environment(),project_ids:[]}],[]); await seed("desktop",[],[]);
  await writeCached(address("laptop","environment-support"),{list:true,createProject:true,create:true,addedSources:true});
  await mount(); const host=document.querySelector('[data-environments-device="laptop"]');
  for(const action of ["project","advanced","resume","new"]){
    const button=host.querySelector(`[data-environment-create="${action}"]`); expect(button).not.toBeNull();
    expect(button.textContent).toContain("Creates a workspace"); button.click();
  }
  expect(openCreateWorkspaceWizard.mock.calls.map(([options])=>options)).toEqual([
    expect.objectContaining({deviceId:"laptop",environmentId:"env-1",fixedProject:false}),
    expect.objectContaining({environmentId:"env-1",advancedExpanded:true}),
    expect.objectContaining({environmentId:"env-1"}), expect.objectContaining({newEnvironment:true,deviceId:"laptop"})]);
  expect(host.querySelector('[data-legacy-add-project]')).toBeNull();
});
it("offers legacy Add project only without cached environments.create",async()=>{
  await seed("laptop"); await seed("desktop"); await mount();
  expect(document.querySelectorAll('[data-legacy-add-project]')).toHaveLength(2);
  await writeCached(address("laptop","environment-support"),{list:true,create:true});
  await vi.waitFor(()=>expect(document.querySelector('[data-environments-device="laptop"] [data-legacy-add-project]')).toBeNull());
});
it("keeps M1 environments read-only until environments.create is cached",async()=>{
 await seed('laptop');await seed('desktop',[],[]);
 await writeCached(address('laptop','environment-support'),{list:true,createProject:true,create:false,addedSources:false});
 await mount();const host=document.querySelector('[data-environments-device="laptop"]');
 expect(host.querySelector('[data-environment-create]')).toBeNull();expect(host.querySelector('[data-legacy-add-project]')).not.toBeNull();
});

it.each([{create:true,createProject:false},{create:false,createProject:true}])('keeps a usable settings path for partial support %j',async support=>{
 await seed('laptop');await seed('desktop',[],[]);
 await writeCached(address('laptop','environment-support'),{list:true,...support});await mount();
 const host=document.querySelector('[data-environments-device="laptop"]');
 expect(Boolean(host.querySelector('[data-environment-create="new"]'))).toBe(support.create);
 expect(Boolean(host.querySelector('[data-legacy-add-project]'))).toBe(!support.create);
 const {openCreateWorkspaceWizard}=await import('../src/core/createWorkspaceWizard.js');
 const {openNewRepo}=await import('../src/sheets/newRepo.js');
 if(support.create){host.querySelector('[data-environment-create="new"]').click();expect(openCreateWorkspaceWizard).toHaveBeenCalledWith(expect.objectContaining({newEnvironment:true,deviceId:'laptop'}));}
 else {host.querySelector('[data-legacy-add-project]').click();expect(openNewRepo).toHaveBeenCalledWith(expect.any(Function),expect.objectContaining({deviceId:'laptop'}));}
});

import { openEnvironmentSettings } from "../src/sheets/environmentSettings.js";
it("archived group is collapsed and lists only archived projects", async () => {
  await seed("laptop", [environment()], [project, { ...project, project_id: "old", name: "Old release", archived_at: 123 }]); await seed("desktop", [], []);
  await writeCached(address("laptop", "environment-support"), { list: true, archive: true, projectRename: true, environmentRename: true }); await mount();
  await vi.waitFor(() => expect(document.querySelector("[data-archived-projects]")).not.toBeNull());
  const group = document.querySelector("[data-archived-projects]");
  expect(group.open).toBe(false); expect(group.textContent).toContain("Old release"); expect(group.textContent).not.toContain("Release");
  expect(group.querySelector("[data-restore-project]")).not.toBeNull();
});
it("restore row restores and repaints from cache", async () => {
  await seed("laptop", [environment()], [{ ...project, archived_at: 123 }]); await seed("desktop", [], []);
  await writeCached(address("laptop", "environment-support"), { list: true, archive: true });
  state.calls.set("laptop", vi.fn(async method => method === "project.restore" ? { project: { ...project, archived_at: null } } : new Promise(() => {})));
  await mount(); await vi.waitFor(() => expect(document.querySelector("[data-restore-project]")).not.toBeNull()); document.querySelector("[data-restore-project]").click();
  await vi.waitFor(() => expect(document.querySelector("[data-archived-projects]")).toBeNull());
  expect((await readCached(address("laptop", "projects"))).value[0].archived_at).toBeNull(); expect(document.querySelector("[data-environment-project]")).not.toBeNull();
});
async function mountEnvironmentSheet(projects = [project]) {
  await seed("laptop", [{ ...environment(), sources: [{ id: "home", path: "/work/source", name: "source", is_git: false }] }], projects);
  await writeCached(address("laptop", "environment-support"), { list: true, environmentRename: true });
  document.body.innerHTML = '<div id="scrim"><div id="sheet"></div></div>';
  return openEnvironmentSettings("env-1", { deviceId: "laptop", callRpc: state.calls.get("laptop") });
}
it("environment sheet renames and keeps home path read only", async () => {
  state.calls.set("laptop", vi.fn(async (method, params) => method === "environment.rename" ? { ...environment(params.name), sources: [] } : new Promise(() => {})));
  await mountEnvironmentSheet(); await vi.waitFor(() => expect(document.querySelector("#esdisplay")).not.toBeNull());
  expect(document.querySelector("#eshome").readOnly).toBe(true); document.querySelector("#esdisplay").value = "Renamed"; document.querySelector("#esrename").click();
  await vi.waitFor(() => expect((document.querySelector("#esdisplay")).value).toBe("Renamed"));
  await vi.waitFor(async () => expect((await readCached(address("laptop", "environments"))).value[0].name).toBe("Renamed"));
  expect(state.calls.get("laptop")).toHaveBeenCalledWith("environment.rename", { environment_id: "env-1", name: "Renamed" });
});
it("shared source row names the other project", async () => {
  await mountEnvironmentSheet([project, { ...project, project_id: "p2", name: "P₂", sources: [{ id: "home", path: "/work/source" }] }]);
  await vi.waitFor(() => expect(document.querySelector("#sheet").textContent).toContain("also used by P₂"));
});

it("environment sources save against environment verbs and repaint their reply", async () => {
  const updated = { ...environment(), sources: [{ id: "home", name: "Updated", path: "/work/source", is_git: false }] };
  const call = vi.fn(async method => method === "environment.update_source" ? updated : new Promise(() => {}));
  state.calls.set("laptop", call); await mountEnvironmentSheet();
  await vi.waitFor(() => expect(document.querySelector("[data-save-source]")).not.toBeNull());
  const input = document.querySelector('input[data-field="name"]');
  input.value = "Updated"; input.dispatchEvent(new Event("input")); document.querySelector("[data-save-source]").click();
  await vi.waitFor(async () => expect((await readCached(address("laptop", "environments"))).value[0].sources[0].name).toBe("Updated"));
  expect(call).toHaveBeenCalledWith("environment.update_source", { environment_id: "env-1", source_id: "home", name: "Updated" });
  await vi.waitFor(() => expect(document.querySelector("[data-source-status]").textContent).toBe("Saved."));
  expect(document.querySelector('[data-field="path"]').readOnly).toBe(true);
});
it("environment isolation sends its exact nullable override", async () => {
  const call = vi.fn(async (method, params) => method === "environment.set_isolation" ? { ...environment(), isolation: params.isolation } : new Promise(() => {}));
  state.calls.set("laptop", call); await mountEnvironmentSheet();
  await vi.waitFor(() => expect(document.querySelector("#esisolation")?.disabled).toBe(false));
  const select = document.querySelector("#esisolation"); select.value = "rift"; select.dispatchEvent(new Event("change"));
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("environment.set_isolation", { environment_id: "env-1", isolation: "rift" }));
  await vi.waitFor(async () => expect((await readCached(address("laptop", "environments"))).value[0].isolation).toBe("rift"));
  await vi.waitFor(() => expect(document.querySelector("#esisolation").value).toBe("rift"));
  const inherit = document.querySelector("#esisolation"); inherit.value = ""; inherit.dispatchEvent(new Event("change"));
  await vi.waitFor(() => expect(call).toHaveBeenCalledWith("environment.set_isolation", { environment_id: "env-1", isolation: null }));
});

vi.mock("../src/core/deviceContexts.js", async original => ({ ...await original(), whenGreeted: async (_context, send) => ({ sent: send() }) }));


it("hides environment deletion on old bridges", async () => {
  await mountEnvironmentSheet();
  await vi.waitFor(() => expect(document.querySelector("#esdisplay")).not.toBeNull());
  expect(document.querySelector("#esdelete")).toBeNull();
});
it("deletes an environment at successful reply and preserves refusal verbatim across repaint and retry", async () => {
  let finish;
  let attempts = 0;
  const callRpc = vi.fn(method => {
    if (method !== "environment.delete") return new Promise(() => {});
    if (++attempts === 1) return Promise.reject(new Error("Delete its 2 projects first"));
    return new Promise(resolve => { finish = resolve; });
  });
  state.calls.set("laptop", callRpc);
  await mountEnvironmentSheet();
  await writeCached(address("laptop", "environment-support"), { environmentRename: true, deleteEnvironment: true });
  await vi.waitFor(() => expect(document.querySelector("#esdelete")).not.toBeNull());
  document.querySelector("#esdelete").click();
  await vi.waitFor(() => expect(document.querySelector("#esdeleteerr").textContent).toBe("Delete its 2 projects first"));
  document.querySelector("#esclose").click();
  openEnvironmentSettings("env-1", { deviceId: "laptop", callRpc });
  await vi.waitFor(() => expect(document.querySelector("#esdelete")).not.toBeNull());
  document.querySelector("#esdelete").click();
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  await writeCached(address("laptop", "environments"), [{ ...environment(), name: "Repaint" }]);
  await vi.waitFor(() => expect(document.querySelector("#esdisplay").value).toBe("Repaint"));
  expect(document.querySelector("#esdelete").disabled).toBe(true);
  expect((await readCached(address("laptop", "environments"))).value).toHaveLength(1);
  finish({}); // Exact already-absent response on b409dcf40.
  await vi.waitFor(async () => expect((await readCached(address("laptop", "environments"))).value).toEqual([]));
  expect(document.querySelector("#scrim").classList.contains("show")).toBe(false);
  expect(document.querySelector("#esdeleteerr")?.textContent || "").toBe("");
  const deletes = callRpc.mock.calls.filter(([method]) => method === "environment.delete");
  expect(deletes).toHaveLength(2); expect(deletes[1]).toEqual(deletes[0]);
  expect(deletes[0]).toEqual(["environment.delete", { environment_id: "env-1" }, { operationId: expect.any(String) }]);
});
