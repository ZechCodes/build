import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import { deviceRecord, deviceRecords, projectOwnerRecords } from "./fixtures/deviceSync.js";

let cache, sync, mutations;
const deviceId = "synthetic-device", scope = { kind: "device" };
const address = (kind, entityId = "", sub = "") => ({ deviceId, entityId, kind, sub });
const raw = async (kind, entityId = "", sub = "") => (await cache.readCached(address(kind, entityId, sub)))?.value;
const response = (cursor = "cursor-1") => ({ status: "ok", scope, incarnation: "incarnation", cursor, head: "2", watermark: "1", bridge_epoch: "epoch", consistency: "leased", lease: { id: "lease-1", expires_at_ms: 1793534700000 } });
const create = (options = {}) => sync.createScopedDeviceSyncCache({ deviceId, scope, bridgeIdentity: deviceId, ...options });
const publish = async (writer, records, options = {}) => writer.publish({ records, capture: await writer.capture(), response: response(), ...options });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  sync = await import("../src/core/scopedDeviceSyncCache.js");
  mutations = await import("../src/core/taskMutationFence.js");
});

it("projects every rendered device family into the ordinary inbox and directory caches atomically", async () => {
  const writer = create();
  expect(await publish(writer, deviceRecords(), { snapshot: true })).toBe(true);
  expect(await raw("projects")).toMatchObject([{ id: "proj-1", projectKey: "synthetic-device/proj-1", sources: [{ id: "source-1" }] }]);
  expect(await raw("device-projects")).toMatchObject([{ project_id: "proj-1", sources: [{ id: "source-1" }] }]);
  expect(await raw("workspaces")).toMatchObject([{ id: "workspace-1", entity_id: "owner-1", directories: [{ id: "dir-1", branch: "synthetic" }], lifecycle: { size_bytes: 200 } }]);
  expect(await raw("row", "owner-1")).toMatchObject({ entity_id: "owner-1", agents: [{ id: "agent-1", unread_count: 2 }] });
  expect((await raw("feed")).items).toEqual(expect.arrayContaining([expect.objectContaining({ kind: "capture", capture_id: "capture-1", title: "Synthetic capture" })]));
  expect(await raw("tracker-summary", "proj-1", "task-1")).toMatchObject({ task: { id: "task-1", title: "Synthetic task", watched: true } });
  expect(await raw("user-session")).toMatchObject({ last_activity_ms: 1793534400000 });
  expect((await raw("workspaces"))[0].retained_last_activity_ms).toBe(1793534300000);
  expect(await raw("archive")).toMatchObject({ items: [{ worktree_id: "archive-1", title: "Synthetic archive" }] });
  expect(await writer.readCursor()).toMatchObject({ cursor: "cursor-1", appliedHead: "1" });
});

it("publishes identities in the established summary-query consumer shape", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const summaries = await import("../src/core/taskSummaryCache.js");
  const place = { deviceId, projectId: "proj-1" };
  await cache.writeCached(summaries.summaryQueryAddress(place, {}), { ids: ["task-1"], identity_ids: ["agent-1"], complete: true });
  expect((await summaries.readSummaryQuery(place, {})).identities["agent-1"]).toMatchObject({ agent_id: "agent-1", name: "Synthetic agent" });
});

it("identity role updates retain held runtime availability and hydrate the paired agent's known workspace link", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const target = address("tracker-summary-identity", "proj-1", "agent-1");
  await cache.writeCached(target, { identity: { agent_id: "agent-1", name: "Held name", workspace_id: "workspace-1", available: true }, read_order: 1 });
  const identity = deviceRecords().find((record) => record.kind === "identity");
  expect(await publish(writer, [{ ...identity, revision: "2" }], { expectedCursor: "cursor-1", response: response("identity-role") })).toBe(true);
  expect((await cache.readCached(target)).value.identity).toMatchObject({ name: "Synthetic agent", project_role: null, available: true, workspace_id: "workspace-1" });
  const rawIdentity = (await cache.readCached(address("sync-device-record", "", JSON.stringify(["identity", "agent-1"])))).value;
  expect(rawIdentity.row.workspace_id).toBeNull();
  expect(Object.hasOwn(rawIdentity.row, "available")).toBe(false);
});

