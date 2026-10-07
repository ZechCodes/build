// A workspace's Files explorer: one tree, one collapsible root per directory
// the workspace has, in its own order (#174) — "The file list to have
// collapsible roots for each directory that the workspace has. It's a common
// IDE pattern we're replicating."
//
// Each root is a row naming its directory, with a chevron, and under it the
// directory's own explorer (core/fileTree.js) standing one level deeper. A root
// reads and writes its own directory's records — its listings, its expanded
// set — so the cache rule holds per root, and a `files` push for one directory
// moves only that root. Which roots are collapsed is UI state remembered per
// workspace, in one record painted from its readback like the expanded sets;
// every root is expanded until the reader folds one.
//
// A file here is named by its root and its path together (`rootedKey`): two
// directories can hold the same path, and the open-file tabs and the preview
// stand across them.
//
// The arrows walk the whole tree: Up and Down cross from a root's last row to
// the next root's row and back, Left from a top row goes to its root, and on a
// root's row Right and Left open and fold it, Enter and Space toggle it.

import { esc } from "./text.js";
import { watchUiState } from "./localUiState.js";
import { mountFileTree } from "./fileTree.js";

/** A file across the roots: which root, and its path in that root's directory. */
export const rootedKey = (rootId, path) => JSON.stringify([rootId, path]);

/** Where a rooted key points — { root, path } — or null for anything that does
 *  not name a file under one of `roots` (a record from another build, a
 *  directory the workspace no longer has). */
export function locateRooted(roots, key) {
  try {
    const [rootId, path] = JSON.parse(key);
    const root = roots.find((candidate) => candidate.id === rootId);
    return root && typeof path === "string" && path ? { root, path } : null;
  } catch {
    return null;
  }
}

const readCollapsed = (value, roots) =>
  new Set((Array.isArray(value?.collapsed) ? value.collapsed : []).filter((id) => roots.some((root) => root.id === id)));

const rootHtml = (root) =>
  `<div class="froot" data-root="${esc(root.id)}">` +
  `<div class="frow fdir froot-head" role="treeitem" aria-level="1" aria-expanded="true" tabindex="0" data-root-head="${esc(root.id)}" style="--depth:0">` +
  `<span class="fk fchev" aria-hidden="true">▸</span><span class="fname">${esc(root.label)}</span></div>` +
  `<div class="froot-list" role="group"></div></div>`;

/**
 * mountFileRoots(listEl, { roots, collapsedAddress, treeFor, onOpen }) — draw
 * the roots into `listEl` and mount each one's explorer as it is first shown.
 *
 * - `roots`: [{ id, label }], in the workspace's order;
 * - `collapsedAddress`: the UI-state address for the collapsed set, or null;
 * - `treeFor(root)`: that root's core/fileTree.js options (its listing and
 *   expanded-set addresses, its reads);
 * - `onOpen(key)`: open a file, by its rooted key.
 *
 * Returns the explorer's interface keyed by rooted key:
 * { ready, setOpenPath, reveal, relist, dispose }.
 */
