// The cache record's write time travels with a row only while a feed is in
// memory. A symbol keeps this ordering hint out of wire and persisted shapes.
export const CACHE_FRESHNESS = Symbol("cache write order");

export const withCacheFreshness = (row, record) => row && record
  ? { ...row, [CACHE_FRESHNESS]: { at: Number(record.at) || 0, order: Number(record.order) || 0 } }
  : row;

export const isAtLeastAsFresh = (candidate, current) => {
  const one = candidate?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  const other = current?.[CACHE_FRESHNESS] || { at: 0, order: 0 };
  return one.at > other.at || (one.at === other.at && one.order >= other.order);
};