it("device agent publication uses the explicit later-ordinal coordinator pin and persisted roles", async () => {
  const writer = create();
  const rosterAddress = address("project-agents", "project-owner");
  await cache.writeCached(rosterAddress, { entity_id: "project-owner", pinned_agent_id: "worker", agents: [] });
  await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
  expect(await publish(writer, [...deviceRecords(), ...projectOwnerRecords()], { snapshot: true })).toBe(true);
  expect(await raw("project-agents", "project-owner")).toMatchObject({ pinned_agent_id: "coordinator", agents: [
    { id: "worker", project_role: "worker" }, { id: "coordinator", project_role: "coordinator" },
  ] });
  const { cachedFeedView } = await import("../src/core/cachedRows.js");
  const { projectAgentEntries } = await import("../src/core/inboxProjectAgent.js");
  const view = await cachedFeedView(deviceId);
  expect(projectAgentEntries(view.projects, view.items)[0].name).toBe("Coordinator");
  expect(await raw("project-agents", "owner-1")).toBeUndefined();
});

it.each([null, "missing"])("a %s project pin never falls back to the first worker", async (pin) => {
  const writer = create();
  await cache.writeCached(address("project-agents-support"), { projectWorkers: true });
  await publish(writer, [...deviceRecords(), ...projectOwnerRecords({ pin })], { snapshot: true });
  const roster = await raw("project-agents", "project-owner");
  expect(roster.pinned_agent_id).toBe(pin);
  const { cachedFeedView } = await import("../src/core/cachedRows.js");
  const { projectAgentEntries } = await import("../src/core/inboxProjectAgent.js");
  const view = await cachedFeedView(deviceId);
  expect(projectAgentEntries(view.projects, view.items)[0].name).toBe("Synthetic project");
});

it("null roles remain null even when an owner has an explicit pin", async () => {
  const writer = create();
  await publish(writer, [...deviceRecords(), ...projectOwnerRecords({ roles: false })], { snapshot: true });
  expect((await raw("project-agents", "project-owner")).agents.map((agent) => agent.project_role)).toEqual([null, null]);
});

const legacyUnlinkedOwner = () => {
  const records = projectOwnerRecords({ pin: null, roles: false }).filter((record) => record.key !== "coordinator");
  return records.map((record) => {
    const row = { ...record.row, owner_id: "legacy-owner" };
    if (record.kind === "agent") Object.assign(row, { id: "legacy-agent", conversation_id: "legacy-agent" });
    if (record.kind === "identity") Object.assign(row, { agent_id: "legacy-agent" });
    return deviceRecord(record.kind, record.kind === "owner_session" ? "legacy-owner" : "legacy-agent", row);
  });
};

it.each(["coordinator", null])("an unrelated null-workspace legacy owner cannot steal a project owner proven by pin=%s or roles", async (pin) => {
  const writer = create();
  await cache.writeCached(address("project-agents", "legacy-owner"), { entity_id: "legacy-owner", pinned_agent_id: null, agents: [] });
  await publish(writer, [...deviceRecords(), ...legacyUnlinkedOwner(), ...projectOwnerRecords({ pin })], { snapshot: true });
  expect((await raw("projects"))[0].entity_id).toBe("project-owner");
  expect(await raw("project-agents", "legacy-owner")).toBeUndefined();
  expect(await raw("project-agents", "project-owner")).toMatchObject({ pinned_agent_id: pin });
  expect(await raw("row", "legacy-owner")).toBeUndefined();
});

it("an unresolved legacy null-role project keeps its held directory owner without promoting unrelated owners", async () => {
  const writer = create();
  await cache.writeCached(address("projects"), [{ id: "proj-1", project_id: "proj-1", entity_id: "project-owner" }]);
  await cache.writeCached(address("row", "legacy-owner"), { kind: "plan", entity_id: "legacy-owner", agents: [] });
  await publish(writer, [...deviceRecords(), ...legacyUnlinkedOwner(), ...projectOwnerRecords({ pin: null, roles: false })], { snapshot: true });
  expect((await raw("projects"))[0].entity_id).toBe("project-owner");
  expect(await raw("project-agents", "project-owner")).toMatchObject({ pinned_agent_id: null });
  expect(await raw("project-agents", "legacy-owner")).toBeUndefined();
  expect((await raw("row", "legacy-owner")).kind).toBe("plan");
});

