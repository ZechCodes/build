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

/** Carry untouched rows' observation times through a local or pruning feed
 * rewrite. A changed row gets the new feed write time instead. A fresh board
 * read does not use this helper: every row in that response was observed. */
export function preserveFeedFreshness(next, oldRecord, changed = () => false) {
  if (!oldRecord) return next;
  const keep = (field, row) => {
    const { [OBSERVED]: _previous, ...plain } = row;
    return changed(field, row) ? plain : { ...plain, [OBSERVED]: freshnessOf(row, oldRecord) };
  };
  return {
    ...next,
    ...Object.fromEntries(["items", "runs"].filter((field) => Array.isArray(next[field]))
      .map((field) => [field, next[field].map((row) => keep(field, row))])),
  };
}

export const isAtLeastAsFresh = (candidate, current) => {
  const one = candidate?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  const other = current?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  return one.at > other.at || (one.at === other.at && one.order >= other.order);
};
