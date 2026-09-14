// What the branch surface stands up from before its first read answers.
//
// The feed already carries the branch's row — ids, scope, agents — and a plain
// folder (a project with no git in it) has no row at all and has to be asked
// for. Both answers are pure enough to keep out of the view: this module reads
// one device's slice of the shared snapshot, asks the machine when the snapshot
// has nothing, and says which tab the surface opens on.

import { subscribeFeed } from "../core/taskFeed.js";
import { deviceFeedView } from "../core/deviceContexts.js";

/** Nothing to stand up from: the Changes tab, and a live read to fill it. */
const NO_SEED = { row: null, defaultTab: "changes" };

/** The one row a project with no git in it has: its folder, browsable in the
 *  Files tab. */
const folderRow = (project, projectId, branch) => ({
  kind: "branch",
  project_id: projectId,
  project: project.name,
  branch,
  primary: true,
  is_git: false,
});

/** One machine's view of the shared snapshot, read off it without subscribing.
 *  A route names one machine's project — every machine mints a `proj-1`, and
 *  the merge holds all of them — so the rows are that machine's. */
function deviceSnapshot(deviceId) {
  let snapshot = null;
  const unsubscribe = subscribeFeed((feed) => {
    snapshot = deviceFeedView(feed, deviceId);
  });
  unsubscribe();
  return snapshot;
}

const branchRowIn = (snapshot, projectId, branch) =>
  (snapshot.items || []).find(
    (item) => item.kind === "branch" && item.project_id === projectId && item.branch === branch,
  ) || null;

const folderRowIn = (snapshot, projectId, branch) => {
  const project = (snapshot.projects || []).find((candidate) => candidate.id === projectId);
  return project && project.is_git === false ? folderRow(project, projectId, branch) : null;
};

/** The row this surface stands up from, and the tab that row is best seen in. */
function seedBranchState(deviceId, projectId, branch) {
  const snapshot = deviceSnapshot(deviceId);
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
