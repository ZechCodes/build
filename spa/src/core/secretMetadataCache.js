// Secret values never enter this module. It owns only the bridge's explicit
// metadata DTO and writes every pull before a consumer paints it.
import { mergeCachedRecordsTogether, readCached, subscribeCache } from "./localCache.js";
import { onSecretsChanged } from "./secretMetadataEvents.js";

const SECRET_METADATA_KIND = "secret-metadata";
const STATUSES = new Set(["staged", "available", "expired", "revoked", "unavailable"]);
const presentText = (value) => typeof value === "string" && value.length > 0 && value.length <= 256 && !value.includes("\0");
const nullableInteger = (value) => value === null || Number.isSafeInteger(value);
const positiveRevision = (value) => Number.isSafeInteger(value) && value >= 1;

function invalidMetadata() {
  throw Object.assign(new Error("Secret metadata is invalid"), { code: "invalid_secret_metadata" });
}

function stageMetadata(stage) {
  if (!stage || ![stage.owner_id, stage.conversation_id, stage.generation].every(presentText)) invalidMetadata();
  if (stage.draft_id !== null && !presentText(stage.draft_id)) invalidMetadata();
  return Object.freeze({
    owner_id: stage.owner_id,
    conversation_id: stage.conversation_id,
    generation: stage.generation,
    draft_id: stage.draft_id,
  });
}

function grantMetadata(grant) {
  if (grant?.scope === "environment" && presentText(grant.environment_id)) {
    return Object.freeze({ scope: "environment", environment_id: grant.environment_id });
  }
  if (grant?.scope === "project" && presentText(grant.project_id)) {
    return Object.freeze({ scope: "project", project_id: grant.project_id });
  }
  if (grant?.scope === "agent" && [grant.agent_id, grant.conversation_id, grant.generation].every(presentText)) {
    return Object.freeze({
      scope: "agent",
      agent_id: grant.agent_id,
      conversation_id: grant.conversation_id,
      generation: grant.generation,
    });
  }
  invalidMetadata();
}

/** Copy the public DTO field by field. Unknown response fields cannot reach
 * IndexedDB, a draft, a diagnostic, or another tab. */
export function secretMetadataOf(value) {
  const source = Object(value);
  const valid = [
    value,
    [source.secret_id, source.request_id, source.name].every(presentText),
    [source.ttl_seconds, source.created_at, source.attached_at, source.expires_at, source.revoked_at].every(nullableInteger),
    positiveRevision(source.revision),
    STATUSES.has(source.status),
  ].every(Boolean);
  if (!valid) invalidMetadata();
  return Object.freeze({
    secret_id: source.secret_id,
    request_id: source.request_id,
    name: source.name,
    ttl_seconds: source.ttl_seconds,
    grant: grantMetadata(source.grant),
    scope: stageMetadata(source.scope),
    created_at: source.created_at,
    attached_at: source.attached_at,
    expires_at: source.expires_at,
    revoked_at: source.revoked_at,
    revision: source.revision,
    status: source.status,
  });
}

export const secretFeaturesAvailable = (features) => features?.attachments === true
  && features.sealedUpload === true && features.metadataEvents === true;

function normalizedScope(scope) {
  if (presentText(scope?.environmentId) && !scope.projectId) return Object.freeze({ environmentId: scope.environmentId });
  if (!presentText(scope?.projectId)) throw new Error("Secret metadata requires a project");
  const project = { projectId: scope.projectId, ...(presentText(scope.environmentId) ? { environmentId: scope.environmentId } : {}) };
  if (!scope.conversationId) return Object.freeze(project);
  if (![scope.ownerId, scope.agentId, scope.conversationId, scope.generation].every(presentText)) {
    throw new Error("Secret metadata requires an exact recipient generation");
  }
  return Object.freeze({
    ...project,
    ownerId: scope.ownerId,
    agentId: scope.agentId,
    conversationId: scope.conversationId,
    generation: scope.generation,
    draftId: scope.draftId ?? null,
  });
}

const scopeKey = (scope) => JSON.stringify(normalizedScope(scope));

export function secretMetadataAddress(deviceId, rawScope) {
  const scope = normalizedScope(rawScope);
  if (!scope.projectId) return { deviceId, entityId: scope.environmentId, kind: SECRET_METADATA_KIND, sub: "environment" };
  return scope.conversationId
    ? { deviceId, entityId: scope.conversationId, kind: SECRET_METADATA_KIND, sub: `${scope.ownerId}:${scope.agentId}:${scope.generation}` }
    : { deviceId, entityId: scope.projectId, kind: SECRET_METADATA_KIND, sub: "project" };
}

