// @vitest-environment jsdom
import { afterEach, expect, it, vi } from "vitest";
import { sealSecretUpload, encryptFrame, decryptEnvelope } from "@build/secure-transport";
import { onSecretsChanged, secretsChangedOn } from "../src/core/secretMetadataEvents.js";
import * as v1 from "../src/core/bridgeApi/v1/index.js";
import { bridgeCapabilities, dispatchChangeEvent, greetBridge, resetChangeEvents } from "../src/core/changeEvents.js";

it("resolves secret sealing and the original transport API through the package entry", () => {
  for (const method of [sealSecretUpload, encryptFrame, decryptEnvelope]) expect(method).toBeTypeOf("function");
});

it("keeps secret features off until the current greeting names each one", () => {
  expect(bridgeCapabilities("ungreeted-secrets").secrets).toEqual({ attachments: false, sealedUpload: false, metadataEvents: false, environmentScope: false });
  expect(v1.capabilitiesOf({ api_version: "3.36.0", capabilities: [] }).secrets).toEqual({ attachments: false, sealedUpload: false, metadataEvents: false, environmentScope: false });
  for (const feature of ["attachments", "sealedUpload", "metadataEvents"]) {
    const flags = v1.capabilitiesOf({ api_version: "3.36.0", capabilities: [`secrets.${feature}`] }).secrets;
    expect(flags[feature]).toBe(true);
    expect(Object.values(flags).filter(Boolean)).toHaveLength(1);
  }
  expect(v1.capabilitiesOf({
    api_version: "3.34.0",
    capabilities: ["secrets.attachments", "secrets.sealedUpload", "secrets.metadataEvents"],
  }).secrets).toEqual({ attachments: false, sealedUpload: false, metadataEvents: false, environmentScope: false });
});

it("recognizes the metadata invalidation without treating it as a reply", () => {
  const event = { type: "secrets.changed", scope: "project", project_id: "project-wire", revision: 1, secret_ids: ["sec_synthetic"] };
  expect(v1.parseEvent(event)).toBe(event);
  expect(dispatchChangeEvent(event, "ungreeted-secrets")).toBe(true);
});

afterEach(() => resetChangeEvents());
const projectEvent = { type: "secrets.changed", scope: "project", project_id: "project-wire", revision: 2, secret_ids: ["sec_synthetic"] };

it("routes project hints without their originating conversation and fences current greeting authority", async () => {
  const hear = vi.fn(), foreign = vi.fn();
  const stop = onSecretsChanged("project-device", hear), stopForeign = onSecretsChanged("other-device", foreign);
  dispatchChangeEvent(projectEvent, "project-device");
  expect(hear).not.toHaveBeenCalled();
  let current = true;
  await greetBridge(async () => ({ api_version: "3.36.0", push_events: true, capabilities: ["secrets.metadataEvents"], events: ["secrets.changed"] }),
    { deviceId: "project-device", isCurrent: () => current });
  dispatchChangeEvent({ ...projectEvent, name: "never forwarded", value: "synthetic value", sealed_payload: "synthetic envelope" }, "project-device");
  expect(hear).toHaveBeenCalledWith({ scope: "project", project_id: "project-wire", revision: 2, secret_ids: ["sec_synthetic"] });
  expect(foreign).not.toHaveBeenCalled();
  current = false;
  dispatchChangeEvent({ ...projectEvent, revision: 3 }, "project-device");
  expect(hear).toHaveBeenCalledTimes(1);
  stop(); stopForeign();
});

it("keeps reordered hints as refresh requests and refuses malformed scope metadata", () => {
  const hear = vi.fn(), stop = onSecretsChanged("hint-device", hear);
  const agent = { type: "secrets.changed", scope: "agent", entity_id: "owner", conversation_id: "conversation", thread_id: "thread", generation: "thread", revision: 4, secret_ids: ["sec_synthetic"] };
  secretsChangedOn("hint-device", agent);
  secretsChangedOn("hint-device", { ...agent, revision: 3 });
  expect(hear.mock.calls.map(([hint]) => hint.revision)).toEqual([4, 3]);
  for (const event of [{ ...agent, generation: "foreign-thread" }, { ...agent, thread_id: "" }, { ...agent, revision: 0 }, { ...agent, secret_ids: Array(101).fill("sec_synthetic") }, { ...agent, secret_ids: [] }, { ...agent, scope: "unknown" }]) {
    secretsChangedOn("hint-device", event);
  }
  expect(hear).toHaveBeenCalledTimes(2);
  secretsChangedOn("hint-device", { ...agent, secret_ids: [], truncated: true });
  expect(hear).toHaveBeenLastCalledWith({ scope: "agent", entity_id: "owner", conversation_id: "conversation", thread_id: "thread", generation: "thread", revision: 4, secret_ids: [], truncated: true });
  expect(hear).toHaveBeenCalledTimes(3);
  stop();
  secretsChangedOn("hint-device", agent);
  expect(hear).toHaveBeenCalledTimes(3);
});

