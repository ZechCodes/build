import { afterEach, expect, it, vi } from "vitest";

vi.mock("../src/core/localCache.js", () => import("./memoryCache.js"));

import { secretsChangedOn } from "../src/core/secretMetadataEvents.js";
import {
  createSecretMetadataCache, readCachedSecrets, secretMetadataAddress,
} from "../src/core/secretMetadataCache.js";

const capabilities = () => ({ attachments: true, sealedUpload: true, metadataEvents: true });
const projectScope = { projectId: "project-cache" };
const agentScope = {
  projectId: "project-cache", ownerId: "owner-cache", agentId: "agent-cache",
  conversationId: "conversation-cache", generation: "thread-cache", draftId: null,
};
const metadata = ({ id = "sec_project", scope = "project", revision = 1, status = "staged" } = {}) => ({
  secret_id: id,
  request_id: `${id === "sec_project" ? "11111111" : "22222222"}-1111-4111-8111-111111111111`,
  name: id === "sec_project" ? "PROJECT_TOKEN" : "AGENT_TOKEN",
  ttl_seconds: null,
  grant: scope === "project"
    ? { scope: "project", project_id: "project-cache" }
    : { scope: "agent", agent_id: "agent-cache", conversation_id: "conversation-cache", generation: "thread-cache" },
  scope: { owner_id: "owner-cache", conversation_id: "conversation-cache", generation: "thread-cache", draft_id: null },
  created_at: 1_800_000_000,
  attached_at: null,
  expires_at: null,
  revoked_at: status === "revoked" ? 1_800_000_010 : null,
  revision,
  status,
});

const caches = [];
afterEach(() => caches.splice(0).forEach((cache) => cache.dispose()));

it("keys project grants by project and agent grants by generation", async () => {
  const calls = [];
  const cache = createSecretMetadataCache({
    deviceId: "device-cache", capabilities,
    call: async (method, params) => {
      calls.push({ method, params });
      return { secrets: [metadata(), metadata({ id: "sec_agent", scope: "agent" })], server_now: 1_800_000_000, next: null };
    },
  });
  caches.push(cache);

  await cache.refresh(agentScope);

  expect(calls).toEqual([{ method: "secrets.list", params: {
    scope: { project_id: "project-cache", stage: { owner_id: "owner-cache", conversation_id: "conversation-cache", generation: "thread-cache", draft_id: null } },
    secret_ids: [], request_ids: [], after: null, limit: 100,
  } }]);
  expect(secretMetadataAddress("device-cache", projectScope)).toEqual({
    deviceId: "device-cache", entityId: "project-cache", kind: "secret-metadata", sub: "project",
  });
  expect(secretMetadataAddress("device-cache", agentScope)).toEqual({
    deviceId: "device-cache", entityId: "conversation-cache", kind: "secret-metadata", sub: "owner-cache:agent-cache:thread-cache",
  });
  expect((await readCachedSecrets("device-cache", projectScope)).map((entry) => entry.secret_id)).toEqual(["sec_project"]);
  expect((await readCachedSecrets("device-cache", agentScope)).map((entry) => entry.secret_id)).toEqual(["sec_agent"]);
  expect(await cache.findRequest(agentScope, projectMetadataRequest())).toMatchObject({ secret_id: "sec_project" });
});

function projectMetadataRequest() {
  return "11111111-1111-4111-8111-111111111111";
}

it("projects only metadata fields and never caches values or upload envelopes", async () => {
  const cache = createSecretMetadataCache({ deviceId: "device-project", capabilities, call: vi.fn() });
  caches.push(cache);
  await cache.upsert({ ...metadata(), value: "must-not-land", sealed_payload: "must-not-land", unknown: "must-not-land" }, projectScope);
  const cached = await readCachedSecrets("device-project", projectScope);
  const serialized = JSON.stringify(cached);
  expect(serialized).not.toContain("must-not-land");
  expect(cached[0]).toEqual(metadata());
});

it("refuses metadata from another project instead of poisoning that project's cache", async () => {
  const cache = createSecretMetadataCache({ deviceId: "device-foreign", capabilities, call: vi.fn() });
  caches.push(cache);
  await expect(cache.upsert({
    ...metadata(), grant: { scope: "project", project_id: "foreign-project" },
  }, projectScope)).rejects.toMatchObject({ code: "invalid_secret_metadata" });
  expect(await cache.read(projectScope)).toEqual([]);
});