it("a live owner session with no agent keeps its exact conversation anchor until its tombstone", async () => {
  const writer = create();
  const owner = projectOwnerRecords({ pin: "missing-agent" }).find((record) => record.kind === "owner_session");
  owner.row.session_started_ms = 1793534400000; owner.row.last_activity_ms = 1793534401000;
  await publish(writer, [...deviceRecords(), owner], { snapshot: true });
  expect(await raw("row", "project-owner")).toMatchObject({ entity_id: "project-owner", agents: [],
    session_started_ms: owner.row.session_started_ms, last_activity_ms: owner.row.last_activity_ms });
  expect(await raw("project-agents", "project-owner")).toMatchObject({ pinned_agent_id: "missing-agent", agents: [] });
  const { cachedFeedView } = await import("../src/core/cachedRows.js");
  const { projectAgentEntries } = await import("../src/core/inboxProjectAgent.js");
  const view = await cachedFeedView(deviceId);
  expect(projectAgentEntries(view.projects, view.items)[0].anchorMs).toBe(owner.row.session_started_ms);
  await publish(writer, [deviceRecord("owner_session", "project-owner", null, "2", { operation: "delete", member: false })],
    { expectedCursor: "cursor-1", response: response("owner-deleted") });
  expect(await raw("row", "project-owner")).toBeUndefined();
  expect((await raw("feed")).items.some((row) => row.entity_id === "project-owner")).toBe(false);
});

it("re-pinning republishes the existing identity role join and owner pin without an agent-row refresh", async () => {
  const writer = create();
  await publish(writer, [...deviceRecords(), ...projectOwnerRecords({ pin: "worker" })], { snapshot: true });
  const roleAndPinChanges = projectOwnerRecords().filter((record) => record.kind === "identity" || record.kind === "owner_session")
    .map((record) => ({ ...record, revision: "2" }));
  await publish(writer, roleAndPinChanges, { expectedCursor: "cursor-1", response: response("repinned") });
  expect(await raw("project-agents", "project-owner")).toMatchObject({ pinned_agent_id: "coordinator", agents: [
    { id: "worker", project_role: "worker" }, { id: "coordinator", project_role: "coordinator" },
  ] });
});

it("a project-owner markSeen repairs its legacy pin-only row and an older device publication cannot undo it", async () => {
  const writer = create();
  await publish(writer, [...deviceRecords(), ...projectOwnerRecords()], { snapshot: true });
  const ownerAddress = address("row", "project-owner");
  const seeded = await raw("row", "project-owner");
  const pin = seeded.agents.find((agent) => agent.id === "coordinator");
  const confirmed = { ...seeded, unread: false, unread_count: 0, needs_attention: false, unread_reason: null,
    agents: [{ ...pin, thread_generation_revision: 1, read_through_sequence: 5, unread_count: 0, unread_reason: null }] };
  const call = vi.fn(async (method) => method === "entity.seen" ? { ok: true } : { items: [confirmed] });
  vi.doMock("../src/core/inboxDevices.js", () => ({ verbCall: () => call }));
  vi.doMock("../src/core/deviceContexts.js", () => ({ contextFor: () => null, homeContext: () => ({ deviceId }) }));
  try {
    const { markSeen } = await import("../src/core/inboxSeen.js");
    expect(await markSeen("project-owner", "coordinator", 1, 5, "thread-coordinator", deviceId)).toBe(true);
    expect((await cache.readCached(ownerAddress)).value.agents).toMatchObject([{ id: "coordinator", read_through_sequence: 5, unread_count: 0 }]);
    await vi.waitFor(async () => expect((await raw("feed")).items.find((row) => row.entity_id === "project-owner").agents[0].read_through_sequence).toBe(5));
    expect(await publish(writer, projectOwnerRecords().filter((record) => record.kind === "agent").map((record) => ({ ...record, revision: "2" })),
      { expectedCursor: "cursor-1", response: response("older-project") })).toBe(true);
    expect((await raw("project-agents", "project-owner")).agents.find((agent) => agent.id === "coordinator")).toMatchObject({ read_through_sequence: 5, unread_count: 0 });
  } finally {
    vi.doUnmock("../src/core/inboxDevices.js"); vi.doUnmock("../src/core/deviceContexts.js");
  }
});

