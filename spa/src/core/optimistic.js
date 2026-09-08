import { notifyError } from "./notify.js";

export const PENDING_GRACE_MS = 30000;

const PROVISIONAL_PREFIX = "pending-";

export function insertRecord(key, entry, { scope = null } = {}) {
  return { kind: "insert", key: String(key), entry, scope, settledAt: null };
}

export function removeRecord(key, { scope = null } = {}) {
  return { kind: "remove", key: String(key), scope, settledAt: null };
}

/** Whether the snapshot has caught up with a patch: by default, by carrying the
 *  fields the patch put on the row. */
const entryCarriesFields = (entry, fields) =>
  Object.keys(fields).every((name) => entry[name] === fields[name]);

/**
 * Lay `fields` over the row named by `key` until the snapshot answers for them.
 *
 * `clearedBy(entry, fields)` is what "answers" means, for a patch whose row the
 * entity will never carry as a field — a state the client shows while the
 * daemon works behind an answer it has already given. It names the push that
 * takes the patch off instead.
 */
export function patchRecord(key, fields, { scope = null, clearedBy = entryCarriesFields } = {}) {
  return { kind: "patch", key: String(key), fields, scope, clearedBy, settledAt: null };
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

export function retirePending(records, entries, { keyOf, nowMs }) {
  const held = new Map(entries.map((entry) => [String(keyOf(entry)), entry]));
  return records.filter((record) => {
    if (!record.settledAt) return true;
    if (nowMs - record.settledAt > PENDING_GRACE_MS) return false;
    if (record.kind === "remove") return held.has(record.key);
    if (record.kind === "insert") return !held.has(record.key);
    const entry = held.get(record.key);
    return Boolean(entry) && !record.clearedBy(entry, record.fields);
  });
}

const overriddenBy = (older, next) => {
  if (older.key !== next.key) return false;
  if (next.kind === "remove") return true;
  if (older.kind !== "patch" || next.kind !== "patch") return false;
  return Object.keys(older.fields).every((name) => name in next.fields);
};

export const isProvisionalKey = (key) => String(key ?? "").startsWith(PROVISIONAL_PREFIX);

let storeCount = 0;

/**
 * Keeps optimistic records local to one consumer. A new store is deliberately
 * given its own provisional-key namespace so independently mounted clients can
 * safely use the same scopes and create work at the same time.
 */
export function createOptimisticStore({ provisionalPrefix = null, resetProvisionalKeys = false } = {}) {
  const recordsByScope = new Map();
  const listenersByScope = new Map();
  let provisionalCount = 0;
  const prefix = provisionalPrefix ?? `${PROVISIONAL_PREFIX}store-${++storeCount}-`;

  const provisionalKey = (kind) => {
    provisionalCount += 1;
    return `${prefix}${kind}-${provisionalCount}`;
  };

  const pendingIn = (scope) => recordsByScope.get(scope) || [];

  const isPending = (scope, key) => pendingIn(scope).some((record) => !record.settledAt && record.key === String(key));

  const projectOptimistic = (scope, entries, { keyOf }) => projectPending(entries, pendingIn(scope), { keyOf });

  const writeScope = (scope, records) => {
    if (records.length) recordsByScope.set(scope, records);
    else recordsByScope.delete(scope);
  };

  const announce = (scopes) => {
    for (const scope of scopes) {
      const listeners = listenersByScope.get(scope);
      if (listeners) for (const listener of [...listeners]) listener();
    }
  };

  const subscribeOptimistic = (scope, listener) => {
    const listeners = listenersByScope.get(scope) || new Set();
    listeners.add(listener);
    listenersByScope.set(scope, listeners);
    return () => {
      listeners.delete(listener);
      if (!listeners.size) listenersByScope.delete(scope);
    };
  };

  const reconcileOptimistic = (scope, entries, { keyOf, nowMs = Date.now() } = {}) => {
    const records = pendingIn(scope);
    if (!records.length) return;
    const kept = retirePending(records, entries, { keyOf, nowMs });
    if (kept.length === records.length) return;
    writeScope(scope, kept);
    announce([scope]);
  };

  const reset = () => {
    recordsByScope.clear();
    listenersByScope.clear();
    if (resetProvisionalKeys) provisionalCount = 0;
  };

  const runOptimistic = async ({ scope, records, call, failureSummary, onRevert = null, notify = true }) => {
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
  };

  return {
    provisionalKey,
    pendingIn,
    isPending,
    projectOptimistic,
    subscribeOptimistic,
    reconcileOptimistic,
    reset,
    runOptimistic,
  };
}

const defaultStore = createOptimisticStore({ provisionalPrefix: PROVISIONAL_PREFIX, resetProvisionalKeys: true });

export const provisionalKey = defaultStore.provisionalKey;
export const pendingIn = defaultStore.pendingIn;
export const isPending = defaultStore.isPending;
export const projectOptimistic = defaultStore.projectOptimistic;
export const subscribeOptimistic = defaultStore.subscribeOptimistic;
export const reconcileOptimistic = defaultStore.reconcileOptimistic;
export const resetOptimistic = defaultStore.reset;
export const runOptimistic = defaultStore.runOptimistic;
