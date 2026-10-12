import { navigationDestinationOf } from "./navigationRoutes.js";
import { destinationIdentity, byVisitTime } from "./navigationPickerModel.js";
import { uiAddress, updateUiState, watchUiState } from "./localUiState.js";

export const navigationVisitAddress = uiAddress({ view: "navigation", kind: "history", sub: "visits" });
const TAB_KEY = "build.navigation.previous";
const trackers = new Set();
let generation = 0;
const validVisits = (value) => Array.isArray(value) ? value.filter((row) =>
  ["project", "workspace"].includes(row?.kind) && typeof row.key === "string" && Number.isFinite(row.at)) : [];
const readPrevious = (storage) => { try { return storage?.getItem(TAB_KEY) || null; } catch { return null; } };
const savePrevious = (storage, value) => {
  try { if (value) storage?.setItem(TAB_KEY, value); else storage?.removeItem(TAB_KEY); } catch { /* Private storage can be refused. */ }
};
const tabStorage = () => { try { return globalThis.sessionStorage; } catch { return null; } };

function mergeVisit(value, visit) {
  const identity = destinationIdentity(visit);
  const held = validVisits(value);
  const newer = held.find((row) => destinationIdentity(row) === identity && row.at > visit.at);
  return [newer || visit, ...held.filter((row) => destinationIdentity(row) !== identity)].sort(byVisitTime).slice(0, 20);
}

export function mountNavigationVisits({ storage = tabStorage(), now = Date.now, onChange = () => {} } = {}) {
  let started = generation;
  let disposed = false;
  let previous = readPrevious(storage);
  let pending = Promise.resolve();
  const record = watchUiState(navigationVisitAddress, (value) => {
    if (started === generation) onChange(validVisits(value));
  });
  const active = () => !disposed && generation === started;
  const tracker = { reset() { started = generation; previous = null; savePrevious(storage, null); onChange([]); } };
  trackers.add(tracker);
  return {
    ready: record.ready,
    visit(route) {
      if (!active()) return pending;
      const destination = navigationDestinationOf(route);
      const identity = destination ? destinationIdentity(destination) : null;
      if (identity === previous) return pending;
      previous = identity;
      savePrevious(storage, identity);
      if (!destination) return pending;
      const visit = { kind: destination.kind, key: destination.key, at: now() };
      const requested = generation;
      pending = pending.then(() => updateUiState(navigationVisitAddress, (value) => mergeVisit(value, visit),
      () => active() && requested === generation));
      return pending;
    },
    async dispose() {
      await pending;
      disposed = true;
      record.dispose();
      trackers.delete(tracker);
    },
  };
}

/** Reset is synchronous: clear this tab's reload marker and reject old queued
 * writes before the account's UI database is wiped. */
export function resetNavigationVisits() {
  generation += 1;
  for (const tracker of trackers) tracker.reset();
  savePrevious(tabStorage(), null);
}