it.each(["Recent", "Today"])("device pooled session facts give the same %s ordering as the legacy directory facts", async (partition) => {
  const writer = create(), now = 1793808000000, yesterday = now - 2 * 24 * 60 * 60 * 1000;
  const records = deviceRecords(), started = partition === "Today" ? now : yesterday;
  for (const record of records.filter((record) => record.kind === "owner_session" || record.kind === "project_session")) {
    record.row.session_started_ms = started; record.row.last_activity_ms = started + 1000;
  }
  // Live pooled summaries may include an unwatched agent absent from conversations.
  const conversation = records.find((record) => record.kind === "inbox_conversation").row;
  conversation.session_started_ms = yesterday; conversation.last_activity_ms = yesterday + 1000;
  const agent = records.find((record) => record.kind === "agent").row;
  records.push(deviceRecord("agent", "unwatched-agent", { ...agent, id: "unwatched-agent", conversation_id: "unwatched-agent", watched: false, unread_count: 0, ordinal: 2 }));
  await publish(writer, records, { snapshot: true });
  const legacy = { session_started_ms: started, last_activity_ms: started + 1000 };
  const { sessionTimes } = await import("../src/core/sessionSpans.js");
  const { workspaceIsRecent } = await import("../src/core/inbox.js");
  const workspace = (await raw("workspaces"))[0], project = (await raw("projects"))[0];
  expect(sessionTimes(workspace, now)).toEqual(sessionTimes(legacy, now));
  expect(sessionTimes(project, now)).toEqual(sessionTimes(legacy, now));
  expect(workspaceIsRecent({ ...sessionTimes(workspace, now), working: false }, now))
    .toBe(workspaceIsRecent({ ...sessionTimes(legacy, now), working: false }, now));
  expect((await raw("row", "owner-1")).last_activity_ms).toBe(legacy.last_activity_ms);
});

it("directories without an owner carry the same explicit empty session as a legacy list", async () => {
  const writer = create(), prior = { session_started_ms: 1793534400000, last_activity_ms: 1793534401000 };
  await cache.writeCached(address("workspaces"), [{ id: "workspace-1", ...prior }]);
  await cache.writeCached(address("projects"), [{ id: "proj-1", project_id: "proj-1", ...prior }]);
  await publish(writer, deviceRecords().filter((record) => record.kind === "workspace" || record.kind === "project"), { snapshot: true });
  for (const kind of ["workspaces", "projects"]) expect((await raw(kind))[0])
    .toMatchObject({ session_started_ms: null, last_activity_ms: null });
});

it("workspace lifecycle durable fields merge inside the existing runtime verdict and tombstones clear only their fields", async () => {
  const writer = create();
  await cache.writeCached(address("workspaces"), [{ id: "workspace-1", lifecycle: { idle: true, holds: ["dirty"], reclaimable: false, size_bytes: 5 } }]);
  await publish(writer, deviceRecords(), { snapshot: true });
  expect((await raw("workspaces"))[0].lifecycle).toMatchObject({ idle: true, holds: ["dirty"], reclaimable: false, size_bytes: 200, pruned_bytes: 100 });
  await publish(writer, [deviceRecord("workspace_lifecycle", "workspace-1", null, "2", { operation: "delete", member: false })],
    { expectedCursor: "cursor-1", response: response("lifecycle-deleted") });
  expect((await raw("workspaces"))[0].lifecycle).toEqual({ idle: true, holds: ["dirty"], reclaimable: false });
});

it("cold owner rows carry message title and unread reason for existing inbox consumers", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  expect(await raw("row", "owner-1")).toMatchObject({ title: "Synthetic agent", unread_reason: "reply", unread_count: 2 });
});

it("family tombstones clear lifecycle and retained session fields and remove project settings", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const tombstone = (kind, key) => deviceRecord(kind, key, null, "2", { operation: "delete", member: false });
  await publish(writer, [tombstone("workspace_lifecycle", "workspace-1"), tombstone("owner_session", "owner-1"), tombstone("project_session", "proj-1")],
    { expectedCursor: "cursor-1", response: response("cleared") });
  const workspace = (await raw("workspaces"))[0], project = (await raw("projects"))[0];
  expect(Object.hasOwn(workspace.lifecycle || {}, "size_bytes")).toBe(false);
  expect(Object.hasOwn(workspace, "retained_last_activity_ms")).toBe(false);
  expect(Object.hasOwn(project, "retained_last_activity_ms")).toBe(false);
  expect(await raw("row", "owner-1")).toBeUndefined();
  await publish(writer, [tombstone("project", "proj-1")], { expectedCursor: "cleared", response: response("project-deleted") });
  expect(await raw("project-settings", "proj-1")).toBeUndefined();
});

