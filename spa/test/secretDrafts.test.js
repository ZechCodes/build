import { expect, it, vi } from "vitest";
import { createSecretDraftService, secretFeaturesAvailable } from "../src/core/secretDrafts.js";

const recipient = () => ({
  ownerId: "owner-draft", agentId: "agent-draft", conversationId: "conversation-draft", generation: "thread-draft", draftId: null,
});
const capabilities = () => ({ attachments: true, sealedUpload: true, metadataEvents: true });
const projectMetadata = {
  secret_id: "sec_staged", request_id: "11111111-1111-4111-8111-111111111111", name: "TEST_TOKEN", ttl_seconds: null,
  grant: { scope: "project", project_id: "project-draft" },
  scope: { owner_id: "owner-draft", conversation_id: "conversation-draft", generation: "thread-draft", draft_id: null },
  created_at: 1_800_000_000, attached_at: null, expires_at: null, revoked_at: null, revision: 1, status: "staged",
};

function harness(overrides = {}) {
  let entries = [];
  const writes = [];
  const draft = {
    read: () => entries.map((entry) => ({ ...entry })),
    write: vi.fn((next) => { entries = next.map((entry) => ({ ...entry })); writes.push(structuredClone(next)); }),
  };
  const metadata = {
    upsert: vi.fn(async () => projectMetadata),
    findRequest: vi.fn(async () => null),
    refresh: vi.fn(async () => []),
  };
  const call = vi.fn(async (method) => method === "secrets.create" ? projectMetadata : { ...projectMetadata, status: "revoked", revision: 2, revoked_at: 1_800_000_010 });
  const sealUpload = vi.fn(async () => "synthetic-sealed-envelope");
  const service = createSecretDraftService({
    deviceId: "device-draft", creator: "device_user", projectId: "project-draft", recipient,
    capabilities, deviceTransportPublicKey: () => "synthetic-public-key",
    call, sealUpload,
    createRequestId: () => "11111111-1111-4111-8111-111111111111",
    createOperationId: () => "22222222-2222-4222-8222-222222222222",
    draft, metadata, ...overrides,
  });
  return { service, call, draft, metadata, sealUpload, writes, entries: () => entries };
}

it("keeps the value out of every persistent sink and wipes modal memory after create", async () => {
  const sinks = harness();
  const clearValue = vi.fn();
  const secretValue = "synthetic-value-that-must-never-persist";
  const staged = await sinks.service.stage({ name: "TEST_TOKEN", value: secretValue, scope: "project", ttlSeconds: null, clearValue });

  expect(staged).toEqual(projectMetadata);
  expect(clearValue).toHaveBeenCalledOnce();
  expect(sinks.call).toHaveBeenCalledWith("secrets.create", expect.objectContaining({
    request_id: projectMetadata.request_id,
    grant: { scope: "project", project_id: "project-draft" },
    sealed_payload: "synthetic-sealed-envelope",
  }));
  const request = sinks.call.mock.calls[0][1];
  expect(JSON.stringify({ draftWrites: sinks.writes, metadataWrites: sinks.metadata.upsert.mock.calls, staged })).not.toContain(secretValue);
  expect(JSON.stringify(request)).not.toContain(secretValue);
  expect(sinks.entries()).toEqual([projectMetadata]);
});

it("round-trips Agent scope and TTL through sealing, create, and the staged descriptor", async () => {
  const agentMetadata = {
    ...projectMetadata, ttl_seconds: 3600,
    grant: { scope: "agent", agent_id: "agent-draft", conversation_id: "conversation-draft", generation: "thread-draft" },
  };
  const sinks = harness({ call: vi.fn(async () => agentMetadata) });
  await sinks.service.stage({ name: "TEST_TOKEN", value: "synthetic", scope: "agent", ttlSeconds: 3600 });
  expect(sinks.sealUpload).toHaveBeenCalledWith({
    deviceTransportPublicKeyB64: "synthetic-public-key",
    context: {
      device_id: "device-draft", creator: "device_user", request_id: projectMetadata.request_id,
      name: "TEST_TOKEN", ttl_seconds: 3600,
      grant: agentMetadata.grant,
      stage: agentMetadata.scope,
    },
    value: "synthetic",
  });
  expect(sinks.entries()).toEqual([agentMetadata]);
});

it("snapshots the selected scope before an asynchronous upload begins", async () => {
  let finishSeal;
  const sealUpload = vi.fn(() => new Promise((resolve) => { finishSeal = resolve; }));
  const sinks = harness({ sealUpload });
  const input = { name: "TEST_TOKEN", value: "synthetic", scope: "project" };

  const pending = sinks.service.stage(input);
  await vi.waitFor(() => expect(sealUpload).toHaveBeenCalledOnce());
  input.scope = "agent";
  finishSeal("synthetic-sealed-envelope");
  await expect(pending).resolves.toEqual(projectMetadata);
  expect(sinks.call).toHaveBeenCalledWith("secrets.create", expect.objectContaining({
    grant: { scope: "project", project_id: "project-draft" },
  }));
});

it("resolves a timed-out create by request id and never creates twice", async () => {
  const timeout = Object.assign(new Error("transport timeout"), { timedOut: true, uncertain: true });
  const metadata = {
    upsert: vi.fn(async () => projectMetadata),
    refresh: vi.fn(async () => [projectMetadata]),
    findRequest: vi.fn(async () => projectMetadata),
  };
  const call = vi.fn(async () => { throw timeout; });
  const sinks = harness({ call, metadata });

  await expect(sinks.service.stage({ name: "TEST_TOKEN", value: "synthetic", scope: "project" })).resolves.toEqual(projectMetadata);
  expect(call).toHaveBeenCalledTimes(1);
  expect(metadata.refresh).toHaveBeenCalledWith(expect.any(Object), { requestIds: [projectMetadata.request_id] });
});

