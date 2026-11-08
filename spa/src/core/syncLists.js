// Ownership lists start together and commit independently. No missing reply
// is interpreted as an authoritative empty collection in the interim.
import { liveFeedSnapshot } from "./feedMerge.js";
import { bridgeCapabilities } from "./changeEvents.js";
import { admitEnvironmentListWrite } from "./environmentCache.js";

/** Environments have their own list even when device journals own other lists. */
export async function readSyncEnvironments(context, { ask, landList, priority = "foreground" }, before = {}) {
  const reading = { ...context, active: () => context.active() && bridgeCapabilities(context.deviceId).environments?.list === true };
  if (!reading.active()) return;
  const admission = await admitEnvironmentListWrite(context.deviceId, reading.active);
  if (!admission || !reading.active()) return;
  const answer = await ask(reading, "environment.list", {}, priority);
  if (Array.isArray(answer?.environments) && reading.active()) await landList("environments", answer.environments, before, admission);
}

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
    readSyncEnvironments(context, { ask, landList }, before),
  ]);
  if (!board || !projects || !context.active()) return null;
  const view = liveFeedSnapshot(board, projects, workspaces || { workspaces: [] }, context.deviceId);
  return { view, before, ownership: { ...view, workspaces: workspaces?.workspaces, projects: projects.projects } };
}
