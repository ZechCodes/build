// The hunks behind one changeset's files, fetched a path at a time.
//
// A review surface holds the list of changed paths long before it holds what
// any of them say: a `git` push carries the list with no patch at all, and the
// cold pass asks for the same shape. The body of a file is the largest thing
// that list points at — a checkout with real work in it is most of a megabyte
// — and on a phone's relayed path sending all of them at connect is the
// reader's own connection competing with hunks nobody has opened.
//
// So the bodies are read through `git.changeset_diff`, which answers the same
// changeset narrowed to the paths asked for. This module decides what is
// worth asking for and keeps what came back; the plug that paints the stack
// decides WHEN, from what is on screen.
//
// It is the same four steps `core/fileDiffs.js` takes for an uncommitted
// file's body, against a different verb: decide what is missing, cross one
// async boundary, write through to the local cache, and answer bodies by path.

import { createCachedBodies } from "./cachedBodies.js";
import { BODY_PAGE_BYTES, pageFromAnswer, textPagesOf } from "./bodyPages.js";
import { bodyMatches } from "./fileDiffs.js";
import { withinBytes } from "./cacheLifetime.js";

/** The local-cache kind one changeset file's body is stored under, sub-keyed
 *  by path. */
export const CHANGESET_DIFF_RECORD_KIND = "changesetdiff";

/** The most paths one `git.changeset_diff` takes — the bridge errors past it. */
export const CHANGESET_DIFF_MAX_PATHS = 50;
export const CHANGESET_DIFF_MAX_BYTES = 1_048_576;

/** How many bodies one turn asks for.
 *
 *  The gate that says a file is worth fetching is the viewport's, and a stack
 *  whose bodies have not arrived is a list of headers — forty of them are a
 *  few hundred pixels, all of it inside the observer's margin, so "what the
 *  reader can see" is the whole changeset until something is drawn. Filling a
 *  screenful at a time is what breaks that: every fill makes the stack taller,
 *  and the files below the fold stop being visible before they are ever asked
 *  for. Measured on a relayed path against a 1.2 MB diff over forty files, the
 *  difference between this and no budget is 984 KB and 96 KB. */
export const BODIES_PER_TURN = 8;

const FILE_HEADER = "diff --git ";

/** The path a file patch is of, read off its header the way the bridge wrote
 *  it (`b/<path>`). */
export function pathOfFilePatch(filePatch) {
  const match = String(filePatch).split("\n", 1)[0].match(/ b\/(.+)$/);
  return match ? match[1] : null;
}

/** One answer's patch split into the file patches it holds, by path. A patch
 *  covering several asked paths arrives as one string; the cache is per file,
 *  so it is taken apart here. */
export function filePatchesByPath(patch) {
  const found = new Map();
  let current = [];
  const keep = () => {
    if (!current.length) return;
    const text = current.join("\n");
    const path = pathOfFilePatch(text);
    if (path) found.set(path, text);
  };
  for (const line of String(patch || "").split("\n")) {
    if (line.startsWith(FILE_HEADER)) {
      keep();
      current = [line];
    } else if (current.length) current.push(line);
  }
  keep();
  return found;
}

/** The paths whose bodies are worth asking for now: the ones the caller says
 *  are on screen, that have no body, or whose body is what the file said
 *  before it last moved.
 *
 *  A view that came with its own rows is not one of them — a payload that
 *  carried the whole patch has already answered every question this module
 *  exists to ask. */
export function pathsToFetch(views, { openPaths, bodyOf }) {
  return (views || [])
    .filter((view) => !view.rows && openPaths.has(view.path))
    .filter((view) => !hasAnsweredBody(bodyOf(view.path)) || !bodyMatches(bodyOf(view.path), view.contentKey))
    .map((view) => view.path);
}

/** Older truncated aggregates kept empty pages for paths the patch never
 *  reached. They carry a content key but still owe the reader a body. Empty
 *  complete replies remain valid, including binary and unchanged files, and
 *  a page read can complete a body whose original answer was truncated. */
