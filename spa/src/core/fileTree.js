// The Files tab's explorer: the checkout's tree, with directories expanding in
// place under their rows the way an IDE draws it.
//
// Each directory's listing is its own `tree` record in the cache, the same one
// the sync layer re-lists when the checkout moves. Expanding a directory lists
// it — from the record when the cache holds one, and through `fs.tree`, written
// back to the record, when it does not (or when nobody else keeps this
// checkout's records true). Collapsing drops the watch and keeps the record.
// Which directories are expanded is UI state, remembered per checkout in its
// own record and painted from that record's readback like every other view.
//
// Pointer and key: on a fine pointer a click selects a file row (the keyboard
// selection) and a double-click opens it; on a coarse pointer a tap opens it.
// A click on a directory toggles it on both. Arrow keys move the selection and
// expand/collapse, Enter opens. The open file's row carries `.sel` on every
// paint; the keyboard selection is `.cursor` — two states, drawn differently.

import { readCached, subscribeCache, writeCached } from "./localCache.js";
import { watchUiState } from "./localUiState.js";
import { ancestorsOf, fileTreeHtml, treeEdge, treeKeyMove, visibleTreeRows } from "./fileTreeModel.js";

const heldRecord = (address) => (address ? readCached(address) : Promise.resolve(undefined));

const readExpanded = (value) =>
  new Set((Array.isArray(value?.expanded) ? value.expanded : []).filter((dir) => typeof dir === "string" && dir));

/** Whether every directory above `dir` is expanded, so its own listing shows. */
const reachable = (expanded, dir) => ancestorsOf(dir).every((above) => expanded.has(above));

const rowsIn = (listEl) => [...listEl.querySelectorAll(".frow[data-path]")];

/** Mark the open file and the keyboard selection on the rows already drawn.
 *  In place rather than a repaint: a double-click's two clicks must land on the
 *  same element, and a repaint between them would swap it. */
const markRows = (listEl, openPath, cursorPath) => {
  const rows = rowsIn(listEl);
  const focusRow = rows.find((row) => row.dataset.path === cursorPath) || rows[0];
  rows.forEach((row) => {
    const open = row.dataset.path === openPath;
    const cursor = row.dataset.path === cursorPath;
    row.classList.toggle("sel", open);
    row.classList.toggle("cursor", cursor);
    if (open) row.setAttribute("aria-current", "true");
    else row.removeAttribute("aria-current");
    row.setAttribute("aria-selected", String(cursor));
    row.tabIndex = row === focusRow ? 0 : -1;
  });
};

/**
 * mountFileTree(listEl, options) — draw the explorer into `listEl`.
 *
 * - `listingAddress(dir)`: the cache address of a directory's listing, or null
 *   for a mount that saves nothing;
 * - `stateAddress`: the UI-state address for the expanded set, or null;
 * - `readsForItself()`: whether a held listing is only a seed (nobody else
 *   keeps this checkout's records true), so it is painted and read anyway;
 * - `listDirectory(dir)`: the `fs.tree` read;
 * - `finePointer()`: whether a click selects rather than opens;
 * - `onOpen(path)`: open a file;
 * - `baseDepth`: how many levels the rows stand under (1 under a workspace's
 *   root row, core/fileRoots.js);
 * - `onEdge(direction)`: the arrows walked out of the rows ("up", "down",
 *   "out"); a tree with nothing around it keeps them, as it always has.
 *
 * Returns { ready, setOpenPath, reveal, relist, dispose }.
 */
