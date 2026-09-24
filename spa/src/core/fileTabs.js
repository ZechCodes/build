// The Files tab's open files: a strip of closable tabs over the preview. The
// open set and the active tab are UI state, remembered per checkout in one
// record and painted from that record's readback; the view is told which file
// to show when the active tab moves.
//
// Unsaved edits belong to the view, not to this record. It hands in the paths
// that hold them so their tabs are marked, a close of one asks first, and a
// tab holding edits is never dropped by a layout written somewhere else (the
// same checkout open in another window).

import { watchUiState } from "./localUiState.js";
import { activateTab, closeTab, fileTabsHtml, openTab, readTabLayout } from "./fileTabsModel.js";

const keepingDirty = (layout, dirty) => {
  const kept = [...dirty].filter((path) => !layout.tabs.includes(path));
  return kept.length ? { ...layout, tabs: [...layout.tabs, ...kept] } : layout;
};

/**
 * mountFileTabs(stripEl, options) — the tab strip.
 *
 * - `stateAddress`: the UI-state address for {tabs, active}, or null for a
 *   mount that saves nothing;
 * - `dirtyPaths()`: the set of open paths holding unsaved edits;
 * - `confirmClose(path)`: resolves true when closing a tab with unsaved edits
 *   may throw them away;
 * - `onClose(path)`: a tab is being closed for good — drop what it held;
 * - `onShow(path)`: the active tab moved (null: nothing is open);
 * - `initial`: a path to open on top of the remembered layout (the route's).
 *
 * Returns { ready, open, refresh, dispose }.
 */
export function mountFileTabs(stripEl, { stateAddress = null, dirtyPaths, confirmClose, onClose, onShow, initial = null }) {
  let disposed = false;
  let started = false;
  let layout = { tabs: [], active: null };
  let shown = null; // the mount starts with nothing open, which the view already shows
  let marked = "";

  const markedKey = () => [...dirtyPaths()].sort().join("\n");

  const paint = (value) => {
    if (disposed || !started) return;
    layout = keepingDirty(readTabLayout(value), dirtyPaths());
    marked = markedKey();
    stripEl.innerHTML = fileTabsHtml(layout, dirtyPaths());
    stripEl.hidden = !layout.tabs.length;
    if (layout.active === shown) return;
    shown = layout.active;
    onShow(shown);
  };

  const record = stateAddress ? watchUiState(stateAddress, paint) : null;
  const commit = (next) => (record ? record.write(next) : paint(next));

  const ready = (async () => {
    const saved = await Promise.resolve(record?.ready).catch(() => undefined);
    started = true;
    const remembered = readTabLayout(saved);
    if (initial) await commit(openTab(remembered, initial));
    else paint(remembered);
  })();

  const close = async (path) => {
    if (dirtyPaths().has(path) && !(await confirmClose(path))) return;
    if (disposed) return;
    onClose(path);
    await commit(closeTab(layout, path));
  };

  const onClick = (event) => {
    const closing = event.target.closest?.("[data-tab-close]");
    if (closing) return void close(closing.dataset.tabClose);
    const tab = event.target.closest?.("[data-tab-path]");
    if (tab) void commit(activateTab(layout, tab.dataset.tabPath));
  };

  stripEl.addEventListener("click", onClick);

  return {
    ready,
    /** Open a file: its tab if it has one, else a new one; either way, active. */
    async open(path) {
      await ready;
      if (!disposed) await commit(openTab(layout, path));
    },
    /** Repaint the unsaved marks when they changed. */
    refresh() {
      if (started && markedKey() !== marked) paint(layout);
    },
    dispose() {
      disposed = true;
      record?.dispose();
      stripEl.removeEventListener("click", onClick);
    },
  };
}