it("tombstones remove rows and nested members without deleting an unwatched task entity or resurrecting membership", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const kinds = [["workspace_directory", "workspace-1/dir-1"], ["project_source", "proj-1/source-1"], ["inbox_capture", "capture-1"], ["inbox_task", "task-1"], ["agent", "agent-1"], ["inbox_conversation", "agent-1"], ["owner_session", "owner-1"]];
  await publish(writer, kinds.map(([kind, key]) => deviceRecord(kind, key, null, "2", { operation: "delete", member: false })), { expectedCursor: "cursor-1", response: response("deleted") });
  expect((await raw("projects"))[0].sources).toEqual([]);
  expect((await raw("workspaces"))[0].directories).toEqual([]);
  expect((await raw("feed")).items).toEqual([]);
  expect(await raw("row", "owner-1")).toBeUndefined();
  expect(await raw("tracker-summary", "proj-1", "task-1")).toMatchObject({ task: { id: "task-1", watched: false } });
  await publish(writer, deviceRecords().filter((record) => kinds.some(([kind, key]) => kind === record.kind && key === record.key)), { expectedCursor: "deleted", response: response("replayed") });
  expect((await raw("feed")).items).toEqual([]);
});

it.each([false, true])("historical deletes carrying the current recreated row preserve membership across split-page=%s", async (split) => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const current = { ...deviceRecords().find((record) => record.kind === "inbox_capture").row, title: "Recreated capture" };
  // changes.rs overlays the historical operation on sync_device_record's
  // CURRENT row/member/revision. The row authority wins over that old verb.
  const historicalDelete = deviceRecord("inbox_capture", "capture-1", current, "5", { operation: "delete", member: true });
  const historicalUpsert = { ...historicalDelete, operation: "upsert" };
  if (split) {
    expect(await publish(writer, [historicalDelete], { expectedCursor: "cursor-1", response: response("page-1") })).toBe(true);
    expect((await raw("feed")).items.find((row) => row.capture_id === "capture-1")?.title).toBe("Recreated capture");
    expect(await publish(writer, [historicalUpsert], { expectedCursor: "page-1", response: response("page-2") })).toBe(true);
  } else {
    expect(await publish(writer, [historicalDelete, historicalUpsert], { expectedCursor: "cursor-1", response: response("page-2") })).toBe(true);
  }
  expect((await raw("feed")).items.find((row) => row.capture_id === "capture-1")?.title).toBe("Recreated capture");
});

it("retains the cache on empty snapshots and refuses late captures or pending local mutations", async () => {
  const writer = create();
  await cache.writeCached(address("feed"), { items: [{ kind: "capture", capture_id: "cached", title: "Cached capture" }] });
  const held = await raw("feed");
  await publish(writer, [], { snapshot: true, response: response("empty") });
  expect(await raw("feed")).toEqual(held);
  const capture = await writer.capture(); let finish;
  mutations.wrapTaskMutationCall(deviceId, "entity.seen", () => new Promise((resolve) => { finish = resolve; }));
  expect(await writer.publish({ records: deviceRecords(), capture, expectedCursor: "empty", response: response("stale") })).toBe(false);
  expect(await raw("feed")).toEqual(held);
  finish(); await Promise.resolve();
  expect(await writer.publish({ records: deviceRecords(), capture, expectedCursor: "empty", response: response("settled-stale") })).toBe(false);
});

it("a complete resnapshot retires previously tracked records omitted after retention expiry", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  await writer.reset({ capture: await writer.capture(), expectedCursor: "cursor-1" });
  await publish(writer, deviceRecords().filter((record) => ["project", "project_source", "badge"].includes(record.kind)),
    { snapshot: true, response: { ...response("resnapshot"), watermark: "8" } });
  expect(await raw("workspaces")).toEqual([]);
  expect(await raw("row", "owner-1")).toBeUndefined();
  expect((await raw("feed")).items).toEqual([]);
  expect(await raw("tracker-summary", "proj-1", "task-1")).toMatchObject({ task: { watched: false } });
  expect(await raw("archive")).toMatchObject({ items: [] });
});

it("a retired shared task generation rejects a device capture even without a local mutation", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const captured = await writer.capture();
  const authority = await import("../src/core/taskContentAuthority.js");
  await authority.mergeTaskContentRecordsTogether({ deviceId, projectId: "proj-1" }, [], () => ({ values: [], retire: true }));
  expect(await writer.publish({ capture: captured, records: [deviceRecord("inbox_task", "task-1", { ...deviceRecords().find((record) => record.kind === "inbox_task").row, title: "Retired title" }, "2")],
    response: response("stale"), expectedCursor: "cursor-1" })).toBe(false);
  expect((await raw("tracker-summary", "proj-1", "task-1")).task.title).toBe("Synthetic task");
});

