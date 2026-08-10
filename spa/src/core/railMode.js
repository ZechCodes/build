// Which rail the sidebar is showing: the per-project blocks, or the flat list of
// everything. Pure functions over an injected Web-Storage-shaped object
// ({getItem, setItem}) so the module is unit-testable under node; the app passes
// localStorage. The choice is a convenience, never fatal — a browser that
// refuses storage still gets the default rail.

export const RAIL_MODE_KEY = "build.sidebar.mode";

/** The two rails, in the order the switch offers them. */
export const RAIL_MODES = ["projects", "all"];

const DEFAULT_RAIL_MODE = "projects";

/** The remembered mode. Anything missing, unrecognised, or unreadable is the
 *  per-project rail — the one you had before you ever chose. */
export function loadRailMode(storage) {
  try {
    const stored = storage.getItem(RAIL_MODE_KEY);
    return RAIL_MODES.includes(stored) ? stored : DEFAULT_RAIL_MODE;
  } catch {
    return DEFAULT_RAIL_MODE;
  }
}

/** Remember a mode. A mode that does not exist is not written, so a bad value
 *  can never be read back later. */
export function persistRailMode(mode, storage) {
  if (!RAIL_MODES.includes(mode)) return;
  try {
    storage.setItem(RAIL_MODE_KEY, mode);
  } catch {
    /* storage disabled — the rail still works, it just forgets */
  }
}
