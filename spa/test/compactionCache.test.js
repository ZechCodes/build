import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { readCached, wipeCache, writeCached } from "../src/core/localCache.js";
import { create, capabilitiesOf, parseEvent } from "../src/core/bridgeApi/v1/index.js";
import { readCompactionSupport, rememberCompactionSupport } from "../src/core/compactionSupport.js";
import { refreshCompactionCaches } from "../src/core/compactionCache.js";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const capability = "compaction.harnessThresholds";
const capabilities = { compaction: { harnessThresholds: true }, agents: { projectWorkers: true } };
const greeting = { api_version: "3.22.0", capabilities: [capability] };
const address = (kind, entityId = "") => ({ deviceId: "device", entityId, kind });
const observation = { configured: { tokens: 100, source: { kind: "project", id: "project" } }, active: null,
  enforcement: "unsupported", pending_change: null, reason: "cli_unavailable", future: { retained: true } };
const agent = { id: "agent", conversation_id: "conversation", compaction: observation };
const context = (call, active = () => true) => ({ deviceId: "device", call, capabilities, active });
beforeEach(async () => { await wipeCache(); });

it("gates thresholds independently by name and accepts its invalidation event", () => {
  expect(capabilitiesOf(greeting).compaction).toEqual({ harnessThresholds: true });
  expect(capabilitiesOf({ api_version: "3.22.0", capabilities: [] }).compaction).toEqual({ harnessThresholds: false });
  const event = { type: "compaction.changed", scope: "device", owner_id: null };
  expect(parseEvent(event)).toEqual(event);
});

it("persists latest support including downgrade, refusing superseded storage writes", async () => {
  await rememberCompactionSupport("device", capabilities);
  expect(await readCompactionSupport("device")).toEqual({ harnessThresholds: true });
  await rememberCompactionSupport("device", {}, () => false);
  expect(await readCompactionSupport("device")).toEqual({ harnessThresholds: true });
  await rememberCompactionSupport("device", {});
  expect(await readCompactionSupport("device")).toEqual({ harnessThresholds: false });
});

it("admits exact new setters only on capable current adapters; old bridges receive no new shape", async () => {
  const call = vi.fn(async () => ({ compaction: observation }));
  const older = create(call, { ...greeting, capabilities: [] });
  for (const method of ["project.set_compaction", "workspace.set_compaction", "settings.set"]) {
    await expect(older.call(method, { compaction_thresholds: { codex: 100 } })).rejects.toMatchObject({ code: "unknown_method" });
  }
  expect(call).not.toHaveBeenCalled();
  const params = { compaction_thresholds: { codex: 100, pi: null } };
  expect(await create(call, greeting).call("settings.set", params)).toEqual({ compaction: observation });
  expect(call).toHaveBeenCalledWith("settings.set", params);
});

it.each([
  ["settings.set", { compact_above_tokens: 0 }],
  ["settings.set", { compaction_thresholds: { codex: 0 } }],
  ["project.set_compaction", { compaction_thresholds: { pi: 0 } }],
  ["workspace.set_compaction", { compaction_thresholds: { claude: 0 } }],
  ["conversation.settings", { max_context_tokens: 0 }],
])("rejects zero before sending %s", async (method, params) => {
  const call = vi.fn();
  await expect(create(call, greeting).call(method, params)).rejects.toMatchObject({ code: "invalid_params" });
  expect(call).not.toHaveBeenCalled();
});

