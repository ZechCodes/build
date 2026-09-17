// What the branch surface stands up from before its first read answers.
//
// The feed already carries the branch's row — ids, scope, agents — and a plain
// folder (a project with no git in it) has no row at all and has to be asked
// for. Both answers are pure enough to keep out of the view: this module reads
// one device's slice of the shared snapshot, asks the machine when the snapshot
// has nothing, and says which tab the surface opens on.

import { branchRowIn, deviceFeedNow } from "../core/feedRows.js";

/** Nothing to stand up from: the Changes tab, and a live read to fill it. */
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

const folderRowIn = (snapshot, projectId, branch) => {
  const project = (snapshot.projects || []).find((candidate) => candidate.id === projectId);
  return project && project.is_git === false ? folderRow(project, projectId, branch) : null;
};

/** The row this surface stands up from, and the tab that row is best seen in. */
function seedBranchState(deviceId, projectId, branch) {
  const snapshot = deviceFeedNow(deviceId);
  if (!snapshot) return NO_SEED;
  const seeded = branchRowIn(snapshot, projectId, branch);
  if (seeded) return { row: seeded, defaultTab: "changes" };
  const folder = folderRowIn(snapshot, projectId, branch);
  return folder ? { row: folder, defaultTab: "files" } : NO_SEED;
}

/** What the machine lists under this project id, or null when it lists nothing
 *  under it and when it could not be asked at all. */
async function listedProject(callRpc, projectId) {
  try {
    const listed = await callRpc("project.list");
    return (listed.projects || []).find((candidate) => (candidate.project_id || candidate.id) === projectId) || null;
  } catch {
    return null;
  }
}

/** The folder behind a project with no git in it. Null for anything the machine
 *  calls a git project. */
async function loadPlainBranch(callRpc, projectId, branch) {
  const project = await listedProject(callRpc, projectId);
  return project && project.is_git === false ? folderRow(project, projectId, branch) : null;
}

/** Whether the machine calls this project a git one, or null when it could not
 *  be asked — a folder that has just been initialized elsewhere says true. */
export async function projectGitState(callRpc, projectId) {
  const project = await listedProject(callRpc, projectId);
  return project ? project.is_git !== false : null;
}

/** The row and the tab the surface opens with: the feed's row where there is
 *  one, the folder behind a plain project where there is not, and the tab the
 *  URL asked for or the one that row is best seen in. */
export async function initialBranchState(callRpc, { deviceId, projectId, branch, requestedTab }) {
  const seeded = seedBranchState(deviceId, projectId, branch);
  const row = seeded.row || (await loadPlainBranch(callRpc, projectId, branch));
  const defaultTab = row?.is_git === false ? "files" : seeded.defaultTab;
  return { row, tab: requestedTab || defaultTab };
}