export function mountFileTree(listEl, { listingAddress, stateAddress = null, readsForItself, listDirectory, finePointer, onOpen, baseDepth = 0, onEdge = null }) {
  let disposed = false;
  const listings = new Map();
  let expanded = new Set();
  let openPath = null;
  let cursorPath = null;
  let rows = [];
  const watches = new Map(); // shown directory → unwatch
  const requests = new Map(); // shown directory → the read its paints still speak for

  const focusCursor = () => rowsIn(listEl).find((row) => row.dataset.path === cursorPath)?.focus();

  const paint = () => {
    if (disposed) return;
    if (!listings.has("")) {
      listEl.innerHTML = "";
      return;
    }
    const hadFocus = listEl.contains(document.activeElement);
    rows = visibleTreeRows(listings, expanded, "", baseDepth);
    listEl.innerHTML = fileTreeHtml(rows, { openPath, cursorPath });
    if (hadFocus) focusCursor();
  };

  const shown = (dir) => !dir || (expanded.has(dir) && reachable(expanded, dir));
  const stillListing = (dir, request) => !disposed && requests.get(dir) === request && shown(dir);

  const takeListing = (dir, listing) => {
    listings.set(dir, listing);
    paint();
  };

  const reread = async (dir, request) => {
    if (!stillListing(dir, request)) return;
    const held = (await heldRecord(listingAddress(dir)))?.value;
    if (stillListing(dir, request) && held) takeListing(dir, held);
  };

  const askMachine = async (dir) => {
    try {
      const answer = await listDirectory(dir);
      return { path: answer?.path || dir, entries: answer?.entries || [] };
    } catch (error) {
      return { error: error?.message || "error" };
    }
  };

  /** A directory nothing holds a listing for (or nothing else keeps true):
   *  ask the machine, and write the answer through so the paint comes from the
   *  record — unless a newer write landed while the question was out. */
  const listFromMachine = async (dir, request, previousAt) => {
    const listing = await askMachine(dir);
    if (!stillListing(dir, request)) return;
    const address = listingAddress(dir);
    // An error is this mount's news, not the directory's: it never replaces a record.
    if (!address || listing.error) return takeListing(dir, listing);
    const current = await heldRecord(address);
    if (!stillListing(dir, request) || current?.at !== previousAt) return;
    await writeCached(address, listing);
  };

  const watch = (dir, request) => {
    watches.get(dir)?.();
    const address = listingAddress(dir);
    watches.set(dir, address ? subscribeCache(address, () => void reread(dir, request)) : () => {});
  };

  const load = async (dir) => {
    const request = (requests.get(dir) || 0) + 1;
    requests.set(dir, request);
    watch(dir, request);
    const record = await heldRecord(listingAddress(dir));
    if (!stillListing(dir, request)) return;
    if (record?.value) {
      takeListing(dir, record.value);
      if (!readsForItself()) return;
    }
    await listFromMachine(dir, request, record?.at);
  };

  const stop = (dir) => {
    watches.get(dir)?.();
    watches.delete(dir);
    requests.set(dir, (requests.get(dir) || 0) + 1);
  };

  /** Paint an expanded set: watch and list every directory it shows, drop the
   *  watch on every one it no longer shows (its record stays). */
  const applyExpanded = (next) => {
    expanded = next;
    [...watches.keys()].filter((dir) => !shown(dir)).forEach(stop);
    [...expanded].filter((dir) => shown(dir) && !watches.has(dir)).forEach((dir) => void load(dir));
    paint();
  };

  const record = stateAddress ? watchUiState(stateAddress, (value) => applyExpanded(readExpanded(value))) : null;
  const ready = Promise.resolve(record?.ready).catch(() => undefined);
  const commitExpanded = (next) =>
    record ? record.write({ expanded: [...next] }) : applyExpanded(next);

  const setExpanded = (dir, open) => {
    const next = new Set(expanded);
    if (open) next.add(dir);
    else next.delete(dir);
    return commitExpanded(next);
  };

  const setCursor = (path, { focus = false } = {}) => {
    cursorPath = path;
    markRows(listEl, openPath, cursorPath);
    if (focus) focusCursor();
  };

  const KEY_MOVES = {
    cursor: (path) => setCursor(path, { focus: true }),
    expand: (dir) => void setExpanded(dir, true),
    collapse: (dir) => void setExpanded(dir, false),
    open: (path) => onOpen(path),
  };

  const CLICKS = {
    // The second click of a double-click is not a second toggle: a folder
    // double-clicked open stays open.
    dir: (path, event) => {
      if (event.detail > 1 && finePointer()) return;
      void setExpanded(path, !expanded.has(path));
    },
    file: (path) => {
      if (!finePointer()) onOpen(path);
    },
  };

  const rowOf = (event) => event.target.closest?.(".frow[data-path]");

  const onClick = (event) => {
    const row = rowOf(event);
    if (!row) return;
    setCursor(row.dataset.path);
    CLICKS[row.dataset.kind]?.(row.dataset.path, event);
  };

  const onDoubleClick = (event) => {
    const row = rowOf(event);
    if (row?.dataset.kind === "file" && finePointer()) onOpen(row.dataset.path);
  };

  const onKeyDown = (event) => {
    const edge = onEdge && treeEdge(rows, cursorPath, event.key);
    if (edge) {
      event.preventDefault();
      onEdge(edge);
      return;
    }
    const move = treeKeyMove(rows, cursorPath, event.key);
    if (!move) return;
    event.preventDefault();
    Object.entries(move).forEach(([kind, path]) => KEY_MOVES[kind](path));
  };

  // Tab landing on a row makes it the selection the arrows move from.
  const onFocusIn = (event) => {
    const row = rowOf(event);
    if (row && row.dataset.path !== cursorPath) setCursor(row.dataset.path);
  };

  listEl.addEventListener("click", onClick);
  listEl.addEventListener("dblclick", onDoubleClick);
  listEl.addEventListener("keydown", onKeyDown);
  listEl.addEventListener("focusin", onFocusIn);
  void load("");

  return {
    ready,
    /** The file in the preview: highlighted wherever its row is drawn, and
     *  where the keyboard selection moves to. */
    setOpenPath(path) {
      openPath = path;
      if (path) cursorPath = path;
      markRows(listEl, openPath, cursorPath);
    },
    /** Put the keyboard on the selection (or the first row), for the roots
     *  around this tree handing the arrows back. False when nothing is drawn. */
    focus() {
      const row = rowsIn(listEl).find((candidate) => candidate.tabIndex === 0);
      row?.focus();
      return Boolean(row);
    },
    /** Put the keyboard on the last row drawn. */
    focusLast() {
      const row = rowsIn(listEl).at(-1);
      if (row) setCursor(row.dataset.path, { focus: true });
      return Boolean(row);
    },
    /** Expand every directory above `path`, so its row is drawn. */
    async reveal(path) {
      await ready;
      if (disposed) return;
      const missing = ancestorsOf(path).filter((dir) => !expanded.has(dir));
      if (missing.length) await commitExpanded(new Set([...expanded, ...missing]));
    },
    /** Read every shown directory again — the scope under the tree moved. */
    relist() {
      ["", ...expanded].filter(shown).forEach((dir) => void load(dir));
    },
    dispose() {
      disposed = true;
      [...watches.keys()].forEach(stop);
      record?.dispose();
      listEl.removeEventListener("click", onClick);
      listEl.removeEventListener("dblclick", onDoubleClick);
      listEl.removeEventListener("keydown", onKeyDown);
      listEl.removeEventListener("focusin", onFocusIn);
    },
  };
}
