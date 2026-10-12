import { navigationPickerModel } from "./navigationModel.js";

export const destinationIdentity = (row) => `${row.kind}:${row.key}`;
export const byVisitTime = (left, right) => right.at - left.at || destinationIdentity(left).localeCompare(destinationIdentity(right));

/** Shortcuts resolve against today's catalog, so renames keep their identity
 * and confirmed removals cannot leave a navigable stale route. */
export function globalNavigationPicker(catalog, { query = "", visits = [] } = {}) {
  const picker = navigationPickerModel(catalog, { query });
  const live = new Map(catalog.destinations.map((row) => [destinationIdentity(row), row]));
  const lastVisited = String(query).trim() ? [] : [...visits].sort(byVisitTime)
    .map((visit) => live.get(destinationIdentity(visit))).filter(Boolean).slice(0, 5);
  return { ...picker, lastVisited };
}

/** Keep occurrence ordering for this query/scope. Payloads remain live; new
 * occurrences append, and vanished ones leave without moving kept rows. */
export function reconcilePickerOrder(previous, next) {
  const groupKeys = [...new Set([...previous, ...next].map((row) => row.groupKey))];
  return groupKeys.flatMap((groupKey) => reconcileGroupOrder(
    previous.filter((row) => row.groupKey === groupKey), next.filter((row) => row.groupKey === groupKey),
  ));
}

function reconcileGroupOrder(previous, next) {
  const live = new Map(next.map((row) => [row.key, row]));
  const held = new Set(previous.map((row) => row.key));
  return [...previous.map((row) => live.get(row.key)).filter(Boolean), ...next.filter((row) => !held.has(row.key))];
}