it("keeps valid project grants when one descriptor no longer belongs to the queried draft", async () => {
  const moved = {
    ...metadata({ id: "sec_agent", scope: "agent" }),
    scope: { ...metadata({ id: "sec_agent", scope: "agent" }).scope, draft_id: null },
  };
  const draftScope = { ...agentScope, draftId: "draft-old" };
  const cache = createSecretMetadataCache({
    deviceId: "device-mixed", capabilities,
    call: vi.fn(async () => ({ secrets: [metadata(), moved], server_now: 1_800_000_000, next: null })),
  });
  caches.push(cache);

  await expect(cache.refresh(draftScope)).resolves.toEqual([metadata()]);
  expect((await cache.read(projectScope)).map((entry) => entry.secret_id)).toEqual(["sec_project"]);
  expect(await cache.read(draftScope)).toEqual([]);
});

it("treats reordered events as refresh hints while revisions prevent rollback", async () => {
  const replies = [metadata({ revision: 4, status: "revoked" }), metadata({ revision: 3, status: "staged" })];
  const call = vi.fn(async () => ({ secrets: [replies.shift()], server_now: 1_800_000_000, next: null }));
  const cache = createSecretMetadataCache({ deviceId: "device-events", capabilities, call });
  caches.push(cache);
  cache.track(projectScope);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  await vi.waitFor(async () => expect((await cache.read(projectScope))[0]?.revision).toBe(4));
  call.mockClear();

  secretsChangedOn("device-events", { scope: "project", project_id: "project-cache", revision: 3, secret_ids: ["sec_project"] });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  await vi.waitFor(async () => expect((await cache.read(projectScope))[0]).toMatchObject({ revision: 4, status: "revoked" }));
});

it("refreshes only the invalidated project or agent scope", async () => {
  const call = vi.fn(async (_method, params) => ({
    secrets: params.scope.stage ? [metadata({ id: "sec_agent", scope: "agent" })] : [metadata()],
    server_now: 1_800_000_000, next: null,
  }));
  const cache = createSecretMetadataCache({ deviceId: "device-scopes", capabilities, call });
  caches.push(cache);
  cache.track(projectScope);
  cache.track(agentScope);
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
  call.mockClear();

  secretsChangedOn("device-scopes", {
    scope: "agent", entity_id: "owner-cache", conversation_id: "conversation-cache",
    thread_id: "thread-cache", generation: "thread-cache", revision: 1, secret_ids: ["sec_agent"],
  });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
  expect(call.mock.calls[0][1].scope.stage).toBeDefined();

  secretsChangedOn("device-scopes", {
    scope: "project", project_id: "project-cache", revision: 2, secret_ids: ["sec_project"],
  });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
  expect(call.mock.calls[1][1].scope).toEqual({ project_id: "project-cache" });
});

it("refreshes tracked scopes after event loss and refuses a stale generation reply", async () => {
  let resolveAgent;
  const call = vi.fn((method, params) => {
    if (params.scope.stage) return new Promise((resolve) => { resolveAgent = resolve; });
    return Promise.resolve({ secrets: [metadata({ revision: 2 })], server_now: 1_800_000_000, next: null });
  });
  let currentGeneration = "thread-cache";
  const cache = createSecretMetadataCache({ deviceId: "device-loss", capabilities, call });
  caches.push(cache);
  cache.track(projectScope);
  cache.track(agentScope, { active: () => currentGeneration === agentScope.generation });
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
  resolveAgent({ secrets: [], server_now: 1_800_000_000, next: null });
  await vi.waitFor(async () => expect(await cache.read(projectScope)).toHaveLength(1));
  call.mockClear();

  const refreshing = cache.refreshTracked();
  await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
  currentGeneration = "thread-new";
  resolveAgent({ secrets: [metadata({ id: "sec_agent", scope: "agent", revision: 9 })], server_now: 1_800_000_000, next: null });
  await refreshing;
  await vi.waitFor(async () => expect(await cache.read(projectScope)).toHaveLength(1));
  expect(await cache.read(agentScope)).toEqual([]);
});

