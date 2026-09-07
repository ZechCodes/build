// Per-session re-review memory: what the reviewer last saw when they sent
// comments, so the next pass can mark which files moved since. Pure and
// dependency-free; state lives in the view.
//
// A file is remembered by its content key — the bridge's for an uncommitted
// file, a hash of the rows for a parsed one (core/fileEntries.js makes both).
// The rows themselves cannot be the comparison any more: a collapsed file has
// no rows until its body is fetched, so hashing them would call every file
// changed the moment a stack painted from shape alone.
//
// The Changes surface shows several stacks — uncommitted, one commit, the
// review aggregate — and a reviewer works through them one at a time, so the
// memory is keyed by changeset: sending comments on the uncommitted stack says
// nothing about what a commit's stack looked like when it was last read.

/** djb2 hash of a string → hex. Stable, order-sensitive, no dependencies. */
export function hashText(text) {
  const source = text || "";
  let hash = 5381;
  for (let i = 0; i < source.length; i++) {
    hash = ((hash << 5) + hash + source.charCodeAt(i)) >>> 0;
  }
  return hash.toString(16);
}

/** Hash of a parsed diff file's row texts (core/diff.js row objects), joined so
 *  a moved boundary between rows still changes the hash. The content key of a
 *  file that arrived as a whole patch. */
export function hashFileRows(file) {
  const rows = (file && file.rows) || [];
  return hashText(rows.map((r) => r.text).join("\n"));
}

/** Snapshot the current diff: path → content key, for every file view. */
export function stampReview(views) {
  const stamps = new Map();
  for (const view of views || []) stamps.set(view.path, view.contentKey);
  return stamps;
}

/** Paths that moved since the stamp: a file with no prior stamp (new since the
 *  review) OR whose content key differs. An empty stamp (nothing was ever
 *  reviewed) flags nothing — there is no baseline to compare against. */
export function changedSinceReview(stamps, views) {
  const changed = new Set();
  if (!stamps || stamps.size === 0) return changed;
  for (const view of views || []) {
    const prior = stamps.get(view.path);
    if (prior === undefined || prior !== view.contentKey) changed.add(view.path);
  }
  return changed;
}

/** Stamp one changeset's file views under its rail key, leaving every other
 *  changeset's stamp alone. Pure: the caller gets a new map back. */
export function stampChangeset(stamps, key, views) {
  const next = new Map(stamps || []);
  next.set(String(key), stampReview(views));
  return next;
}

/** Paths that moved since the reviewer last sent comments on THIS changeset.
 *  A changeset never reviewed has no baseline, so nothing is flagged. */
export function changedSinceChangeset(stamps, key, views) {
  return changedSinceReview((stamps && stamps.get(String(key))) || null, views);
}

/** Whether this changeset has a baseline at all — what decides if the surface
 *  offers "only what changed since my review" on it. */
export function changesetStamped(stamps, key) {
  const stamp = stamps && stamps.get(String(key));
  return Boolean(stamp && stamp.size > 0);
}
