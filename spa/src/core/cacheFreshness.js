// A feed write can remove or locally edit one row without observing its other
// rows again. Keep their original observation beside each persisted feed row;
// the in-memory symbol keeps the ordering hint out of rendered shapes.
import { entityIdOf } from "./entityId.js";

export const CACHE_FRESHNESS = Symbol("cache write order");
const OBSERVED = "__cacheObserved";

const freshnessOf = (row, record) => row?.[OBSERVED] || {
  at: Number(record?.at) || 0,
  order: Number(record?.order) || 0,
};

export const withCacheFreshness = (row, record) => {
  if (!row || !record) return row;
  const { [OBSERVED]: _persisted, ...visible } = row;
  return { ...visible, [CACHE_FRESHNESS]: freshnessOf(row, record) };
};

const rowIdentity = (row) => entityIdOf(row) || (row?.capture_id ? `capture:${row.capture_id}` : null)
  || (row?.branch ? `branch:${row.projectKey || row.project_id}:${row.branch}` : null);

/** Called by the cache's one feed write path. A local edit or prune carries
 * each surviving row's observation, even when it changes non-roster fields.
 * A bridge board read explicitly marks its rows as newly observed. */
export function feedWithObservations(next, oldRecord, stamp, newlyObserved = false) {
  const rowsFor = (field) => new Map((oldRecord?.value?.[field] || [])
    .map((row) => [rowIdentity(row), row]).filter(([key]) => key));
  const carry = (field) => {
    const oldRows = rowsFor(field);
    return next[field].map((row) => {
      const { [OBSERVED]: incoming, ...plain } = row;
      const old = !newlyObserved && oldRows.get(rowIdentity(row));
      // An undo can put a row back after the prior feed write removed it. Its
      // own persisted observation travels with it even though the current
      // record no longer names it.
      const observed = newlyObserved ? stamp : old ? freshnessOf(old, oldRecord) : incoming || stamp;
      return { ...plain, [OBSERVED]: observed };
    });
  };
  return {
    ...next,
    ...Object.fromEntries(["items", "runs"].filter((field) => Array.isArray(next[field]))
      .map((field) => [field, carry(field)])),
  };
}

export const isAtLeastAsFresh = (candidate, current) => {
  const one = candidate?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  const other = current?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  return one.at > other.at || (one.at === other.at && one.order >= other.order);
};
