// The Changes surface's decisions, as pure functions. Nothing here touches the
// DOM or the wire: the controller (core/gitPane.js) and the markup builders
// (core/changesRender.js, core/diffRender.js) both read their behaviour from
// here, so a rule about what the surface shows lives in exactly one place.
//
// The rules come from the UX Redesign Decisions doc's "Changes" bullet:
// the review aggregate ("All changes") is the rail's top entry and where the
// surface opens, Uncommitted sits under it carrying +/− counts (not a file
// count), a branch with no aggregate and a clean tree opens at the commit list
// with no commit box, commit is commit-all (no staged set), and noise files are
// COLLAPSED, never filtered away.

import { pickAChangesetText } from "./text.js";

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

/** Where the surface opens: on the review aggregate wherever the surface has
 *  one — everything this branch carries against its base is what a reviewer
 *  came to read, committed or not. Without an aggregate (the primary checkout,
 *  a bare project) it opens on the uncommitted changeset while the tree is
 *  dirty, otherwise at the commit list with nothing selected (a clean branch
 *  gets no commit box). */
export function defaultChangesSelection({ status, review = null }) {
  if (review) return "review";
  return hasUncommittedChanges(status) ? "uncommitted" : null;
}

/** Plain words for the state of the working tree, as the rail's Uncommitted row
 *  wears them: clean, or what it is carrying. */
const uncommittedWords = (status) => {
  if (!hasUncommittedChanges(status)) return "clean";
  const { insertions, deletions } = uncommittedTotals(status);
  return `+${insertions} −${deletions}`;
};

/** What the aggregate is measured against — its own subtitle, or the base. */
const reviewWords = (review) => (review && review.subtitle) || `vs ${(review && review.base) || "main"}`;

/** One line naming the changeset the detail column is showing, in the same
 *  words its rail row wears. The pane's narrow-viewport trigger reads it: there
 *  the rail is a drawer, so the row that says where you are standing is behind
 *  the control you would press to leave. Plain text, never markup — a trigger is
 *  a line of copy, not a row. */
export function changesSelectionSummary({ status, log, selected, review = null }) {
  if (selected === "review") return `All changes · ${reviewWords(review)}`;
  if (selected === "uncommitted") return `Uncommitted · ${uncommittedWords(status)}`;
  const commit = ((log && log.commits) || []).find((entry) => entry.hash === selected);
  return commit ? `${commit.subject} · ${commit.short}` : pickAChangesetText;
}

/** The selection a fresh poll should hold: the user's, always — except that an
 *  empty selection falls back to where the surface opens, so the first edit
 *  after a commit lands somewhere visible. */
export function selectionAfterPoll(selected, status, { review = null } = {}) {
  if (selected !== null && selected !== undefined) return selected;
  return defaultChangesSelection({ status, review });
}

/** Progressive disclosure: the commit message box and its button exist only
 *  while there is something to commit. */
export function commitBoxVisible(status) {
  return hasUncommittedChanges(status);
}

/** Every path the worktree has changed, in the order the bridge lists them.
 *  What a commit takes unless the reviewer has ticked a narrower set
 *  (core/reviewMarks.js `commitPaths`). */
export function commitAllPaths(status) {
  return ((status && status.files) || []).map((file) => file.path);
}

/** Runs and workspace sources can reach a conversation. Bare project
 * checkouts render the same diffs without comment controls. */
export function commentsSupported(scope) {
  return Boolean(scope && (scope.run_id || (scope.workspace_id && scope.source_id)));
}

/** The reviewer is mid-comment — the poll must not rebuild the diff under them
 *  (it would drop anchors, the open popover, and typed text).
 *
 *  `selecting` is the earliest of these states and the only one the reviewer has
 *  not finished stating: the drag is still under way, and the popover that would
 *  make it a comment opens only once it settles. A rebuild in that window
 *  cancels the selection with nothing to show for it. */
// eslint-disable-next-line complexity -- ratchet: commentLayerBusy is at 11, cap 10 — reduce it, then drop this line
export function commentLayerBusy({ pending = 0, popOpen = false, generalText = "", menuOpen = false, selecting = false } = {}) {
  return Boolean(pending > 0 || popOpen || String(generalText).trim() || menuOpen || selecting);
}