it("clears plaintext as soon as create settles, before reconciliation finishes", async () => {
  let finishRefresh;
  const refresh = vi.fn(() => new Promise((resolve) => { finishRefresh = resolve; }));
  const metadata = {
    upsert: vi.fn(async () => projectMetadata),
    refresh,
    findRequest: vi.fn(async () => projectMetadata),
  };
  const sinks = harness({ call: vi.fn(async () => { throw new Error("uncertain"); }), metadata });
  const input = { name: "TEST_TOKEN", value: "synthetic", scope: "project", clearValue: vi.fn() };

  const pending = sinks.service.stage(input);
  await vi.waitFor(() => expect(refresh).toHaveBeenCalledOnce());
  expect(input.value).toBe("");
  expect(input.clearValue).toHaveBeenCalledOnce();
  finishRefresh([]);
  await expect(pending).resolves.toEqual(projectMetadata);
});

it("clears plaintext before a delayed metadata cache write", async () => {
  let finishUpsert;
  const upsert = vi.fn(() => new Promise((resolve) => { finishUpsert = resolve; }));
  const sinks = harness({ metadata: { upsert, refresh: vi.fn(), findRequest: vi.fn() } });
  const input = { name: "TEST_TOKEN", value: "synthetic", scope: "project", clearValue: vi.fn() };

  const pending = sinks.service.stage(input);
  await vi.waitFor(() => expect(upsert).toHaveBeenCalledOnce());
  expect(input.value).toBe("");
  expect(input.clearValue).toHaveBeenCalledOnce();
  finishUpsert(projectMetadata);
  await expect(pending).resolves.toEqual(projectMetadata);
});

it("clears plaintext when request setup fails", async () => {
  const sinks = harness({ recipient: () => { throw new Error("recipient unavailable"); } });
  const input = { name: "TEST_TOKEN", value: "synthetic", scope: "agent", clearValue: vi.fn() };

  await expect(sinks.service.stage(input)).rejects.toThrow("recipient unavailable");
  expect(input.value).toBe("");
  expect(input.clearValue).toHaveBeenCalledOnce();
});

it("clears plaintext when the selected scope is invalid", async () => {
  const sinks = harness();
  const input = { name: "TEST_TOKEN", value: "synthetic", scope: "workspace", clearValue: vi.fn() };

  await expect(sinks.service.stage(input)).rejects.toMatchObject({ code: "invalid_scope" });
  expect(input.value).toBe("");
  expect(input.clearValue).toHaveBeenCalledOnce();
  expect(sinks.sealUpload).not.toHaveBeenCalled();
});

it("uses a safe error that cannot stringify the value when create and lookup both fail", async () => {
  const secretValue = "synthetic-error-value";
  const sinks = harness({
    call: vi.fn(async () => { throw new Error(`upstream included ${secretValue}`); }),
    metadata: { upsert: vi.fn(), refresh: vi.fn(async () => []), findRequest: vi.fn(async () => null) },
  });
  let thrown;
  try {
    await sinks.service.stage({ name: "TEST_TOKEN", value: secretValue, scope: "project" });
  } catch (error) {
    thrown = error;
  }
  expect(thrown).toMatchObject({ name: "SecretDraftError", code: "stage_unresolved" });
  expect(JSON.stringify({ name: thrown.name, message: thrown.message, code: thrown.code })).not.toContain(secretValue);
});

it("removes a chip only through revoke and sends every staged id in one atomic post fragment", async () => {
  const sinks = harness();
  await sinks.service.stage({ name: "TEST_TOKEN", value: "synthetic", scope: "project" });
  expect(sinks.service.submission()).toEqual({ secret_ids: ["sec_staged"] });
  await sinks.service.remove("sec_staged");
  expect(sinks.call).toHaveBeenLastCalledWith("secrets.revoke", {
    scope: {
      project_id: "project-draft",
      stage: { owner_id: "owner-draft", conversation_id: "conversation-draft", generation: "thread-draft", draft_id: null },
    },
    secret_id: "sec_staged",
    operation_id: "22222222-2222-4222-8222-222222222222",
  });
  expect(sinks.service.submission()).toEqual({ secret_ids: [] });
});

it("does not expose or dispatch the feature unless all capabilities are present", async () => {
  expect(secretFeaturesAvailable({ attachments: true, sealedUpload: true, metadataEvents: true })).toBe(true);
  expect(secretFeaturesAvailable({ attachments: true, sealedUpload: true, metadataEvents: false })).toBe(false);
  const call = vi.fn();
  const sinks = harness({ capabilities: () => ({ attachments: true, sealedUpload: false, metadataEvents: true }), call });
  await expect(sinks.service.stage({ name: "TEST_TOKEN", value: "synthetic", scope: "project" })).resolves.toBeNull();
  expect(call).not.toHaveBeenCalled();
  expect(sinks.service.submission()).toEqual({ secret_ids: [] });
});

it("refuses a stale generation before writing the returned descriptor", async () => {
  let current = true;
  let resolve;
  const call = vi.fn(() => new Promise((done) => { resolve = done; }));
  const sinks = harness({ call, active: () => current });
  const pending = sinks.service.stage({ name: "TEST_TOKEN", value: "synthetic", scope: "project" });
  await vi.waitFor(() => expect(call).toHaveBeenCalledOnce());
  current = false;
  resolve(projectMetadata);
  await expect(pending).rejects.toMatchObject({ code: "stale_generation" });
  expect(sinks.entries()).toEqual([]);
});
