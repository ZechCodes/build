// One file of a diff stack, as the keyed list draws it.
//
// A file's html is a function of three things and nothing else: what the shape
// says about it (path, status, weights, content key), the fold the reader put
// it in, and the body that is cached for it. So an unchanged file's entry is
// byte-identical from paint to paint and the reconciler leaves its element
// alone; a collapsed file costs a header and a peek whatever its diff weighs;
// and a file whose body has not arrived says so instead of drawing nothing.
//
// Two constructors make the view, and they are the only place the two sources
// of a stack differ: an uncommitted file comes from the status shape and wears
// the bridge's content_key, a git.show file comes from a parsed patch and wears
// a hash of its own rows.

import { fileKey, parseDiff } from "./diff.js";
import { diffStackEntries, fileBodyHtml, fileFoldOf, fileFrameHtml } from "./diffRender.js";
import { bodyMatches } from "./fileDiffs.js";
import { hashFileRows } from "./reviewMemory.js";

/** How much of a collapsed file's diff is worth keeping in the document: a peek,
 *  not a diff. */
export const COLLAPSED_PREVIEW_ROWS = 8;

const SHUT = "shut";

// The one word the stack shows for a file, from the letters git status uses.
const STATUS_WORD = { A: "ADD", "?": "ADD", D: "DEL" };

const statusWordOf = (statusFile) =>
  STATUS_WORD[statusFile.worktree_status] || STATUS_WORD[statusFile.index_status] || "EDIT";

/** One file of a `git.status` shape as the stack's view of it. Its rows are not
 *  in the shape — they arrive as a body, or never. */
export function fileViewFromStatus(statusFile) {
  return {
    path: statusFile.path,
    status: statusWordOf(statusFile),
    add: Number(statusFile.added) || 0,
    del: Number(statusFile.deleted) || 0,
    contentKey: statusFile.content_key,
    rows: null,
  };
}

/** One file of a parsed patch (a `git.show` payload, the review aggregate) as
 *  the same view. Its content key is a hash of the rows it came with, so the
 *  re-review chip compares the same way on both stacks. */
export function fileViewFromParsedFile(parsedFile) {
  return {
    path: parsedFile.path,
    status: parsedFile.status,
    add: parsedFile.add,
    del: parsedFile.del,
    contentKey: hashFileRows(parsedFile),
    rows: parsedFile.rows,
  };
}

/** The rows to draw: the view's own where a payload carried them, else the
 *  cached body's — and none at all while the body is missing or holds what the
 *  file said before its last edit. */
function rowsOf(view, body) {
  if (view.rows) return view.rows;
  if (!bodyMatches(body, view.contentKey)) return null;
  const parsed = parseDiff(body.patch)[0];
  return parsed ? parsed.rows : [];
}

const noBodyHtml = (label) => `<div class="dscroll"><div class="dload">${label}</div></div>`;

const previewOf = (file) => ({ ...file, rows: file.rows.slice(0, COLLAPSED_PREVIEW_ROWS) });

function foldedBodyHtml(file, fold, options) {
  const collapsed = fold === SHUT;
  if (!file.rows) return noBodyHtml(collapsed ? "expand to load this file" : "loading…");
  return fileBodyHtml(collapsed ? previewOf(file) : file, options);
}

/** One file's entry for the keyed list: its key, and the html of it in the fold
 *  it is in. `options.body` is the cached `{ content_key, patch, truncated }`;
 *  every other option is the stack's (folds, viewed, changedSince, fileMenu…). */
export function fileEntry(view, options = {}) {
  const file = { ...view, rows: rowsOf(view, options.body) };
  const fold = fileFoldOf(file, options);
  return { key: fileKey(file), html: fileFrameHtml(file, options, foldedBodyHtml(file, fold, options)) };
}

const noBodies = () => undefined;

/** A whole stack of views as the entries `patchList` paints: noise grouped and
 *  triage ordering as ever, with every file drawn through the fold-aware entry.
 *  `bodyOf(path)` answers the body cached for a file, or undefined. */
export function fileStackEntries(views, options = {}) {
  const bodyOf = options.bodyOf || noBodies;
  const hydrated = views.map((view) => ({ ...view, rows: rowsOf(view, bodyOf(view.path)) }));
  return diffStackEntries(hydrated, { ...options, renderFile: (file, fileOptions) => fileEntry(file, fileOptions).html });
}

/** The paths whose bodies are on screen — everything the reader has not folded
 *  shut. Those are the files worth fetching eagerly. */
export function openFilePaths(views, options = {}) {
  return new Set(views.filter((view) => fileFoldOf(view, options) !== SHUT).map((view) => view.path));
}