it("refreshes a scope registered after support is already live", async () => {
  const call = vi.fn(async () => ({ secrets: [metadata()], server_now: 1_800_000_000, next: null }));
  const cache = createSecretMetadataCache({ deviceId: "device-late", capabilities, call });
  caches.push(cache);

  cache.track(projectScope);

  await vi.waitFor(() => expect(call).toHaveBeenCalledOnce());
  await vi.waitFor(async () => expect((await cache.read(projectScope)).map((entry) => entry.secret_id)).toEqual(["sec_project"]));
});

it("keeps overlapping registrations independent until their own disposer runs", async () => {
  const call = vi.fn(async () => ({ secrets: [metadata()], server_now: 1_800_000_000, next: null }));
  const cache = createSecretMetadataCache({ deviceId: "device-overlap", capabilities, call });
  caches.push(cache);
  const stopFirst = cache.track(projectScope);
  const stopSecond = cache.track(projectScope);
  await vi.waitFor(() => expect(call).toHaveBeenCalledOnce());
  call.mockClear();

  stopFirst();
  secretsChangedOn("device-overlap", {
    scope: "project", project_id: "project-cache", revision: 2, secret_ids: ["sec_project"],
  });
  await vi.waitFor(() => expect(call).toHaveBeenCalledOnce());
  call.mockClear();

  stopSecond();
  secretsChangedOn("device-overlap", {
    scope: "project", project_id: "project-cache", revision: 3, secret_ids: ["sec_project"],
  });
  await Promise.resolve();
  expect(call).not.toHaveBeenCalled();
});

it("degrades silently when the complete capability family is absent", async () => {
  for (const offered of [
    {},
    { attachments: true, sealedUpload: true, metadataEvents: false },
  ]) {
    const call = vi.fn();
    const cache = createSecretMetadataCache({ deviceId: `device-off-${callsKey(offered)}`, capabilities: () => offered, call });
    caches.push(cache);
    await expect(cache.refresh(projectScope)).resolves.toEqual([]);
    expect(call).not.toHaveBeenCalled();
  }
});

function callsKey(value) {
  return Object.values(value).filter(Boolean).length;
}


it("keeps same-name environment and project grants apart and opts inherited reads in", async () => {
  const environment = { ...metadata(), secret_id: "sec_environment", grant: { scope: "environment", environment_id: "env-cache" } };
  const scope = { projectId: "project-cache", environmentId: "env-cache" };
  const call = vi.fn(async () => ({ secrets: [metadata(), environment], server_now: 1800000000, next: null }));
  const cache = createSecretMetadataCache({ deviceId: "environment-cache", call,
    capabilities: () => ({ ...capabilities(), environmentScope: true }) });
  caches.push(cache);
  await cache.refresh(scope);
  expect(call).toHaveBeenLastCalledWith("secrets.list", expect.objectContaining({ scope: { project_id: "project-cache" }, include_environment: true }));
  expect(await cache.read(projectScope)).toEqual([metadata()]);
  expect(await cache.read({ environmentId: "env-cache" })).toEqual([environment]);
  expect(secretMetadataAddress("environment-cache", { environmentId: "env-cache" })).not.toEqual(secretMetadataAddress("environment-cache", { projectId: "env-cache" }));
  call.mockClear(); cache.track({ environmentId: "env-cache" });
  await vi.waitFor(() => expect(call).toHaveBeenCalled());
  expect(call).toHaveBeenLastCalledWith("secrets.list", expect.objectContaining({ scope: { environment_id: "env-cache" } }));
  call.mockClear(); secretsChangedOn("environment-cache", { scope: "environment", environment_id: "env-cache", revision: 2, secret_ids: ["sec_environment"] });
  await vi.waitFor(() => expect(call).toHaveBeenCalledOnce());
});

it("does not clear a cached environment list when an inherited read is refused", async () => {
  const environment = { ...metadata(), secret_id: "sec_environment", grant: { scope: "environment", environment_id: "env-refusal" } };
  const cache = createSecretMetadataCache({ deviceId: "environment-refusal", call: vi.fn(async () => { throw new Error("membership uncertain"); }),
    capabilities: () => ({ ...capabilities(), environmentScope: true }) });
  caches.push(cache);
  await cache.upsert(environment, { environmentId: "env-refusal" });
  await expect(cache.refresh({ projectId: "project-cache", environmentId: "env-refusal" })).rejects.toThrow();
  expect(await cache.read({ environmentId: "env-refusal" })).toEqual([environment]);
});

