export const destinationIdentity = (row) => `${row.kind}:${row.key}`;
export const byVisitTime = (left, right) => right.at - left.at || destinationIdentity(left).localeCompare(destinationIdentity(right));

/** Visit shortcuts resolve against current cache membership and names. Legacy
 * history has no 24-hour expiry and retains workspace destinations. */
export function lastVisitedDestinations(catalog, { query = "", visits = [] } = {}, accepts = () => true) {
  if (String(query).trim()) return [];
  const live = new Map(catalog.destinations.map((row) => [destinationIdentity(row), row]));
  return [...visits].sort(byVisitTime).map((visit) => live.get(destinationIdentity(visit)))
    .filter((row) => row && accepts(row)).slice(0, 5);
}
