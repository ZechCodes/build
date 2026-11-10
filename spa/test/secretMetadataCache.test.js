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