it.each(["project", "environment"])("updates %s expiry at the same revision and never regresses to an older server time", async kind => {
  const scope = kind === "project" ? projectScope : { environmentId: "env-expiry" };
  const available = { ...metadata({ status: "available" }), attached_at: 1800000000, expires_at: 1800000060,
    ...(kind === "environment" ? { grant: { scope: "environment", environment_id: "env-expiry" } } : {}) };
  const pages = [
    { secrets: [available], server_now: 1800000000, next: null },
    { secrets: [{ ...available, status: "expired" }], server_now: 1800000060, next: null },
    { secrets: [available], server_now: 1800000001, next: null },
  ];
  const cache = createSecretMetadataCache({ deviceId: `expiry-${kind}`, capabilities: () => ({ ...capabilities(), environmentScope: true }), call: async () => pages.shift() });
  caches.push(cache);
  await cache.refresh(scope); expect((await cache.read(scope))[0].status).toBe("available");
  await cache.refresh(scope); expect((await cache.read(scope))[0]).toMatchObject({ status: "expired", revision: 1 });
  await cache.refresh(scope); expect((await cache.read(scope))[0].status).toBe("expired");
});

it.each(["project", "environment"])("expires cached %s grants at an authoritative deadline without a push, read, or revision change", async kind => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date("2040-01-01T00:00:00Z")); // deliberately unlike the bridge clock
  const scope = kind === "project" ? projectScope : { environmentId: "env-timer" };
  const available = { ...metadata({ status: "available" }), attached_at: 1800000000, expires_at: 1800000060,
    ...(kind === "environment" ? { grant: { scope: "environment", environment_id: "env-timer" } } : {}) };
  const call = vi.fn(async () => ({ secrets: [available], server_now: 1800000000, next: null }));
  const cache = createSecretMetadataCache({ deviceId: `timer-${kind}`, capabilities: () => ({ ...capabilities(), environmentScope: true }), call });
  caches.push(cache);
  try {
    await cache.refresh(scope);
    const changed = vi.fn(); const stop = cache.subscribe(scope, changed);
    await vi.advanceTimersByTimeAsync(59000);
    expect((await cache.read(scope))[0].status).toBe("available");
    vi.setSystemTime(new Date("2000-01-01T00:00:00Z")); // a wall-clock rollback must not extend the lease
    await vi.advanceTimersByTimeAsync(1000);
    expect((await readCachedSecrets(`timer-${kind}`, scope))[0]).toMatchObject({ status: "expired", revision: 1 });
    expect(changed).toHaveBeenCalled(); expect(call).toHaveBeenCalledOnce(); stop();
  } finally { cache.dispose(); vi.useRealTimers(); }
});

it.each(["project", "environment"])("replaces a complete %s list and fences an older in-flight list", async kind => {
  const scope = kind === "project" ? projectScope : { environmentId: "env-replace" };
  const row = kind === "project" ? metadata() : { ...metadata(), grant: { scope: "environment", environment_id: "env-replace" } };
  let releaseOld;
  const call = vi.fn()
    .mockResolvedValueOnce({ secrets: [row], server_now: 1800000000, next: null })
    .mockImplementationOnce(() => new Promise(resolve => { releaseOld = resolve; }))
    .mockResolvedValueOnce({ secrets: [], server_now: 1800000001, next: null });
  const cache = createSecretMetadataCache({ deviceId: `replace-secrets-${kind}`, capabilities: () => ({ ...capabilities(), environmentScope: true }), call }); caches.push(cache);
  await cache.refresh(scope);
  const old = cache.refresh(scope);
  await vi.waitFor(() => expect(releaseOld).toBeTypeOf("function"));
  await cache.refresh(scope); expect(await cache.read(scope)).toEqual([]);
  releaseOld({ secrets: [row], server_now: 1800000001, next: null }); await old;
  expect(await cache.read(scope)).toEqual([]);
});

