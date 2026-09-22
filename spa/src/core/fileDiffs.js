// The per-file diffs behind one git status.
//
// `git.status` carries shape: which files changed, how much, and a content_key
// naming what each one holds right now. The body of a file — its unified diff —
// is asked for on its own through `git.diff`, cached under its path, and
// answered by path. So a poll that finds nothing new costs one small answer,
// an edit refetches one file, and a file the reader never opens is never sent.
//
// The decisions are pure and exported on their own; `createFileDiffs` is the
// one thing that talks to the wire and the cache.

import { createCachedBodies } from "./cachedBodies.js";
import { coordinatedRead, rpcReadKey } from "./readRequests.js";

/** The local-cache kind one file's body is stored under, sub-keyed by path. */
export const FILE_DIFF_RECORD_KIND = "filediff";

/** The most paths one `git.diff` takes — the bridge errors past it. */
export const GIT_DIFF_MAX_PATHS = 50;

const filesOf = (status) => (status && status.files) || [];

/** Every path the shape names, in its own order. */
export const statusPaths = (status) => filesOf(status).map((file) => file.path);

/** Whether `body` is the body of the content `contentKey` names. A body whose
 *  key differs is what the file held before the last edit. */
export function bodyMatches(body, contentKey) {
  return Boolean(body && body.content_key === contentKey);
}

const nothingCached = () => undefined;
const isOpen = (openPaths, path) => Boolean(openPaths && openPaths.has(path));

/** The paths whose bodies are worth asking for now.
 *
 *  An open file with no body, or one whose content moved, is fetched eagerly:
 *  it is on screen. A collapsed file waits for the reader to expand it or for
 *  the warmer to reach it in idle time.
 *
 *  `openPaths` is a Set of paths; `cached` answers the body held for a path. */
export function pathsToFetch(status, options = {}) {
  const bodyOf = options.cached || nothingCached;
  return filesOf(status)
    .filter((file) => isOpen(options.openPaths, file.path) && !bodyMatches(bodyOf(file.path), file.content_key))
    .map((file) => file.path);
}

/** The paths one call at a time, in request order. */
export function batchPaths(paths, max = GIT_DIFF_MAX_PATHS) {
  const batches = [];
  for (let start = 0; start < paths.length; start += max) batches.push(paths.slice(start, start + max));
  return batches;
}

/** A previous fill's outcome, already delivered to whoever asked for it: the
 *  chain reads it only to know the turn is over. */
const alreadyDelivered = () => undefined;

/** Fills run one at a time, so two callers — a poll and a reader opening a
 *  file — never ask `git.diff` for the same paths at once. */
function createFillQueue() {
  let tail = Promise.resolve();
  return (work) => {
    const started = tail.then(work, work);
    tail = started.then(alreadyDelivered, alreadyDelivered);
    return started;
  };
}

/**
 * The file bodies of one checkout.
 *
 * `bodyOf(path)` answers `{ content_key, patch, truncated }` or undefined — a
 * paint asks it and never waits. `sync` keeps the open files' bodies current
 * against a status shape, `warm` fills the rest in the background, and both
 * answer how many bodies were filled so a caller can repaint only on news.
 */
export function createFileDiffs({
  deviceId,
  entityId,
  scope,
  call,
  requestPriority = "foreground",
  requestScope = call,
  onChange = () => {},
}) {
  let disposed = false;
  const enqueue = createFillQueue();
  const repository = scope.run_id
    ? `run:${scope.run_id}`
    : scope.worktree_id
      ? `worktree:${scope.project_id || ""}:${scope.worktree_id}`
      : `project:${scope.project_id || ""}`;
  const bodies = createCachedBodies({
    addressOf: (path) =>
      deviceId && entityId ? { deviceId, entityId, kind: FILE_DIFF_RECORD_KIND, sub: path } : null,
    fetchMissing: async (paths) => {
      const params = { ...scope, paths };
      const key = rpcReadKey({ deviceId, requestScope, repository, call, method: "git.diff", params });
      const answer = await coordinatedRead({
        key,
        priority: requestPriority,
        load: (envelope) => call("git.diff", params, envelope),
      });
      return answer.files || [];
    },
    valueOf: (file) => ({
      key: file.path,
      value: { content_key: file.content_key, patch: file.patch, truncated: Boolean(file.truncated) },
    }),
    onChange,
  });

  const bodyOf = (path) => bodies.read(path);

  async function fetchBatches(paths) {
    let filled = 0;
    for (const batch of batchPaths(paths)) {
      if (disposed) break;
      filled += (await bodies.ensure(batch)).length;
    }
    return filled;
  }

  /** Which of `paths` the shape still has no matching body for. */
  const stillWanted = (status, paths) => {
    const wanted = new Set(pathsToFetch(status, { openPaths: new Set(paths), cached: bodyOf }));
    return paths.filter((path) => wanted.has(path));
  };

  /** One pass over a shape: fetch what the pass is about, then — because the
   *  local cache can answer with the body a file held before its last edit —
   *  fetch again whatever came back stale. A body from the wire matches by
   *  definition, so two goes are always enough. */
  async function fill(status, { openPaths = null, budget = Infinity }) {
    const wanted = pathsToFetch(status, { openPaths, cached: bodyOf }).slice(0, budget);
    if (!wanted.length || disposed) return 0;
    const filled = await fetchBatches(wanted);
    const stale = stillWanted(status, wanted);
    return stale.length && !disposed ? filled + (await fetchBatches(stale)) : filled;
  }

  return {
    bodyOf,
    sync: ({ status, openPaths = null }) => enqueue(() => fill(status, { openPaths })),
    warm: (status, { budget = GIT_DIFF_MAX_PATHS } = {}) =>
      enqueue(() => fill(status, { openPaths: new Set(statusPaths(status)), budget })),
    dispose: () => {
      disposed = true;
      bodies.dispose();
    },
  };
}