it("refreshes held settings and agent observations from authority, retaining other fields", async () => {
  await writeCached(address("settings"), { compaction_thresholds: {} });
  await writeCached(address("project-settings", "project"), { project_id: "project" });
  await writeCached(address("workspace-settings", "workspace"), { workspace_id: "workspace" });
  await writeCached(address("row", "owner"), { entity_id: "owner", label: "keep", agents: [{ id: "agent", conversation_id: "conversation", state: "working" }] });
  await writeCached(address("project-agents", "owner"), { entity_id: "owner", pinned_agent_id: "agent", agents: [{ id: "agent", state: "working" }] });
  const settings = { compaction_thresholds: { codex: 100 }, compaction: { inherited: {}, effective: {} } };
  const call = vi.fn(async (method) => ({
    "settings.get": settings,
    "project.list": { projects: [{ project_id: "project", ...settings }] },
    "workspace.get": { workspace_id: "workspace", ...settings },
    "agent.list": { entity_id: "owner", agents: [agent] },
  })[method]);
  await refreshCompactionCaches(context(call), { scope: "device", owner_id: null });
  expect((await readCached(address("settings"))).value).toEqual(settings);
  expect((await readCached(address("project-settings", "project"))).value).toEqual({ project_id: "project", ...settings });
  expect((await readCached(address("workspace-settings", "workspace"))).value).toEqual({ workspace_id: "workspace", ...settings });
  expect((await readCached(address("row", "owner"))).value).toMatchObject({ label: "keep", agents: [{ state: "working", compaction: observation }] });
  expect((await readCached(address("project-agents", "owner"))).value.agents[0].compaction).toEqual(observation);
  expect(call).toHaveBeenCalledWith("agent.list", { entity_id: "owner", include_project_workers: true });
});

it("a newer cache write or retired greeting fences an in-flight observation", async () => {
  const addr = address("row", "owner");
  await writeCached(addr, { agents: [{ id: "agent", compaction: observation }] });
  let finish;
  const pending = refreshCompactionCaches(context(() => new Promise((resolve) => { finish = resolve; })), { scope: "conversation", owner_id: "agent" });
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  const newer = { agents: [{ id: "agent", compaction: { configured: { tokens: 300 } } }] };
  await writeCached(addr, newer);
  finish({ agents: [agent] });
  await pending;
  expect((await readCached(addr)).value).toEqual(newer);
  let active = true;
  finish = null;
  const retired = refreshCompactionCaches(context(() => new Promise((resolve) => { finish = resolve; }), () => active), { scope: "conversation", owner_id: "agent" });
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  active = false;
  finish({ agents: [agent] });
  await retired;
  expect((await readCached(addr)).value).toEqual(newer);
});

it("ignores pushes on an incapable greeting and invalid scope payloads", async () => {
  const call = vi.fn();
  await refreshCompactionCaches({ ...context(call), capabilities: {} }, { scope: "device", owner_id: null });
  await refreshCompactionCaches(context(call), { scope: "alien", owner_id: null });
  expect(call).not.toHaveBeenCalled();
});

it("one capable subscription catches up settings after acceptance and other-client invalidations", async () => {
  const { greetBridge, dispatchChangeEvent, resetChangeEvents } = await import("../src/core/changeEvents.js");
  resetChangeEvents();
  await writeCached(address("settings"), { compaction_thresholds: { codex: 10 } });
  let tokens = 100;
  let accept;
  const call = vi.fn(async (method) => {
    if (method === "session.hello") return { ...greeting, push_events: true, capabilities: [capability, "changes.subscriptions"] };
    if (method === "changes.subscribe") return new Promise((resolve) => { accept = resolve; });
    if (method === "settings.get") return { compaction_thresholds: { codex: tokens } };
    return {};
  });
  await greetBridge(call, { deviceId: "device" });
  await vi.waitFor(() => expect(accept).toBeTypeOf("function"));
  expect(call.mock.calls.filter(([method]) => method === "session.hello")).toHaveLength(1);
  expect(call.mock.calls[0][1]).not.toHaveProperty("capabilities");
  expect(call).toHaveBeenCalledWith("changes.subscribe", expect.objectContaining({ scope: { kind: "all" }, kinds: ["compaction.changed"] }));
  expect(call).not.toHaveBeenCalledWith("settings.get", {});
  accept({});
  await vi.waitFor(async () => expect((await readCached(address("settings"))).value.compaction_thresholds.codex).toBe(100));
  tokens = 200;
  dispatchChangeEvent({ type: "compaction.changed", scope: "project", owner_id: "project" }, "device");
  await vi.waitFor(async () => expect((await readCached(address("settings"))).value.compaction_thresholds.codex).toBe(200));
  const older = vi.fn(async () => ({ ...greeting, capabilities: ["changes.subscriptions"] }));
  await greetBridge(older, { deviceId: "device" });
  await vi.waitFor(async () => expect(await readCompactionSupport("device")).toEqual({ harnessThresholds: false }));
  expect(older.mock.calls.filter(([method]) => method === "changes.subscribe")).toHaveLength(0);
  resetChangeEvents();
});

