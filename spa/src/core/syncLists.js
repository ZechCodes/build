// Ownership lists start together and commit independently. No missing reply
// is interpreted as an authoritative empty collection in the interim.
import { liveFeedSnapshot } from "./feedMerge.js";

export async function readSyncLists(context, { ask, landBoard, landList }) {
  const before = {};
  const answers = {};
  const read = async (method, field, land) => {
    const answer = await ask(context, method, {}, "foreground");
    answers[field] = answer;
    if (answer && context.active()) await land(answer, answers, before);
    return answer;
  };
  const [board, projects, workspaces] = await Promise.all([
    read("board.list", "board", landBoard),
    read("project.list", "projects", (answer, _answers, before) => landList("projects", answer.projects || [], before)),
    read("workspace.list", "workspaces", (answer, _answers, before) => landList("workspaces", answer.workspaces || [], before)),
  ]);
  if (!board || !projects || !context.active()) return null;
  const view = liveFeedSnapshot(board, projects, workspaces || { workspaces: [] }, context.deviceId);
  return { view, before, ownership: { ...view, workspaces: workspaces?.workspaces, projects: projects.projects } };
}