it("turns a bounded device overflow hint into a full metadata refresh", () => {
  const hear = vi.fn();
  const dispose = onSecretsChanged("overflow-device", hear);
  secretsChangedOn("overflow-device", { type: "secrets.changed", scope: "device", revision: 1001, secret_ids: [], truncated: true, name: "discarded", value: "discarded" });
  expect(hear).toHaveBeenCalledWith({ scope: "device", revision: 1001, secret_ids: [], truncated: true });
  dispose();
});


it("advertises environment secrets independently and projects environment hints without values", () => {
  expect(v1.capabilitiesOf({ api_version: "3.40.0", capabilities: ["secrets.environmentScope"] }).environments.secrets).toBe(true);
  expect(v1.capabilitiesOf({ api_version: "3.39.0", capabilities: [] }).environments.secrets).toBe(false);
  const hear = vi.fn(), stop = onSecretsChanged("environment-wire", hear);
  secretsChangedOn("environment-wire", { scope: "environment", environment_id: "env-wire", revision: 3,
    secret_ids: ["sec_env"], value: "never forward", name: "never forward" });
  expect(hear).toHaveBeenCalledWith({ scope: "environment", environment_id: "env-wire", revision: 3, secret_ids: ["sec_env"] });
  secretsChangedOn("environment-wire", { scope: "environment", environment_id: "", revision: 4, secret_ids: ["sec_env"] });
  expect(hear).toHaveBeenCalledOnce(); stop();
});

it("binds the environment upload to StageContext, then lists and revokes by environment selector", async () => {
  const { createSecretDraftService } = await import("../src/core/secretDrafts.js");
  const { createSecretMetadataCache } = await import("../src/core/secretMetadataCache.js");
  const flags = () => ({ attachments: true, sealedUpload: true, metadataEvents: true, environmentScope: true });
  const descriptor = { secret_id: "sec_environment_wire", request_id: "557ab895-85e0-4c94-82ad-f16fcb0dd11c", name: "ENV_TOKEN", ttl_seconds: null,
    grant: { scope: "environment", environment_id: "env-wire" },
    scope: { owner_id: "owner-wire", conversation_id: "conversation-wire", generation: "thread-wire", draft_id: null },
    created_at: 1800000000, attached_at: null, expires_at: null, revoked_at: null, revision: 1, status: "staged" };
  const call = vi.fn(async (method, params) => {
    if (method === "secrets.create") {
      expect(params.scope).toEqual(descriptor.scope); expect(params.grant).toEqual(descriptor.grant);
      expect(JSON.stringify(params)).not.toContain("synthetic environment wire value");
      return descriptor;
    }
    if (method === "secrets.list") return { secrets: [descriptor], next: null, server_now: 1800000000 };
    return { ...descriptor, status: "revoked", revoked_at: 1800000010, revision: 2 };
  });
  const cache = createSecretMetadataCache({ deviceId: "env-wire-device", capabilities: flags, call });
  let entries = [];
  const service = createSecretDraftService({ deviceId: "env-wire-device", creator: "device_user", projectId: "project-wire", environmentId: () => "env-wire",
    recipient: () => ({ ownerId: "owner-wire", agentId: "agent-wire", conversationId: "conversation-wire", generation: "thread-wire" }),
    capabilities: flags, deviceTransportPublicKey: () => "synthetic-device-public-key",
    sealUpload: async input => {
      expect(input.context.grant).toEqual(descriptor.grant); expect(input.context.stage).toEqual(descriptor.scope);
      return "synthetic-sealed-upload";
    },
    call, metadata: cache, draft: { read: () => entries, write: next => { entries = next; } }, createRequestId: () => descriptor.request_id });
  try {
    await service.stage({ name: "ENV_TOKEN", value: "synthetic environment wire value", scope: "environment" });
    await cache.refresh({ environmentId: "env-wire" });
    expect(call).toHaveBeenCalledWith("secrets.list", expect.objectContaining({ scope: { environment_id: "env-wire" } }));
    await service.remove(descriptor.secret_id);
    expect(call).toHaveBeenCalledWith("secrets.revoke", expect.objectContaining({ scope: { environment_id: "env-wire" } }));
    expect(call.mock.calls.some(([method]) => method === "thread.post")).toBe(false);
  } finally { cache.dispose(); }
});
