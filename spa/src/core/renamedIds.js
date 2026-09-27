// Ids minted before issues were renamed tasks (#190).
//
// The bridge rewrote every stored record to the new prefixes when it upgraded,
// but an old id lives on where no migration reaches: a link somebody saved, a
// message body that quotes one, a read mark this browser cached. Each still
// names the same record, under its new prefix.

const RENAMED_PREFIXES = Object.freeze([
  ["issue-", "task-"],
  ["ic-", "tc-"],
  ["ie-", "te-"],
]);

/** The id as the bridge knows it now: `issue-…` is `task-…`, a comment's
 *  `ic-…` is `tc-…` and an event's `ie-…` is `te-…`. Anything else, and
 *  anything that is not a string, comes back untouched. */
export function currentId(id) {
  if (typeof id !== "string") return id;
  const renamed = RENAMED_PREFIXES.find(([old]) => id.startsWith(old));
  return renamed ? renamed[1] + id.slice(renamed[0].length) : id;
}
