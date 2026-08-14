// The Changes surface's decisions, as pure functions. Nothing here touches the
// DOM or the wire: the controller (core/gitPane.js) and the markup builders
// (core/changesRender.js, core/diffRender.js) both read their behaviour from
// here, so a rule about what the surface shows lives in exactly one place.
//
// The rules come from the UX Redesign Decisions doc's "Changes" bullet:
// Uncommitted is the top rail entry (carrying +/− counts, not a file count),
// the review aggregate is never the default selection, a clean branch opens at
// the commit list with no commit box, commit is commit-all (no staged set), and
// noise files are COLLAPSED, never filtered away.

/** Generated files a reviewer almost never reads: they still render, grouped
 *  and collapsed at the bottom of the stack. Matched on the basename so a
 *  vendored copy deep in a tree is recognised too. */
const LOCKFILE_NAMES = new Set([
  "uv.lock",
  "poetry.lock",
  "Pipfile.lock",
  "package-lock.json",
  "yarn.lock",
  "pnpm-lock.yaml",
  "bun.lockb",
  "Cargo.lock",
  "Gemfile.lock",
  "composer.lock",
  "mix.lock",
  "go.sum",
]);

/** True for machine-generated paths: Build's own metadata directory, python
 *  bytecode caches, and dependency lockfiles. Everything else is source. */
export function isNoiseFile(path) {
  const text = String(path ?? "");
  if (!text) return false;
  if (text === ".build" || text.startsWith(".build/")) return true;
  if (text.split("/").includes("__pycache__")) return true;
  return LOCKFILE_NAMES.has(text.split("/").pop());
}

/** Split parsed diff files into what a reviewer reads and what is noise. Pure:
 *  the input array is never reordered or mutated, and every file comes out in
 *  exactly one of the two lists — "collapse, never hide". */
export function groupNoiseFiles(files) {
  const primary = [];
  const noise = [];
  for (const file of files || []) (isNoiseFile(file && file.path) ? noise : primary).push(file);
  return { files: primary, noise };
}

/** The count line under a collapsed noise group. */
export function noiseGroupLabel(count) {
  return `${count} generated file${count === 1 ? "" : "s"} — lockfiles, caches, Build metadata`;
}

/** The Uncommitted entry's numbers: +/− counts (what the doc asks the rail to
 *  carry) plus the file count the detail header still names. */
export function uncommittedTotals(status) {
  const stat = (status && status.stat) || {};
  return {
    insertions: Number(stat.insertions) || 0,
    deletions: Number(stat.deletions) || 0,
    files: ((status && status.files) || []).length,
  };
}

export function hasUncommittedChanges(status) {
  return ((status && status.files) || []).length > 0;
}

/** Where the surface opens: on the uncommitted changeset while the tree is
 *  dirty, otherwise at the commit list with nothing selected (a clean branch
 *  gets no commit box). The review aggregate is never the default — it is an
 *  entry under the commit list, reachable but not in the way. */
export function defaultChangesSelection({ status }) {
  return hasUncommittedChanges(status) ? "uncommitted" : null;
}

/** The selection a fresh poll should hold: the user's, always — except that an
 *  empty selection follows the tree into dirt, so the first edit after a commit
 *  lands somewhere visible. */
export function selectionAfterPoll(selected, status) {
  if (selected !== null && selected !== undefined) return selected;
  return hasUncommittedChanges(status) ? "uncommitted" : null;
}

/** Progressive disclosure: the commit message box and its button exist only
 *  while there is something to commit. */
export function commitBoxVisible(status) {
  return hasUncommittedChanges(status);
}

/** Commit is commit-all: every changed path is staged, then committed. There is
 *  no per-file staged set to assemble. */
export function commitAllPaths(status) {
  return ((status && status.files) || []).map((file) => file.path);
}

/** Comments need an agent to reach: only a run-backed scope has a conversation
 *  to post them into. A bare project/worktree checkout renders the same diffs
 *  without the ✎. */
export function commentsSupported(scope) {
  return Boolean(scope && scope.run_id);
}

/** The reviewer is mid-comment — the poll must not rebuild the diff under them
 *  (it would drop anchors, the open popover, and typed text). */
export function commentLayerBusy({ pending = 0, popOpen = false, generalText = "", menuOpen = false } = {}) {
  return Boolean(pending > 0 || popOpen || String(generalText).trim() || menuOpen);
}