export function mountFileRoots(listEl, { roots, collapsedAddress = null, treeFor, onOpen }) {
  listEl.innerHTML = roots.map(rootHtml).join("");
  const trees = new Map(); // root id → its mounted explorer
  let disposed = false;
  let collapsed = new Set();
  let openKey = null;
  const sectionOf = (root) => [...listEl.children].find((section) => section.dataset.root === root.id);
  const headOf = (root) => sectionOf(root).querySelector("[data-root-head]");
  const shown = (root) => !collapsed.has(root.id);
  const locate = (key) => (key ? locateRooted(roots, key) : null);

  const markOpen = (root, tree) => {
    const open = locate(openKey);
    tree.setOpenPath(open?.root === root ? open.path : null);
  };

  const neighbour = (root, step) => roots[roots.indexOf(root) + step] || null;
  /** The keyboard, handed across roots: onto a root's rows (first or last), or
   *  its row when it has none showing. */
  const focusRows = (root, last) => {
    const tree = shown(root) ? trees.get(root.id) : null;
    const landed = tree && (last ? tree.focusLast() : tree.focus());
    if (!landed) headOf(root).focus();
  };
  const EDGES = {
    up: (root) => headOf(root).focus(),
    out: (root) => headOf(root).focus(),
    down: (root) => neighbour(root, 1) && headOf(neighbour(root, 1)).focus(),
  };

  const mountTree = (root) => {
    if (trees.has(root.id)) return;
    const tree = mountFileTree(sectionOf(root).querySelector(".froot-list"), {
      ...treeFor(root),
      baseDepth: 1,
      onOpen: (path) => onOpen(rootedKey(root.id, path)),
      onEdge: (edge) => EDGES[edge](root),
    });
    trees.set(root.id, tree);
    markOpen(root, tree);
  };

  const apply = (next) => {
    collapsed = next;
    for (const root of roots) {
      const open = shown(root);
      headOf(root).setAttribute("aria-expanded", String(open));
      sectionOf(root).querySelector(".froot-list").hidden = !open;
      if (open) mountTree(root);
    }
  };

  const record = collapsedAddress ? watchUiState(collapsedAddress, (value) => apply(readCollapsed(value, roots))) : null;
  const ready = Promise.resolve(record?.ready).catch(() => undefined).then((saved) => {
    // Every root open until the reader folds one; the record, once read, says
    // which they folded.
    if (!disposed) apply(readCollapsed(saved, roots));
  });
  const commit = (next) => (record ? record.write({ collapsed: [...next] }) : apply(next));
  const setShown = (root, open) => {
    const next = new Set(collapsed);
    if (open) next.delete(root.id);
    else next.add(root.id);
    return commit(next);
  };

  const rootOfHead = (event) => {
    const head = event.target.closest?.("[data-root-head]");
    return head ? roots.find((root) => root.id === head.dataset.rootHead) || null : null;
  };
  const HEAD_KEYS = {
    Enter: (root) => setShown(root, !shown(root)),
    " ": (root) => setShown(root, !shown(root)),
    ArrowRight: (root) => (shown(root) ? focusRows(root, false) : setShown(root, true)),
    ArrowLeft: (root) => setShown(root, false),
    ArrowDown: (root) => (shown(root) && trees.get(root.id)?.focus()) || EDGES.down(root),
    ArrowUp: (root) => neighbour(root, -1) && focusRows(neighbour(root, -1), true),
  };
  const onClick = (event) => {
    const root = rootOfHead(event);
    if (root) void setShown(root, !shown(root));
  };
  const onKeyDown = (event) => {
    const root = rootOfHead(event);
    const move = root && HEAD_KEYS[event.key];
    if (!move) return;
    event.preventDefault();
    void move(root);
  };
  listEl.addEventListener("click", onClick);
  listEl.addEventListener("keydown", onKeyDown);

  return {
    ready,
    /** The file in the preview, highlighted in its root and nowhere else. */
    setOpenPath(key) {
      openKey = key;
      for (const root of roots) if (trees.has(root.id)) markOpen(root, trees.get(root.id));
    },
    /** Open the file's root and every directory above it, so its row is drawn. */
    async reveal(key) {
      const open = locate(key);
      await ready;
      if (!open || disposed) return;
      if (!shown(open.root)) await setShown(open.root, true);
      await trees.get(open.root.id)?.reveal(open.path);
    },
    /** Read every shown directory of every mounted root again. */
    relist(paths = null, rootId = null) {
      if (rootId !== null) trees.get(rootId)?.relist(paths);
      else trees.forEach((tree) => tree.relist(paths));
    },
    dispose() {
      disposed = true;
      trees.forEach((tree) => tree.dispose());
      record?.dispose();
      listEl.removeEventListener("click", onClick);
      listEl.removeEventListener("keydown", onKeyDown);
    },
  };
}