it("a later invalidation wins even while the first authoritative reply is in flight", async () => {
  const addr = address("settings");
  await writeCached(addr, { compaction_thresholds: { codex: 10 } });
  let finish;
  const first = refreshCompactionCaches(context(() => new Promise((resolve) => { finish = resolve; })), { scope: "device", owner_id: null });
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  await refreshCompactionCaches(context(async () => ({ compaction_thresholds: { codex: 300 } })), { scope: "project", owner_id: "project" });
  finish({ compaction_thresholds: { codex: 100 } });
  await first;
  expect((await readCached(addr)).value.compaction_thresholds.codex).toBe(300);
});

it("an invalidation marks ordinary sync pulls as older and settings pulls cannot replace its result", async () => {
  const { watchSettingsRecord } = await import("../src/core/settingsRecords.js");
  const { pushFence, pushedSince } = await import("../src/core/pushFence.js");
  const addr = address("settings");
  await writeCached(addr, { compaction_thresholds: { codex: 10 } });
  const record = watchSettingsRecord(addr, () => {});
  const fence = pushFence();
  let finish;
  const stale = record.pull(() => new Promise((resolve) => { finish = resolve; }));
  await vi.waitFor(() => expect(finish).toBeTypeOf("function"));
  await refreshCompactionCaches(context(async () => ({ compaction_thresholds: { codex: 200 } })), { scope: "device", owner_id: null });
  expect(pushedSince(addr, fence)).toBeTruthy();
  finish({ compaction_thresholds: { codex: 100 } });
  await stale;
  expect((await readCached(addr)).value.compaction_thresholds.codex).toBe(200);
  record.dispose();
});

it("saving a conversation can update inheritance without inventing a different active observation", async () => {
  const { createCompactionChoice } = await import("../src/core/conversationCompaction.js");
  const active = { tokens: 150, source: { kind: "conversation", id: "conversation" } };
  let held = { id: "agent", max_context_tokens: 150, compaction: { ...observation, active } };
  const inherited = { tokens: 250, source: { kind: "project", id: "project" } };
  const choice = createCompactionChoice({ call: async () => ({ max_context_tokens: null, compaction: { inherited } }),
    capture: async () => ({}), write: async (_capture, _agent, rewrite) => { held = rewrite(held); } });
  await choice.choose({ entityId: "owner", agent: held, maxContextTokens: null });
  expect(held.compaction.active).toEqual(active);
  expect(held.compaction.inherited).toEqual(inherited);
});

it("fences retired authority again inside a delayed cache transaction", async () => {
  const cache = await import("../src/core/localCache.js");
  const addr = address("settings");
  await writeCached(addr, { compaction_thresholds: { codex: 10 } });
  let active = true;
  let release;
  const merge = cache.mergeCachedRecordsTogether;
  const spy = vi.spyOn(cache, "mergeCachedRecordsTogether").mockImplementation(async (...args) => {
    await new Promise((resolve) => { release = resolve; });
    return merge(...args);
  });
  try {
    const pending = refreshCompactionCaches(context(async () => ({ compaction_thresholds: { codex: 200 } }), () => active), { scope: "device", owner_id: null });
    await vi.waitFor(() => expect(release).toBeTypeOf("function"));
    active = false;
    release();
    await pending;
    expect((await readCached(addr)).value.compaction_thresholds.codex).toBe(10);
  } finally { release?.(); spy.mockRestore(); }
});