const projectScope = (projectId) => ({ projectId });
const stageOf = (scope) => scope.conversationId ? {
  owner_id: scope.ownerId,
  conversation_id: scope.conversationId,
  generation: scope.generation,
  draft_id: scope.draftId,
} : undefined;

function wireScope(rawScope) {
  const scope = normalizedScope(rawScope);
  if (!scope.projectId) return { environment_id: scope.environmentId };
  return {
    project_id: scope.projectId,
    ...(scope.conversationId ? { stage: stageOf(scope) } : {}),
  };
}

function entryScope(metadata, queryScope) {
  if (metadata.grant.scope === "environment") return { environmentId: metadata.grant.environment_id };
  if (metadata.grant.scope === "project") return projectScope(metadata.grant.project_id);
  const stage = metadata.scope;
  return {
    projectId: queryScope.projectId,
    ownerId: stage.owner_id,
    agentId: metadata.grant.agent_id,
    conversationId: stage.conversation_id,
    generation: stage.generation,
    draftId: stage.draft_id,
  };
}

function belongsTo(metadata, scope) {
  if (metadata.grant.scope === "environment") return metadata.grant.environment_id === scope.environmentId;
  if (metadata.grant.scope === "project") return metadata.grant.project_id === scope.projectId;
  return Boolean(scope.conversationId)
    && metadata.grant.agent_id === scope.agentId
    && metadata.grant.conversation_id === scope.conversationId
    && metadata.grant.generation === scope.generation
    && metadata.scope.owner_id === scope.ownerId
    && metadata.scope.conversation_id === scope.conversationId
    && metadata.scope.generation === scope.generation
    && metadata.scope.draft_id === scope.draftId;
}

function hintMatches(hint, scope) {
  switch (hint.scope) {
    case "device": return true;
    case "environment": return scope.environmentId === hint.environment_id;
    case "project": return scope.projectId === hint.project_id && !scope.conversationId;
    case "agent": return scope.ownerId === hint.entity_id && scope.conversationId === hint.conversation_id && scope.generation === hint.generation;
    default: return false;
  }
}

const byId = (left, right) => left.secret_id.localeCompare(right.secret_id);
const trackedActive = (entry) => [...entry.registrations].some((registration) => registration.active());
function mergeEntries(held = [], incoming = []) {
  const entries = new Map(held.map((entry) => [entry.secret_id, entry]));
  for (const entry of incoming) {
    const previous = entries.get(entry.secret_id);
    if (!previous || entry.revision > previous.revision) entries.set(entry.secret_id, entry);
  }
  return [...entries.values()].sort(byId);
}

async function writeEntries(deviceId, scope, entries, serverNow) {
  const address = secretMetadataAddress(deviceId, scope);
  await mergeCachedRecordsTogether([address], ([held]) => [{
    secrets: mergeEntries(held?.value?.secrets, entries),
    server_now: Number.isSafeInteger(serverNow) ? serverNow : held?.value?.server_now ?? null,
  }]);
}

export async function readCachedSecrets(deviceId, scope) {
  const record = await readCached(secretMetadataAddress(deviceId, scope));
  return (record?.value?.secrets || []).map((entry) => secretMetadataOf(entry));
}

function safeReadError(code = "metadata_unavailable") {
  return Object.assign(new Error("Secret metadata is unavailable"), { name: "SecretMetadataError", code });
}

/** A per-device cache coordinator. `track` names scopes views can paint; a
 * reconnect calls `refreshTracked`, and invalidation pushes only schedule the
 * same authoritative list read. */
