// The Files tab's explorer, as data. The tree is the checkout's root listing
// with every expanded directory's listing nested under its row, the way an
// IDE's explorer draws it: no breadcrumb, no `..` row, a chevron on each
// directory. Listings are the per-directory `tree` records the tab already
// reads; this module only arranges what they hold and says what a key means.
//
// Two row states are drawn, and they are different things: `.sel` is the file
// open in the preview (aria-current), `.cursor` is the keyboard selection
// (aria-selected, the one row Tab lands on).

import { esc } from "./text.js";

export const joinPath = (dir, name) => (dir ? `${dir}/${name}` : name);
export const parentPath = (path) => path.split("/").slice(0, -1).join("/");

/** Pure: every directory above `path`, outermost first — what has to be
 *  expanded for its row to be visible. */
export function ancestorsOf(path) {
  const parts = path.split("/").slice(0, -1);
  return parts.map((_, index) => parts.slice(0, index + 1).join("/"));
}

const entryRow = (dir, depth, expanded) => (entry) => {
  const path = joinPath(dir, entry.name);
  const row = { path, name: entry.name, kind: entry.kind, size: Number(entry.size) || 0, depth };
  if (entry.kind === "dir") row.expanded = expanded.has(path);
  return row;
};

/** Pure: the rows the tree shows, in order. `listings` maps a directory path
 *  to its listing ({entries}) or to {error}; `expanded` is the set of expanded
 *  directory paths. A directory's children show only while it and every
 *  directory above it are expanded; one expanded with no listing yet shows
 *  as expanded and childless until its listing lands. */
export function visibleTreeRows(listings, expanded, dir = "", depth = 0) {
  const listing = listings.get(dir);
  if (!listing) return [];
  if (listing.error) return [{ kind: "error", path: dir, depth, message: listing.error }];
  return (listing.entries || []).flatMap((entry) => {
    const row = entryRow(dir, depth, expanded)(entry);
    return row.expanded ? [row, ...visibleTreeRows(listings, expanded, row.path, depth + 1)] : [row];
  });
}

const ROW_MARK = {
  dir: '<span class="fk fchev" aria-hidden="true">▸</span>',
  file: '<span class="fk" aria-hidden="true">·</span>',
  symlink: '<span class="fk" aria-hidden="true">↳</span>',
};

const ROW_CLASS = { dir: "fdir", file: "ffile", symlink: "fsym" };

const rowStates = (row, { openPath, cursorPath }, focusable) => {
  const open = row.path === openPath;
  const cursor = row.path === cursorPath;
  const classes = [`frow ${ROW_CLASS[row.kind] || "ffile"}`, open ? "sel" : "", cursor ? "cursor" : ""].filter(Boolean).join(" ");
  const expanded = row.kind === "dir" ? ` aria-expanded="${row.expanded}"` : "";
  const current = open ? ' aria-current="true"' : "";
  return `class="${classes}"${expanded}${current} aria-selected="${cursor}" tabindex="${focusable ? 0 : -1}"`;
};

const rowTail = (row) =>
  row.kind === "file" ? `<span class="fsize mono">${row.size}</span>` : "";

const rowTitle = (row) => (row.kind === "symlink" ? ' title="symlink — not followed"' : "");

// The name is its own element in every row: the tree is a fixed-width column
// that gives ground rather than growing, so a long unbroken name has to
// ellipsize inside it (.fname), and a bare text node in the row's flex line has
// no box to do that in.
const rowHtml = (marks, focusPath) => (row) => {
  if (row.kind === "error")
    return `<div class="frow ferr" style="--depth:${row.depth}" role="none">cannot list: ${esc(row.message)}</div>`;
  const states = rowStates(row, marks, row.path === focusPath);
  return `<div ${states} role="treeitem" aria-level="${row.depth + 1}" data-path="${esc(row.path)}" data-kind="${esc(row.kind)}" style="--depth:${row.depth}"${rowTitle(row)}>${ROW_MARK[row.kind] || ROW_MARK.file}<span class="fname">${esc(row.name)}</span>${rowTail(row)}</div>`;
};

/** Pure: the tree's HTML. `marks.openPath` is the file in the preview,
 *  `marks.cursorPath` the keyboard selection. Repo file names are untrusted
 *  input (spec §9): every name and path is escaped, in the row label AND in the
 *  data-path attribute the wiring reads back. */
export function fileTreeHtml(rows, marks) {
  if (!rows.length) return '<div class="empty">Empty directory.</div>';
  const navigable = rows.filter((row) => row.kind !== "error");
  const focusPath = navigable.some((row) => row.path === marks.cursorPath) ? marks.cursorPath : navigable[0]?.path;
  return rows.map(rowHtml(marks, focusPath)).join("");
}

const stepFrom = (rows, index, step) => ({ cursor: rows[Math.min(rows.length - 1, Math.max(0, index + step))].path });

const intoDirectory = (rows, index) => {
  const row = rows[index];
  if (!row.expanded) return { expand: row.path };
  const child = rows[index + 1];
  return child && child.depth > row.depth ? { cursor: child.path } : null;
};

const outOfRow = (rows, index) => {
  const row = rows[index];
  if (row.expanded) return { collapse: row.path };
  const parent = parentPath(row.path);
  return rows.some((candidate) => candidate.path === parent) && parent ? { cursor: parent } : null;
};

const toggleOrOpen = (row) => {
  if (row.kind === "file") return { open: row.path };
  if (row.kind === "dir") return row.expanded ? { collapse: row.path } : { expand: row.path };
  return null;
};

const KEY_MOVES = {
  ArrowDown: (rows, index) => stepFrom(rows, index, 1),
  ArrowUp: (rows, index) => stepFrom(rows, index, -1),
  ArrowRight: (rows, index) => (rows[index].kind === "dir" ? intoDirectory(rows, index) : null),
  ArrowLeft: outOfRow,
  Enter: (rows, index) => toggleOrOpen(rows[index]),
};

/** Pure: what a key does on the tree with the keyboard selection at
 *  `cursorPath` — {cursor}, {expand}, {collapse}, {open}, or null when the key
 *  is not the tree's. Up/Down move the selection; Right expands a directory
 *  (or steps into an expanded one), Left collapses one (or steps out to the
 *  parent); Enter opens a file and toggles a directory. */
export function treeKeyMove(rows, cursorPath, key) {
  const move = KEY_MOVES[key];
  const navigable = rows.filter((row) => row.kind !== "error");
  if (!move || !navigable.length) return null;
  const index = navigable.findIndex((row) => row.path === cursorPath);
  if (index < 0) return { cursor: navigable[0].path };
  return move(navigable, index);
}
