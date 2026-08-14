// Per-session re-review memory: a stable hash of what the reviewer last saw
// when they sent comments, so the next pass can mark which files moved since.
// Pure and dependency-free; state lives in the view.
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
 *  a moved boundary between rows still changes the hash. */
export function hashFileRows(file) {
  const rows = (file && file.rows) || [];
  return hashText(rows.map((r) => r.text).join("\n"));
}

/** Snapshot the current diff: path → row-hash for every file. */
export function stampReview(files) {
  const stamps = new Map();
  for (const file of files || []) stamps.set(file.path, hashFileRows(file));
  return stamps;
}

/** Paths that moved since the stamp: a file with no prior stamp (new since the
 *  review) OR whose row-hash differs. An empty stamp (nothing was ever reviewed)
 *  flags nothing — there is no baseline to compare against. */
export function changedSinceReview(stamps, files) {
  const changed = new Set();
  if (!stamps || stamps.size === 0) return changed;
  for (const file of files || []) {
    const prior = stamps.get(file.path);
    if (prior === undefined || prior !== hashFileRows(file)) changed.add(file.path);
  }
  return changed;
}

/** Stamp one changeset's diff under its rail key, leaving every other
 *  changeset's stamp alone. Pure: the caller gets a new map back. */
export function stampChangeset(stamps, key, files) {
  const next = new Map(stamps || []);
  next.set(String(key), stampReview(files));
  return next;
}

/** Paths that moved since the reviewer last sent comments on THIS changeset.
 *  A changeset never reviewed has no baseline, so nothing is flagged. */
export function changedSinceChangeset(stamps, key, files) {
  return changedSinceReview((stamps && stamps.get(String(key))) || null, files);
}

/** Whether this changeset has a baseline at all — what decides if the surface
 *  offers "only what changed since my review" on it. */
export function changesetStamped(stamps, key) {
  const stamp = stamps && stamps.get(String(key));
  return Boolean(stamp && stamp.size > 0);
}