export function createSecretMetadataCache({ deviceId, call, capabilities, active = () => true }) {
  if (!presentText(deviceId) || typeof call !== "function" || typeof capabilities !== "function") {
    throw new Error("Secret metadata cache requires a device, call, and capabilities");
  }
  const tracked = new Map();
  let disposed = false;
  const current = () => !disposed && active();
  const supported = () => current() && secretFeaturesAvailable(capabilities());
  const scopeSupported = scope => !scope.environmentId || capabilities()?.environmentScope === true;

  async function upsert(raw, queryScope) {
    const metadata = secretMetadataOf(raw);
    const normalized = normalizedScope(queryScope || entryScope(metadata, queryScope || {}));
    if (!belongsTo(metadata, normalized)) invalidMetadata();
    const target = normalizedScope(entryScope(metadata, normalized));
    if (!current()) return null;
    await writeEntries(deviceId, target, [metadata]);
    return current() ? metadata : null;
  }

  async function writePage(scope, page) {
    if (!page || !Array.isArray(page.secrets) || !Number.isSafeInteger(page.server_now)) throw safeReadError("invalid_metadata_page");
    const groups = new Map();
    const accepted = [];
    for (const raw of page.secrets) {
      let metadata;
      try { metadata = secretMetadataOf(raw); } catch { continue; }
      // A staged descriptor can move to its final scope while this page is in
      // flight. Leave the old copy to age out on a later authoritative pull;
      // one moved entry must not discard the valid grants beside it.
      if (!belongsTo(metadata, scope)) continue;
      accepted.push(metadata);
      const target = normalizedScope(entryScope(metadata, scope));
      const key = scopeKey(target);
      const group = groups.get(key) || { scope: target, entries: [] };
      group.entries.push(metadata);
      groups.set(key, group);
    }
    if (!current()) return [];
    await Promise.all([...groups.values()].map((group) => writeEntries(deviceId, group.scope, group.entries, page.server_now)));
    return accepted;
  }

  async function readPage(scope, secretIds, requestIds, after) {
    try {
      return await call("secrets.list", {
        scope: wireScope(scope),
        ...(scope.projectId && scope.environmentId && capabilities()?.environmentScope === true ? { include_environment: true } : {}),
        secret_ids: [...secretIds], request_ids: [...requestIds], after, limit: 100,
      });
    } catch {
      throw safeReadError();
    }
  }

  const validCursor = (value) => value === null || presentText(value);
  const refreshCurrent = (scope) => {
    const trackedScope = tracked.get(scopeKey(scope));
    return current() && (!trackedScope || trackedActive(trackedScope));
  };
  const nextCursor = (page) => page.next === undefined ? null : page.next;

  async function refresh(rawScope, { secretIds = [], requestIds = [] } = {}) {
    if (!supported()) return [];
    const scope = normalizedScope(rawScope);
    if (!scopeSupported(scope)) return [];
    const entries = [];
    let after = null;
    do {
      const page = await readPage(scope, secretIds, requestIds, after);
      if (!refreshCurrent(scope)) return [];
      entries.push(...await writePage(scope, page));
      after = nextCursor(page);
      if (!validCursor(after)) throw safeReadError("invalid_metadata_page");
    } while (after !== null);
    return entries;
  }

  function track(rawScope, { active: scopeActive = () => true } = {}) {
    const scope = normalizedScope(rawScope);
    const key = scopeKey(scope);
    const entry = tracked.get(key) || { scope, registrations: new Set() };
    const wasActive = trackedActive(entry);
    const registration = { active: scopeActive };
    entry.registrations.add(registration);
    tracked.set(key, entry);
    if (!wasActive && trackedActive(entry)) void refresh(scope).catch(() => {});
    return () => {
      entry.registrations.delete(registration);
      if (!entry.registrations.size && tracked.get(key) === entry) tracked.delete(key);
    };
  }

  async function refreshTracked() {
    if (!supported()) return [];
    return Promise.all([...tracked.values()].filter(trackedActive).map((entry) => refresh(entry.scope)));
  }

  function onHint(hint) {
    if (!supported()) return;
    const candidates = [...tracked.values()].filter(entry => trackedActive(entry) && hintMatches(hint, entry.scope));
    for (const entry of candidates) {
      const filters = hint.truncated ? {} : { secretIds: hint.secret_ids };
      void refresh(entry.scope, filters).catch(() => {});
    }
  }

  const stopEvents = onSecretsChanged(deviceId, onHint);
  return Object.freeze({
    refresh,
    refreshTracked,
    track,
    upsert,
    read: (scope) => readCachedSecrets(deviceId, scope),
    async findRequest(rawScope, requestId) {
      const scope = normalizedScope(rawScope);
      const scopes = [scope];
      if (scope.conversationId) scopes.push(projectScope(scope.projectId));
      if (scope.projectId && scope.environmentId) scopes.push({ environmentId: scope.environmentId });
      const lists = await Promise.all(scopes.map((candidate) => readCachedSecrets(deviceId, candidate)));
      return lists.flat().find((entry) => entry.request_id === requestId) || null;
    },
    subscribe(rawScope, listener) {
      const address = secretMetadataAddress(deviceId, rawScope);
      return subscribeCache(address, () => { void readCachedSecrets(deviceId, rawScope).then(listener); });
    },
    dispose() {
      disposed = true;
      tracked.clear();
      stopEvents();
    },
  });
}
