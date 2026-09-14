// What the branch surface stands up from before its first read answers.
//
// The feed already carries the branch's row — ids, scope, agents — and a plain
// folder (a project with no git in it) has no row at all and has to be asked
// for. Both answers are pure enough to keep out of the view: this module reads
// one device's slice of the shared snapshot, asks the machine when the snapshot
// has nothing, and says which tab the surface opens on.

import { subscribeFeed } from "../core/taskFeed.js";
import { deviceFeedView } from "../core/deviceContexts.js";

/** The row this surface stands up from, off the shared snapshot without
 *  subscribing. A route names one machine's project — every machine mints a
 *  `proj-1`, and the merge holds all of them — so the row is looked for in
 *  that machine's view of the feed. */
function seedBranchState(deviceId, projectId, branch) {
  let snapshot = null;
  const unsubscribe = subscribeFeed((feed) => {
    snapshot = deviceFeedView(feed, deviceId);
  });
  unsubscribe();
  if (!snapshot) return { row: null, defaultTab: "changes" };
  const seeded = (snapshot.items || []).find(
    (item) => item.kind === "branch" && item.project_id === projectId && item.branch === branch,
  );
  if (seeded) return { row: seeded, defaultTab: "changes" };
  const project = (snapshot.projects || []).find((candidate) => candidate.id === projectId);
  if (!project || project.is_git !== false) return { row: null, defaultTab: "changes" };
  return {
    row: { kind: "branch", project_id: projectId, project: project.name, branch, primary: true, is_git: false },
    defaultTab: "files",
  };
}

/** The one row a project with no git in it has: its folder, browsable in the
 *  Files tab. Null for anything the machine calls a git project. */
async function loadPlainBranch(callRpc, projectId, branch) {
  try {
    const listed = await callRpc("project.list");
    const project = (listed.projects || []).find((candidate) => (candidate.project_id || candidate.id) === projectId);
    if (!project || project.is_git !== false) return null;
    return { kind: "branch", project_id: projectId, project: project.name, branch, primary: true, is_git: false };
  } catch {
    return null;
  }
}

/** Whether the machine calls this project a git one, or null when it could not
 *  be asked — a folder that has just been initialized elsewhere says true. */
export async function projectGitState(callRpc, projectId) {
  try {
    const listed = await callRpc("project.list");
    const project = (listed.projects || []).find((candidate) => (candidate.project_id || candidate.id) === projectId);
    return project ? project.is_git !== false : null;
  } catch {
    return null;
  }
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
