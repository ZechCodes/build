import { notifyError } from "./notify.js";

export const PENDING_GRACE_MS = 30000;

const PROVISIONAL_PREFIX = "pending-";

export function insertRecord(key, entry, { scope = null } = {}) {
  return { kind: "insert", key: String(key), entry, scope, settledAt: null };
}

export function removeRecord(key, { scope = null } = {}) {
  return { kind: "remove", key: String(key), scope, settledAt: null };
}

export function patchRecord(key, fields, { scope = null } = {}) {
  return { kind: "patch", key: String(key), fields, scope, settledAt: null };
}

export function projectPending(entries, records, { keyOf }) {
  const named = (entry) => String(keyOf(entry));
  return records.reduce((projected, record) => {
    if (record.kind === "remove") return projected.filter((entry) => named(entry) !== record.key);
    if (record.kind === "insert") {
      if (projected.some((entry) => named(entry) === record.key)) return projected;
      return [...projected, record.entry];
    }
    return projected.map((entry) => (named(entry) === record.key ? { ...entry, ...record.fields } : entry));
  }, [...entries]);
}

const entryCarriesFields = (entry, fields) =>
  Object.keys(fields).every((name) => entry[name] === fields[name]);

export function retirePending(records, entries, { keyOf, nowMs }) {
  const held = new Map(entries.map((entry) => [String(keyOf(entry)), entry]));
  return records.filter((record) => {
    if (!record.settledAt) return true;
    if (nowMs - record.settledAt > PENDING_GRACE_MS) return false;
    if (record.kind === "remove") return held.has(record.key);
    if (record.kind === "insert") return !held.has(record.key);
    const entry = held.get(record.key);
    return Boolean(entry) && !entryCarriesFields(entry, record.fields);
  });
}

const overriddenBy = (older, next) => {
  if (older.key !== next.key) return false;
  if (next.kind === "remove") return true;
  if (older.kind !== "patch" || next.kind !== "patch") return false;
  return Object.keys(older.fields).every((name) => name in next.fields);
};

const recordsByScope = new Map();
const listenersByScope = new Map();
let provisionalCount = 0;

export function provisionalKey(kind) {
  provisionalCount += 1;
  return `${PROVISIONAL_PREFIX}${kind}-${provisionalCount}`;
}

export const isProvisionalKey = (key) => String(key ?? "").startsWith(PROVISIONAL_PREFIX);

export function pendingIn(scope) {
  return recordsByScope.get(scope) || [];
}

export function isPending(scope, key) {
  return pendingIn(scope).some((record) => !record.settledAt && record.key === String(key));
}

export function projectOptimistic(scope, entries, { keyOf }) {
  return projectPending(entries, pendingIn(scope), { keyOf });
}

function writeScope(scope, records) {
  if (records.length) recordsByScope.set(scope, records);
  else recordsByScope.delete(scope);
}

function announce(scopes) {
  for (const scope of scopes) {
    const listeners = listenersByScope.get(scope);
    if (listeners) for (const listener of [...listeners]) listener();
  }
}

export function subscribeOptimistic(scope, listener) {
  const listeners = listenersByScope.get(scope) || new Set();
  listeners.add(listener);
  listenersByScope.set(scope, listeners);
  return () => {
    listeners.delete(listener);
    if (!listeners.size) listenersByScope.delete(scope);
  };
}

export function reconcileOptimistic(scope, entries, { keyOf, nowMs = Date.now() } = {}) {
  const records = pendingIn(scope);
  if (!records.length) return;
  const kept = retirePending(records, entries, { keyOf, nowMs });
  if (kept.length === records.length) return;
  writeScope(scope, kept);
  announce([scope]);
}

export function resetOptimistic() {
  recordsByScope.clear();
  listenersByScope.clear();
  provisionalCount = 0;
}

export async function runOptimistic({ scope, records, call, failureSummary, onRevert = null, notify = true }) {
  const held = records.map((record) => ({ scope: record.scope || scope, record: { ...record } }));
  const touched = new Set(held.map((entry) => entry.scope));
  for (const entry of held) {
    const standing = pendingIn(entry.scope).filter((older) => !overriddenBy(older, entry.record));
    writeScope(entry.scope, [...standing, entry.record]);
  }
  announce(touched);

  const replace = (entry, next) => {
    writeScope(
      entry.scope,
      pendingIn(entry.scope).map((standing) => (standing === entry.record ? next : standing)),
    );
    entry.record = next;
  };
  const take = (entry) => {
    writeScope(
      entry.scope,
      pendingIn(entry.scope).filter((standing) => standing !== entry.record),
    );
  };
  const heldFor = (key) => held.find((entry) => entry.record.key === String(key)) || null;

  const handle = {
    rekey(fromKey, toKey, replacementEntry = null) {
      const entry = heldFor(fromKey);
      if (!entry) return false;
      const renamed = { ...entry.record, key: String(toKey), settledAt: Date.now() };
      if (replacementEntry) renamed.entry = replacementEntry;
      replace(entry, renamed);
      announce([entry.scope]);
      return true;
    },
    moveScope(fromScope, toScope) {
      const moving = held.filter((entry) => entry.scope === fromScope);
      if (!moving.length) return false;
      for (const entry of moving) {
        take(entry);
        entry.scope = toScope;
        writeScope(toScope, [...pendingIn(toScope), entry.record]);
        touched.add(toScope);
      }
      announce([fromScope, toScope]);
      return true;
    },
    drop(key) {
      const entry = heldFor(key);
      if (!entry) return false;
      take(entry);
      held.splice(held.indexOf(entry), 1);
      announce([entry.scope]);
      return true;
    },
  };

  try {
    await call(handle);
  } catch (error) {
    for (const entry of held.filter((standing) => !standing.record.settledAt)) take(entry);
    announce(touched);
    if (onRevert) onRevert(error);
    if (notify) notifyError(failureSummary, error.message);
    return false;
  }
  const settledAt = Date.now();
  for (const entry of held.filter((standing) => !standing.record.settledAt)) {
    replace(entry, { ...entry.record, settledAt });
  }
  announce(touched);
  return true;
}
