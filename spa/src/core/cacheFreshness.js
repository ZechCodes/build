// A feed write can remove or locally edit one row without observing its other
// rows again. Keep their original observation beside each persisted feed row;
// the in-memory symbol keeps the ordering hint out of rendered shapes.
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

/** Carry rows' observation times through a local or pruning feed rewrite.
 * Even a locally edited row's agent roster was not observed again. A fresh
 * board read does not use this helper: every row in it was observed. */
export function preserveFeedFreshness(next, oldRecord) {
  if (!oldRecord) return next;
  const keep = (row) => {
    const { [OBSERVED]: _previous, ...plain } = row;
    return { ...plain, [OBSERVED]: freshnessOf(row, oldRecord) };
  };
  return {
    ...next,
    ...Object.fromEntries(["items", "runs"].filter((field) => Array.isArray(next[field]))
      .map((field) => [field, next[field].map(keep)])),
  };
}

export const isAtLeastAsFresh = (candidate, current) => {
  const one = candidate?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  const other = current?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  return one.at > other.at || (one.at === other.at && one.order >= other.order);
};