const hasAnsweredBody = (body) => Boolean(body && (!body.truncated || body.patch || body.pages?.complete));

/** The paths one call at a time, in request order. */
export function batchPaths(paths, max = CHANGESET_DIFF_MAX_PATHS) {
  const batches = [];
  for (let start = 0; start < paths.length; start += max) batches.push(paths.slice(start, start + max));
  return batches;
}

/** The carried body the cut fell in. Earlier segments arrived whole;
 *  paths the patch never reached have no body to keep yet. */
function cutPaths(answer, segments) {
  if (!answer.truncated) return new Set();
  const carried = [...segments.keys()];
  const last = carried[carried.length - 1];
  return new Set(last ? [last] : []);
}

/** One answer's bodies, one per asked path, in request order: a path with
 *  nothing to say in a complete answer gets an empty patch under the list's
 *  key, the way `git.diff` answers one. A truncated answer's metadata can
 *  name paths its patch never reached; those still need a wire read. */
function answeredBodies(answer, paths, keyFor) {
  const segments = filePatchesByPath(answer.patch);
  const keys = new Map((answer.files || []).map((file) => [file.path, file.content_key]));
  const cut = cutPaths(answer, segments);
  return paths.filter((path) => !answer.truncated || segments.has(path)).map((path) => ({
    path,
    content_key: keys.get(path) ?? keyFor(path),
    patch: segments.get(path) || "",
    truncated: cut.has(path),
  }));
}

/** A previous fill's outcome, already delivered to whoever asked for it. */
const alreadyDelivered = () => undefined;

/** Fills run one at a time, so a repaint and a reader opening a file never
 *  ask for the same paths at once. */
function createFillQueue() {
  let tail = Promise.resolve();
  return (work) => {
    const started = tail.then(work, work);
    tail = started.then(alreadyDelivered, alreadyDelivered);
    return started;
  };
}

/**
 * One changeset's file bodies.
 *
 * - `addressOf(path)` is the local-cache address for a path, or null where the
 *   surface has no entity to cache under.
 * - `fetchFiles(paths, { range })` is the one wire call: it answers `{ files,
 *   patch }`, the verb's own shape. `range` is only ever sent with one path,
 *   and only where `canPage()` says the bridge announced `bodies.pages`; the
 *   answer then carries the page in `patch` and where it sits in `range`.
 * - `keyFor(path)` is the content key the file list currently holds for a
 *   path, which is what a body with no hunks of its own is filed under — a
 *   binary file and a path the changeset no longer touches both answer an
 *   empty patch, and neither should be asked for again on every paint.
 *
 * `bodyOf(path)` answers `{ content_key, patch }` or undefined and never
 * waits, because a paint asks it. A body over the cap, or one the bridge cut,
 * is kept in pages (#95, core/bodyPages.js) and also answers `pages: { end,
 * total, complete }`; `more(path)` reads the page after the last one held.
 */