it("publishes a full paginated list only after all pages succeed and retains cached rows on failure", async () => {
  const call = vi.fn()
    .mockResolvedValueOnce({ secrets: [metadata()], server_now: 1800000000, next: null })
    .mockResolvedValueOnce({ secrets: [metadata({ id: "sec_new" })], server_now: 1800000001, next: "page2" })
    .mockRejectedValueOnce(new Error("membership uncertain"));
  const cache = createSecretMetadataCache({ deviceId: "atomic-secret-pages", capabilities, call }); caches.push(cache);
  await cache.refresh(projectScope);
  await expect(cache.refresh(projectScope)).rejects.toThrow();
  expect(await cache.read(projectScope)).toEqual([metadata()]);
});

it("does not discard an immediate mutation or resurrect a removed grant from an incidental stage read", async () => {
  const pending = [];
  const call = vi.fn((_method, params) => new Promise(resolve => pending.push({ resolve, params })));
  const cache = createSecretMetadataCache({ deviceId: "secret-write-fence", capabilities, call }); caches.push(cache);
  const read = cache.refresh(projectScope);
  await vi.waitFor(() => expect(pending).toHaveLength(1));
  await cache.upsert(metadata({ revision: 3, status: "revoked" }), projectScope);
  pending[0].resolve({ secrets: [], server_now: 1800000000, next: null }); await read;
  expect((await cache.read(projectScope))[0]).toMatchObject({ revision: 3, status: "revoked" });

  const full = cache.refresh(projectScope);
  await vi.waitFor(() => expect(pending).toHaveLength(2));
  const stage = cache.refresh(agentScope);
  await vi.waitFor(() => expect(pending).toHaveLength(3));
  pending[1].resolve({ secrets: [], server_now: 1800000001, next: null }); await full;
  pending[2].resolve({ secrets: [metadata()], server_now: 1800000001, next: null }); await stage;
  expect(await cache.read(projectScope)).toEqual([]);
});

it("a filtered request lookup preserves unrelated cached rows", async () => {
  const call = vi.fn(async () => ({ secrets: [], server_now: 1800000000, next: null }));
  const cache = createSecretMetadataCache({ deviceId: "secret-filtered-list", capabilities, call }); caches.push(cache);
  await cache.upsert(metadata(), projectScope);
  await cache.refresh(projectScope, { requestIds: ["missing-request"] });
  expect(await cache.read(projectScope)).toEqual([metadata()]);
});


it("removes a missing explicitly queried ID without clearing unrelated grants", async () => {
  const cache = createSecretMetadataCache({ deviceId: "secret-filtered-removal", capabilities,
    call: async () => ({ secrets: [], server_now: 1800000000, next: null }) }); caches.push(cache);
  const other = { ...metadata(), secret_id: "sec_other" };
  await cache.upsert(metadata(), projectScope); await cache.upsert(other, projectScope);
  await cache.refresh(projectScope, { secretIds: ["sec_project"] });
  expect(await cache.read(projectScope)).toEqual([other]);
});

it("keeps elapsed bridge time when an equal-time page arrives late, including across a cache restart", async () => {
  vi.useFakeTimers(); vi.setSystemTime(new Date("2040-01-01T00:00:00Z"));
  const row = { ...metadata({ status: "available" }), attached_at: 1800000000, expires_at: 1800000060 };
  const options = { deviceId: "clock-restart", capabilities,
    call: async () => ({ secrets: [row], server_now: 1800000000, next: null }) };
  const first = createSecretMetadataCache(options); caches.push(first);
  let restarted;
  try {
    await first.refresh(projectScope);
    await vi.advanceTimersByTimeAsync(59000);
    await first.refresh(projectScope); // delayed old server_now must not restart its 60 seconds
    first.dispose();
    restarted = createSecretMetadataCache(options); caches.push(restarted);
    await restarted.read(projectScope);
    await vi.advanceTimersByTimeAsync(1000);
    expect((await readCachedSecrets(options.deviceId, projectScope))[0].status).toBe("expired");
  } finally { first.dispose(); restarted?.dispose(); vi.useRealTimers(); }
});

const environmentMutation = (id, revision = 1, status = "available") => ({
  ...metadata({ id, revision, status }), attached_at: 1800000000,
  grant: { scope: "environment", environment_id: "env-write-races" },
});
const environmentWriteScope = { environmentId: "env-write-races" };
const environmentCapabilities = () => ({ ...capabilities(), environmentScope: true });

