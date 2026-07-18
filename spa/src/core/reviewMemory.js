// Per-session re-review memory: a stable hash of what the reviewer last saw at
// their Request Changes / Send Notes, so the next pass can mark which files (or
// the doc) moved since. Pure and dependency-free; state lives in the view.

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