it("rejects unsafe integers before sending any compaction write", async () => {
  const call = vi.fn();
  await expect(create(call, greeting).call("settings.set", { compaction_thresholds: { pi: Number.MAX_SAFE_INTEGER + 1 } }))
    .rejects.toMatchObject({ code: "invalid_params" });
  expect(call).not.toHaveBeenCalled();
});

it("clears another client's removed conversation override from held agent digests", async () => {
  const addr = address("row", "owner");
  await writeCached(addr, { agents: [{ id: "agent", max_context_tokens: 150000, compact_at_tokens: 150000 }] });
  const compaction = { ...observation, configured: { tokens: 200000, source: { kind: "project", id: "project" } } };
  await refreshCompactionCaches(context(async () => ({ agents: [{ id: "agent", compact_at_tokens: 200000, compaction }] })), { scope: "conversation", owner_id: "agent" });
  expect((await readCached(addr)).value.agents[0]).toMatchObject({ max_context_tokens: null, compact_at_tokens: 200000, compaction });
});

it("invalidating a cold scope fills its authoritative settings record", async () => {
  const settings = { workspace_id: "workspace", compaction_thresholds: { codex: 100 }, compaction: { inherited: {}, effective: {} } };
  const call = vi.fn(async () => settings);
  await refreshCompactionCaches(context(call), { scope: "workspace", owner_id: "workspace" });
  expect(call).toHaveBeenCalledWith("workspace.get", { workspace_id: "workspace" });
  expect((await readCached(address("workspace-settings", "workspace"))).value).toEqual(settings);
});

it("coalesces a burst into one trailing pass and retains every invalidated cold scope", async () => {
  const { invalidateCompactionCaches } = await import("../src/core/compactionCache.js");
  const replies = [];
  const call = vi.fn((method) => {
    if (method === "settings.get") return new Promise((resolve) => { replies.push(resolve); });
    if (method === "project.list") return Promise.resolve({ projects: [{ project_id: "cold-project", compaction_thresholds: {} }] });
    return Promise.resolve({ workspace_id: "cold-workspace", compaction_thresholds: {} });
  });
  const ctx = context(call);
  const first = invalidateCompactionCaches(ctx, { scope: "device", owner_id: null });
  await vi.waitFor(() => expect(replies).toHaveLength(1));
  const burst = Array.from({ length: 10 }, () => invalidateCompactionCaches(ctx, { scope: "project", owner_id: "cold-project" }));
  burst.push(invalidateCompactionCaches(ctx, { scope: "workspace", owner_id: "cold-workspace" }));
  expect(call.mock.calls.filter(([method]) => method === "settings.get")).toHaveLength(1);
  replies[0]({ compaction_thresholds: { codex: 100 } });
  await vi.waitFor(() => expect(replies).toHaveLength(2));
  replies[1]({ compaction_thresholds: { codex: 200 } });
  await Promise.all([first, ...burst]);
  expect(call.mock.calls.filter(([method]) => method === "settings.get")).toHaveLength(2);
  expect((await readCached(address("project-settings", "cold-project"))).value.project_id).toBe("cold-project");
  expect((await readCached(address("workspace-settings", "cold-workspace"))).value.workspace_id).toBe("cold-workspace");
});

