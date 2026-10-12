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

export function mountNavigationVisits({ storage = globalThis.sessionStorage, now = Date.now, onChange = () => {} } = {}) {
  const started = generation;
  let disposed = false;
  let previous = readPrevious(storage);
  let pending = Promise.resolve();
  const record = watchUiState(navigationVisitAddress, (value) => {
    if (started === generation) onChange(validVisits(value));
  });
  const active = () => !disposed && generation === started;
  const tracker = { reset() { previous = null; savePrevious(storage, null); } };
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
      pending = pending.then(() => updateUiState(navigationVisitAddress, (value) =>
        [visit, ...validVisits(value).filter((row) => destinationIdentity(row) !== identity)].sort(byVisitTime).slice(0, 20), active));
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
  savePrevious(globalThis.sessionStorage, null);
}