it("publishes both overlapping acknowledged environment mutations", async () => {
  const cache = createSecretMetadataCache({ deviceId: "overlapping-secret-writes", capabilities: environmentCapabilities, call: vi.fn() }); caches.push(cache);
  await cache.upsert(environmentMutation("sec_a"), environmentWriteScope);
  await cache.upsert(environmentMutation("sec_b"), environmentWriteScope);
  const acknowledgments = [environmentMutation("sec_a", 2, "revoked"), environmentMutation("sec_b", 2, "revoked")];
  await expect(Promise.all(acknowledgments.map(row => cache.upsert(row, environmentWriteScope)))).resolves.toEqual(acknowledgments);
  expect(await cache.read(environmentWriteScope)).toEqual(acknowledgments);
});

it("a refresh admitted while an acknowledged mutation is publishing cannot suppress that write", async () => {
  let finishRead;
  const call = vi.fn(() => new Promise(resolve => { finishRead = resolve; }));
  const cache = createSecretMetadataCache({ deviceId: "refresh-between-secret-write", capabilities: environmentCapabilities, call }); caches.push(cache);
  const original = environmentMutation("sec_a");
  const revoked = environmentMutation("sec_a", 2, "revoked");
  await cache.upsert(original, environmentWriteScope);
  const mutation = cache.upsert(revoked, environmentWriteScope);
  const refresh = cache.refresh(environmentWriteScope);
  try {
    await expect(mutation).resolves.toEqual(revoked);
    expect(await cache.read(environmentWriteScope)).toEqual([revoked]);
  } finally {
    await vi.waitFor(() => expect(finishRead).toBeTypeOf("function"));
    finishRead({ secrets: [original], server_now: 1800000000, next: null }); await refresh;
  }
  expect(await cache.read(environmentWriteScope)).toEqual([revoked]);
});

it("rejects an entire snapshot when a later page contains malformed metadata", async () => {
  const original = [metadata(), { ...metadata(), secret_id: "sec_second" }];
  const call = vi.fn().mockResolvedValueOnce({ secrets: original, server_now: 1800000000, next: null })
    .mockResolvedValueOnce({ secrets: [{ ...original[0], name: "CHANGED", revision: 2 }], server_now: 1800000001, next: "page2" })
    .mockResolvedValueOnce({ secrets: [{ ...original[1], expires_at: "malformed" }], server_now: 1800000001, next: null });
  const cache = createSecretMetadataCache({ deviceId: "malformed-secret-page", capabilities, call }); caches.push(cache);
  await cache.refresh(projectScope);
  await expect(cache.refresh(projectScope)).rejects.toThrow();
  expect(await cache.read(projectScope)).toEqual(original);
});

it.each(["secretIds", "requestIds"])("a malformed filtered %s lookup preserves all cached rows", async selector => {
  const original = [metadata(), { ...metadata(), secret_id: "sec_other", request_id: "other-request" }];
  const call = vi.fn().mockResolvedValueOnce({ secrets: original, server_now: 1800000000, next: null })
    .mockResolvedValueOnce({ secrets: [{ ...original[0], grant: { scope: "project" } }], server_now: 1800000001, next: null });
  const cache = createSecretMetadataCache({ deviceId: `malformed-secret-${selector}`, capabilities, call }); caches.push(cache);
  await cache.refresh(projectScope);
  const selected = selector === "secretIds" ? original[0].secret_id : original[0].request_id;
  await expect(cache.refresh(projectScope, { [selector]: [selected] })).rejects.toThrow();
  expect(await cache.read(projectScope)).toEqual(original.sort((a, b) => a.secret_id.localeCompare(b.secret_id)));
});