export function createChangesetBodies({ addressOf, fetchFiles, keyFor, canPage = () => false, onChange = () => {} }) {
  let disposed = false;
  const enqueue = createFillQueue();
  // What has been asked for and answered, by the key the file wore when it was
  // asked. A file whose body came back empty — a binary, a path the changeset
  // no longer touches — would otherwise be wanted by every paint for ever.
  const answered = new Map();
  // An answer already in hand that covers the paths being filled — the whole
  // changeset's patch, read for the aggregate — which a fill takes its bodies
  // from instead of the wire.
  let inHand = null;
  // Coverage of the current fill's replies, separate from bodies already
  // held or hydrated. A reply can name a newer key than the requesting list.
  let fillReplies = null;
  /** The page of `path`'s body the bridge cuts from `offset`, named by the
   *  version the bridge gives the whole patch; null where it cannot cut one —
   *  asked as the page is wanted, so a bridge greeted after this surface
   *  mounted is read on from — or where its answer is no page. */
  const readPage = async (path, offset) => {
    if (!canPage()) return null;
    const answer = (await fetchFiles([path], { range: { offset, bytes: BODY_PAGE_BYTES } })) || {};
    return answer.range ? pageFromAnswer(answer, "patch") : null;
  };

  const fetchMissing = async (paths) => {
    const replies = fillReplies;
    const files = answeredBodies(inHand || (await fetchFiles(paths)) || {}, paths, keyFor);
    for (const file of files) replies?.set(file.path, file.content_key);
    return files;
  };

  const bodies = createCachedBodies({
    addressOf,
    fetchMissing,
    valueOf: (file) => ({
      key: file.path,
      value: { content_key: file.content_key, patch: file.patch, ...(file.truncated ? { truncated: true } : {}) },
    }),
    cacheable: (body) => !body.truncated && withinBytes(body.patch, CHANGESET_DIFF_MAX_BYTES),
    // A body over the cap, or cut, is kept in pages named by the version the
    // bridge gives the whole patch — a content key does not name a patch, which
    // moves with HEAD and the base while the file stands still — or, from a
    // bridge that cannot page, split here out of the answer.
    pages: {
      field: "patch",
      split: (body, of) => textPagesOf(body.patch, { of, cut: body.truncated }),
      readPage,
    },
    onChange,
  });

  const bodyOf = (path) => bodies.read(path);
  const wasAnswered = (path, key) => Boolean(hasAnsweredBody(bodyOf(path)) && answered.has(path) && answered.get(path) === key);

  const settleReplies = (landed, keyOf, replies) => {
    for (const path of landed) {
      const body = bodyOf(path);
      const key = keyOf.get(path);
      // Stale hydration cannot settle the request. A mismatching key settles
      // only when this fill carried a reply under the key now held.
      const replied = replies.has(path) && bodyMatches(body, replies.get(path));
      if (hasAnsweredBody(body) && (bodyMatches(body, key) || replied)) answered.set(path, key);
    }
  };

  const fillBatch = async (batch, keyOf) => {
    const replies = new Map();
    fillReplies = replies;
    try {
      const landed = await bodies.ensure(batch);
      settleReplies(landed, keyOf, replies);
      return landed.length;
    } finally {
      fillReplies = null;
    }
  };

  async function fill(views, openPaths, budget) {
    const keyOf = new Map((views || []).map((view) => [view.path, view.contentKey]));
    const wanted = pathsToFetch(views, { openPaths, bodyOf })
      .filter((path) => !wasAnswered(path, keyOf.get(path)))
      .slice(0, budget);
    if (!wanted.length || disposed) return 0;
    let filled = 0;
    for (const batch of batchPaths(wanted)) {
      if (disposed) break;
      filled += await fillBatch(batch, keyOf);
    }
    return filled;
  }

  return {
    bodyOf,
    canPage,
    /** Read the next page of `path`'s paged body into the cache. Answers
     *  whether one landed; the repaint comes through `onChange`. */
    more: (path) => bodies.more(path),
    /** Hold bodies for the views whose paths are in `openPaths`, and answer
     *  how many landed, so a caller repaints only on news. A turn asks for at
     *  most [`BODIES_PER_TURN`] of them: the paint that follows is what tells
     *  the next turn which files are still on screen. */
    sync: (views, openPaths, { budget = BODIES_PER_TURN } = {}) => enqueue(() => fill(views, openPaths, budget)),
    /** Keep the bodies of `paths` out of `answer`, a `{ files, patch,
     *  truncated }` already read for the whole changeset, as if the wire had
     *  answered them — so an aggregate too big for its own record still costs
     *  one read. A path already holding a body keeps it. */
    seed: (answer, paths) =>
      enqueue(async () => {
        inHand = answer;
        try {
          return (await bodies.ensure(paths)).length;
        } finally {
          inHand = null;
        }
      }),
    dispose: () => {
      disposed = true;
      bodies.dispose();
    },
  };
}
