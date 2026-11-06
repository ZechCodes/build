// Staging is an immediate user action. The sealed upload is deliberately sent
// outside the persisted write queue; uncertainty is resolved by request id,
// never by manufacturing another create.
import { sealSecretUpload as defaultSealUpload } from "@build/secure-transport";
import { secretFeaturesAvailable, secretMetadataOf } from "./secretMetadataCache.js";

export { secretFeaturesAvailable } from "./secretMetadataCache.js";

const safeError = (code, message) => Object.assign(new Error(message), { name: "SecretDraftError", code });
const cloneEntries = (entries = []) => entries.map(secretMetadataOf);

function stageContext(recipient) {
  return {
    owner_id: recipient.ownerId,
    conversation_id: recipient.conversationId,
    generation: recipient.generation,
    draft_id: recipient.draftId ?? null,
  };
}

function grantFor(scope, recipient, projectId) {
  if (scope === "project") return { scope: "project", project_id: projectId };
  if (scope === "agent") return {
    scope: "agent",
    agent_id: recipient.agentId,
    conversation_id: recipient.conversationId,
    generation: recipient.generation,
  };
  throw safeError("invalid_scope", "Choose Agent or Project scope");
}

const queryScope = (projectId, recipient) => ({
  projectId,
  ownerId: recipient.ownerId,
  agentId: recipient.agentId,
  conversationId: recipient.conversationId,
  generation: recipient.generation,
  draftId: recipient.draftId ?? null,
});

function sameStage(metadata, context, grant, requestId, name) {
  return metadata.request_id === requestId && metadata.name === name
    && JSON.stringify(metadata.scope) === JSON.stringify(context)
    && JSON.stringify(metadata.grant) === JSON.stringify(grant);
}

const DEFAULTS = Object.freeze({
  sealUpload: defaultSealUpload,
  createRequestId: () => crypto.randomUUID(),
  createOperationId: () => crypto.randomUUID(),
  active: () => true,
  cachedSupport: () => false,
});

function assertBindings({ recipient, capabilities, deviceTransportPublicKey, call, draft, metadata }) {
  const valid = [recipient, capabilities, deviceTransportPublicKey, call].every((value) => typeof value === "function")
    && [draft?.read, draft?.write, metadata?.upsert].every((value) => typeof value === "function");
  if (!valid) throw new Error("Secret draft service requires recipient, wire, draft, and metadata bindings");
}

function clearLiveValue(input) {
  try { input.value = ""; } catch { /* A caller may freeze its submission record. */ }
  try { input.clearValue?.(); } catch { /* The modal owns its live field. */ }
}

export function createSecretDraftService(provided) {
  const options = { ...DEFAULTS, ...provided };
  assertBindings(options);
  const {
    deviceId, creator, projectId, recipient, capabilities, deviceTransportPublicKey, call,
    draft, metadata, sealUpload, createRequestId, createOperationId, active, cachedSupport,
  } = options;
  const current = () => active() === true;
  const wireSupported = () => current() && secretFeaturesAvailable(capabilities());
  const displaySupported = () => current() && (cachedSupport() === true || wireSupported());

  function entries() {
    return displaySupported() ? cloneEntries(draft.read()) : [];
  }

  function stageRequest({ name, value, scope = "agent", ttlSeconds = null }) {
    const addressed = recipient();
    const context = stageContext(addressed);
    const grant = grantFor(scope, addressed, projectId);
    return { addressed, context, grant, requestId: createRequestId(), name, value, ttlSeconds };
  }

  async function sealedRequest(request) {
    try {
      return await sealUpload({
        deviceTransportPublicKeyB64: deviceTransportPublicKey(),
        context: {
          device_id: deviceId, creator, request_id: request.requestId, name: request.name,
          ttl_seconds: request.ttlSeconds, grant: request.grant, stage: request.context,
        },
        value: request.value,
      });
    } catch {
      throw safeError("stage_unresolved", "The secret upload could not be confirmed");
    }
  }

  async function createOrResolve(request, sealed, createSettled) {
    try {
      const answer = await call("secrets.create", {
        request_id: request.requestId,
        device_id: deviceId,
        creator,
        scope: request.context,
        grant: request.grant,
        name: request.name,
        ttl_seconds: request.ttlSeconds,
        sealed_payload: sealed,
      });
      createSettled();
      return answer;
    } catch {
      createSettled();
      const scopeAddress = queryScope(projectId, request.addressed);
      try { await metadata.refresh(scopeAddress, { requestIds: [request.requestId] }); } catch { /* The lookup below decides. */ }
      const answer = await metadata.findRequest?.(scopeAddress, request.requestId);
      if (answer) return answer;
      throw safeError("stage_unresolved", "The secret upload could not be confirmed");
    }
  }

  async function acceptStage(request, answer) {
    if (!current()) throw safeError("stale_generation", "The recipient conversation changed");
    const descriptor = secretMetadataOf(answer);
    if (!sameStage(descriptor, request.context, request.grant, request.requestId, request.name)) {
      throw safeError("stage_mismatch", "The secret upload returned different metadata");
    }
    await metadata.upsert(descriptor, queryScope(projectId, request.addressed));
    if (!current()) throw safeError("stale_generation", "The recipient conversation changed");
    draft.write([...cloneEntries(draft.read()), descriptor]);
    return descriptor;
  }

  async function stage(input) {
    let request;
    let sealed;
    let cleared = false;
    const clear = () => {
      if (cleared) return;
      cleared = true;
      if (request) request.value = "";
      clearLiveValue(input);
    };
    try {
      if (!wireSupported()) throw safeError("stage_unavailable", "The secret could not be attached");
      request = stageRequest(input);
      sealed = await sealedRequest(request);
      return await acceptStage(request, await createOrResolve(request, sealed, clear));
    } finally {
      sealed = "";
      clear();
    }
  }

  async function remove(secretId) {
    if (!wireSupported()) throw safeError("revoke_unavailable", "The secret could not be revoked");
    const held = entries().find((entry) => entry.secret_id === secretId);
    if (!held) return null;
    const addressed = recipient();
    let answer;
    try {
      answer = await call("secrets.revoke", {
        scope: {
          project_id: projectId,
          stage: stageContext(addressed),
        },
        secret_id: secretId,
        operation_id: createOperationId(),
      });
    } catch {
      throw safeError("revoke_unresolved", "The secret revocation could not be confirmed");
    }
    if (!current()) throw safeError("stale_generation", "The recipient conversation changed");
    const descriptor = secretMetadataOf(answer);
    await metadata.upsert(descriptor, queryScope(projectId, addressed));
    draft.write(entries().filter((entry) => entry.secret_id !== secretId));
    return descriptor;
  }

  return Object.freeze({
    stage,
    remove,
    entries,
    submission: () => ({ secret_ids: entries().map((entry) => entry.secret_id) }),
  });
}