it.each(["project", "environment"])("publishes newer %s metadata after bridge time steps back with a corrected time anchor", async kind => {
  const scope = kind === "project" ? projectScope : { environmentId: "env-step-back" };
  const original = { ...metadata({ status: "available" }), attached_at: 1800000000,
    ...(kind === "environment" ? { grant: { scope: "environment", environment_id: "env-step-back" } } : {}) };
  const revoked = { ...original, revision: 2, status: "revoked", revoked_at: 1800000005 };
  const call = vi.fn().mockResolvedValueOnce({ secrets: [original], server_now: 1800000010, next: null })
    .mockResolvedValueOnce({ secrets: [revoked], server_now: 1800000005, next: null });
  const deviceId = `step-back-${kind}`;
  const cache = createSecretMetadataCache({ deviceId, capabilities: environmentCapabilities, call }); caches.push(cache);
  await cache.refresh(scope);
  const { readCached } = await import("../src/core/localCache.js");
  const address = secretMetadataAddress(deviceId, scope);
  await cache.refresh(scope);
  expect(await cache.read(scope)).toEqual([revoked]);
  expect((await readCached(address)).value).toMatchObject({ server_now: 1800000005, clock_now: 1800000005 });
});

it.each(["project", "environment"])("a new short-lived %s grant after a bridge clock correction stays live until its real expiry", async kind => {
  vi.useFakeTimers(); vi.setSystemTime(new Date("2040-01-01T00:00:00Z"));
  const scope = kind === "project" ? projectScope : { environmentId: "env-corrected-clock" };
  const grant = kind === "project" ? { scope: "project", project_id: projectScope.projectId }
    : { scope: "environment", environment_id: "env-corrected-clock" };
  const expired = { ...metadata({ status: "expired" }), secret_id: "sec_expired", grant,
    created_at: 800, attached_at: 800, expires_at: 950 };
  const revoked = { ...expired, secret_id: "sec_revoked", status: "revoked", revision: 2, revoked_at: 980 };
  const fresh = { ...expired, secret_id: "sec_fresh", status: "available", created_at: 900, attached_at: 900,
    expires_at: 960, ttl_seconds: 60 };
  const oldAvailable = { ...expired, status: "available", revision: 3 };
  const oldRevoked = { ...revoked, status: "available", revision: 1, revoked_at: null };
  const corrected = { secrets: [oldAvailable, oldRevoked, fresh], server_now: 900, next: null };
  const call = vi.fn().mockResolvedValueOnce({ secrets: [expired, revoked], server_now: 1000, next: null }).mockImplementation(async () => corrected);
  const deviceId = `new-grant-after-correction-${kind}`;
  const cache = createSecretMetadataCache({ deviceId, capabilities: environmentCapabilities, call }); caches.push(cache);
  const statuses = async () => Object.fromEntries((await cache.read(scope)).map(row => [row.secret_id, row.status]));
  try {
    await cache.refresh(scope);
    await cache.refresh(scope);
    expect(await statuses()).toEqual({ sec_expired: "expired", sec_fresh: "available", sec_revoked: "revoked" });
    await vi.advanceTimersByTimeAsync(59000);
    await cache.refresh(scope); // equal-time delayed observations must not restart the TTL
    expect((await statuses()).sec_fresh).toBe("available");
    await vi.advanceTimersByTimeAsync(1000);
    expect((await statuses()).sec_fresh).toBe("expired");
    await cache.refresh(scope);
    expect(await statuses()).toEqual({ sec_expired: "expired", sec_fresh: "expired", sec_revoked: "revoked" });
  } finally { cache.dispose(); vi.useRealTimers(); }
});

it.each(["project", "environment"])("an older %s response cannot overwrite a corrected clock or remove its new grant", async kind => {
  const scope = kind === "project" ? projectScope : { environmentId: "env-clock-read-fence" };
  const row = { ...metadata({ status: "available" }), created_at: 900, attached_at: 900, expires_at: 960,
    ...(kind === "environment" ? { grant: { scope: "environment", environment_id: "env-clock-read-fence" } } : {}) };
  let releaseOld;
  const call = vi.fn().mockResolvedValueOnce({ secrets: [], server_now: 1000, next: null })
    .mockImplementationOnce(() => new Promise(resolve => { releaseOld = resolve; }))
    .mockResolvedValueOnce({ secrets: [row], server_now: 900, next: null });
  const cache = createSecretMetadataCache({ deviceId: `corrected-read-fence-${kind}`, capabilities: environmentCapabilities, call }); caches.push(cache);
  await cache.refresh(scope);
  const older = cache.refresh(scope);
  await vi.waitFor(() => expect(releaseOld).toBeTypeOf("function"));
  await cache.refresh(scope);
  releaseOld({ secrets: [], server_now: 1000, next: null }); await older;
  expect(await cache.read(scope)).toEqual([row]);
});