it("a retired conversation thread rejects stale device digests without advancing the cursor", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  await cache.writeCached(address("thread", "owner-1", "agent-1"), { thread_id: "thread-2", thread_generation_revision: "2", retired_thread_ids: ["thread-1"], items: [] });
  expect(await publish(writer, deviceRecords().filter((record) => record.kind === "agent" || record.kind === "inbox_conversation").map((record) => ({ ...record, revision: "2" })),
    { expectedCursor: "cursor-1", response: response("retired") })).toBe(false);
  expect((await writer.readCursor()).cursor).toBe("cursor-1");
});

it("refuses a stale capture when a previously absent owner row was created during its request", async () => {
  const writer = create(), capture = await writer.capture();
  await cache.writeCached(address("row", "owner-1"), { entity_id: "owner-1", title: "Immediate local row", agents: [] });
  expect(await writer.publish({ capture, records: deviceRecords(), response: response(), snapshot: true })).toBe(false);
  expect(await raw("row", "owner-1")).toMatchObject({ title: "Immediate local row" });
  expect(await writer.readCursor()).toBeUndefined();
});

it.each(["project-settings", "tracker-summary-identity", "tracker-summary"])("a newer %s write made during the request fences a device publication", async (kind) => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const captured = await writer.capture();
  const sub = kind === "tracker-summary-identity" ? "agent-1" : kind === "tracker-summary" ? "task-1" : "";
  const target = address(kind, "proj-1", sub), value = { title: "Newer immediate value" };
  await cache.writeCached(target, value);
  expect(await writer.publish({ capture: captured, records: deviceRecords().map((record) => ({ ...record, revision: "2" })),
    expectedCursor: "cursor-1", response: response("stale-write") })).toBe(false);
  expect((await cache.readCached(target)).value).toEqual(value);
  expect((await writer.readCursor()).cursor).toBe("cursor-1");
});

it("a newer read mark in the complete project roster wins over an older owner row", async () => {
  const writer = create(); await publish(writer, [...deviceRecords(), ...projectOwnerRecords()], { snapshot: true });
  const roster = await raw("project-agents", "project-owner");
  await cache.writeCached(address("project-agents", "project-owner"), { ...roster, agents: roster.agents.map((agent) => agent.id === "coordinator"
    ? { ...agent, read_through_sequence: 5, unread_count: 0, unread_reason: null } : agent) });
  expect(await publish(writer, projectOwnerRecords().filter((record) => record.kind === "agent").map((record) => ({ ...record, revision: "2" })),
    { expectedCursor: "cursor-1", response: response("older-owner") })).toBe(true);
  expect((await raw("project-agents", "project-owner")).agents.find((agent) => agent.id === "coordinator")).toMatchObject({ read_through_sequence: 5, unread_count: 0 });
});

it("keeps a local mark-read and unread badges across older conversation publication until bridge confirmation", async () => {
  const writer = create(); await publish(writer, deviceRecords(), { snapshot: true });
  const before = await cache.readCached(address("row", "owner-1"));
  await cache.mergeCachedReadState(address("row", "owner-1"), before, (row) => ({ ...row, unread: false, unread_count: 0, agents: row.agents.map((agent) => ({ ...agent, thread_generation_revision: 1, read_through_sequence: 5, unread_count: 0, unread_reason: null })) }));
  const older = deviceRecords().find((record) => record.kind === "inbox_conversation");
  await publish(writer, [{ ...older, revision: "2" }], { expectedCursor: "cursor-1", response: response("old-state") });
  expect(await raw("row", "owner-1")).toMatchObject({ unread_count: 0, agents: [{ read_through_sequence: 5, unread_count: 0 }] });
  expect((await raw("feed")).items.find((row) => row.entity_id === "owner-1")).toMatchObject({ unread_count: 0 });
  await publish(writer, [{ ...older, revision: "3", row: { ...older.row, read_through_sequence: "5", unread_count: 0, unread_reason: null } }], { expectedCursor: "old-state", response: response("confirmed") });
  expect(await raw("row", "owner-1")).toMatchObject({ agents: [{ read_through_sequence: "5", unread_count: 0 }] });
});

it("cursor persistence failure rolls the whole device publication back", async () => {
  const writer = create();
  const put = IDBObjectStore.prototype.put;
  const refusal = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function(value, key) {
    if (String(key).includes("sync-cursor")) throw new DOMException("full", "QuotaExceededError");
    return put.call(this, value, key);
  });
  try { expect(await publish(writer, deviceRecords(), { snapshot: true })).toBe(false); }
  finally { refusal.mockRestore(); }
  expect(await raw("projects")).toBeUndefined(); expect(await writer.readCursor()).toBeUndefined();
});
