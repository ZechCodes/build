import { switcherModel } from "./environmentSwitcherModel.js";
import { navigationPickerModel } from "./navigationModel.js";
import { lastVisitedDestinations } from "./navigationVisitModel.js";
export { destinationIdentity, byVisitTime } from "./navigationVisitModel.js";

/** Shortcuts resolve against today's catalog, so renames keep their identity
 * and confirmed removals cannot leave a navigable stale route. */
export function globalNavigationPicker(catalog, options = {}) {
  if (Object.values(options.support || {}).some(Boolean)) return switcherModel(catalog, options);
  const { query = "" } = options;
  const picker = navigationPickerModel(catalog, { query });
  return { ...picker, lastVisited: lastVisitedDestinations(catalog, options) };
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
