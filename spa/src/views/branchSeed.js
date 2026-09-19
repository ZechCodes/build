// What the branch surface stands on: one machine's slice of the cache.
//
// A branch is three things depending on what is under it — a run Build cut, a
// checkout nobody has claimed, or a plain folder with no git in it — and the
// cache holds all three in collections of its own. This module is the one place
// the three are read and folded into the row the surface is written against.
//
// Nothing here asks a bridge. The feed is a view over the cache
// (core/taskFeed.js), so a `state` push that moves this row moves what these
// answer, and the surface hears it on the same delivery the rail does.

import { branchRowIn, deviceFeedNow, runBodyIn, worktreeRowIn } from "../core/feedRows.js";

/** Nothing this machine holds answers to the branch. */
const NO_SEED = { row: null, defaultTab: "changes" };

/** The one row a project with no git in it has: its folder, browsable in the
 *  Files tab. It names no run and no worktree, which is what says the row IS
 *  the project's own directory — the project alone names it, and that is the
 *  scope its Files tab and its terminals read under (core/consoleModel.js,
 *  views/branchView.js branchScope). */
const folderRow = (project, projectId, branch) => ({
  kind: "branch",
  project_id: projectId,
  project: project.name,
  branch,
  is_git: false,
});

const projectIn = (snapshot, projectId) =>
  (snapshot?.projects || []).find((candidate) => candidate.id === projectId) || null;

const folderRowIn = (snapshot, projectId, branch) => {
  const project = projectIn(snapshot, projectId);
  return project && project.is_git === false ? folderRow(project, projectId, branch) : null;
};

/** The feed row with the run's own body on it, the way `branch.get` used to
 *  answer. The board carries every live run whole beside its rows — the goal
 *  and state the commit box reads, the base the diff is measured against —
 *  and a row that names no run carries null, exactly as a bare checkout
 *  always did. */
const withRunBody = (snapshot, row) => ({ ...row, run: runBodyIn(snapshot, row.run_id) });

/**
 * The row this surface stands on and the tab that row is best seen in, out of
 * one machine's slice.
 *
 * In the order a branch can be backed: the board's own row for work Build is
 * carrying, then a checkout on that branch the board does not list (a worktree
 * nobody has adopted — the inbox leaves those out, and a link to one still has
 * to open), then the project's folder when the project is not a repository.
 */
export function branchStateIn(snapshot, projectId, branch) {
  if (!snapshot) return NO_SEED;
  const listed = branchRowIn(snapshot, projectId, branch);
  if (listed) return { row: withRunBody(snapshot, listed), defaultTab: "changes" };
  const checkout = worktreeRowIn(snapshot, projectId, branch);
  if (checkout) return { row: checkout, defaultTab: "changes" };
  const folder = folderRowIn(snapshot, projectId, branch);
  return folder ? { row: folder, defaultTab: "files" } : NO_SEED;
}

/** Whether this machine's cache calls the project a git one, or null when it
 *  holds nothing about the project at all — a folder initialized a moment ago
 *  says true as soon as the pass that heard about it lands. */
export function projectGitState(deviceId, projectId) {
  const project = projectIn(deviceFeedNow(deviceId), projectId);
  return project ? project.is_git !== false : null;
}

/** The row and the tab the surface opens with: the URL's tab where it asked
 *  for one, and otherwise the tab that row is best seen in. */
export function initialBranchState({ deviceId, projectId, branch, requestedTab }) {
  const seeded = branchStateIn(deviceFeedNow(deviceId), projectId, branch);
  const defaultTab = seeded.row?.is_git === false ? "files" : seeded.defaultTab;
  return { row: seeded.row, tab: requestedTab || defaultTab };
}