it.each([
  ["settings", "", "device"],
  ["project-settings", "project", "project"],
  ["workspace-settings", "workspace", "workspace"],
])("a pre-invalidation %s pull cannot consume the authoritative refresh's cache stamp", async (kind, owner, scope) => {
  const { watchSettingsRecord } = await import("../src/core/settingsRecords.js");
  const addr = address(kind, owner);
  const value = (tokens) => ({ project_id: "project", workspace_id: "workspace", compaction_thresholds: { codex: tokens } });
  await writeCached(addr, value(50));
  const record = watchSettingsRecord(addr, () => {});
  let finishOld;
  let finishFresh;
  try {
    const old = record.pull(() => new Promise((resolve) => { finishOld = resolve; }));
    await vi.waitFor(() => expect(finishOld).toBeTypeOf("function"));
    const fresh = refreshCompactionCaches(context(() => new Promise((resolve) => { finishFresh = resolve; })),
      { scope, owner_id: owner || null });
    await vi.waitFor(() => expect(finishFresh).toBeTypeOf("function"));
    finishOld(value(100));
    await old;
    finishFresh(scope === "project" ? { projects: [value(200)] } : value(200));
    await fresh;
    expect((await readCached(addr)).value.compaction_thresholds.codex).toBe(200);
  } finally { record.dispose(); }
});

it("a pre-invalidation project roster cannot consume the authoritative agent refresh's cache stamp", async () => {
  const { syncProjectAgents } = await import("../src/core/projectAgentsCache.js");
  const addr = address("project-agents", "owner");
  const roster = (tokens) => ({ entity_id: "owner", pinned_agent_id: "agent",
    agents: [{ id: "agent", compaction: { configured: { tokens }, active: null, pending_change: null } }] });
  await writeCached(addr, roster(50));
  await writeCached(address("row", "owner"), roster(50));
  let finishOld;
  let finishFresh;
  const old = syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities,
    call: () => new Promise((resolve) => { finishOld = resolve; }) });
  await vi.waitFor(() => expect(finishOld).toBeTypeOf("function"));
  const fresh = refreshCompactionCaches(context(() => new Promise((resolve) => { finishFresh = resolve; })),
    { scope: "conversation", owner_id: "agent" });
  await vi.waitFor(() => expect(finishFresh).toBeTypeOf("function"));
  finishOld(roster(100));
  await old;
  finishFresh(roster(200));
  await fresh;
  expect((await readCached(addr)).value.agents[0].compaction.configured.tokens).toBe(200);
  expect((await readCached(address("row", "owner"))).value.agents[0].compaction.configured.tokens).toBe(200);
});

it("an ordinary settings read begun after invalidation retains its newer answer", async () => {
  const { watchSettingsRecord } = await import("../src/core/settingsRecords.js");
  const addr = address("settings");
  await writeCached(addr, { compaction_thresholds: { codex: 50 } });
  let finishFresh;
  const fresh = refreshCompactionCaches(context(() => new Promise((resolve) => { finishFresh = resolve; })),
    { scope: "device", owner_id: null });
  await vi.waitFor(() => expect(finishFresh).toBeTypeOf("function"));
  const record = watchSettingsRecord(addr, () => {});
  try {
    await record.pull(async () => ({ compaction_thresholds: { codex: 300 } }));
    finishFresh({ compaction_thresholds: { codex: 200 } });
    await fresh;
    expect((await readCached(addr)).value.compaction_thresholds.codex).toBe(300);
  } finally { record.dispose(); }
});

it("an ordinary roster read begun after invalidation retains its newer observation", async () => {
  const { syncProjectAgents } = await import("../src/core/projectAgentsCache.js");
  const addr = address("project-agents", "owner");
  const roster = (tokens) => ({ entity_id: "owner", pinned_agent_id: "agent",
    agents: [{ id: "agent", compaction: { configured: { tokens }, active: null } }] });
  await writeCached(addr, roster(50));
  let finishFresh;
  const fresh = refreshCompactionCaches(context(() => new Promise((resolve) => { finishFresh = resolve; })),
    { scope: "conversation", owner_id: "agent" });
  await vi.waitFor(() => expect(finishFresh).toBeTypeOf("function"));
  await syncProjectAgents({ deviceId: "device", entityId: "owner", capabilities, call: async () => roster(300) });
  finishFresh(roster(200));
  await fresh;
  expect((await readCached(addr)).value.agents[0].compaction.configured.tokens).toBe(300);
});
