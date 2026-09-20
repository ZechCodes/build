// The project's agent rail, as every page standing IN a project mounts it: the
// project page, and one issue of its tracker. The rail is part of the shell a
// reader stands in, not of the page — the project's agent is reachable from
// everywhere in the project, so the bubble that opens it must not come and go
// with the page under it.
//
// `project.ensure_conversation` is the project's half of what
// `workspace.ensure_conversation` is for a workspace: it answers the owner the
// project already has, or mints one over a scratch directory Build owns. It is
// the one call made here.
//
// Nothing here names a harness, model or effort. What a project agent starts
// on is the DEVICE's setting, held by the bridge beside its default harness,
// and the mint reads it there — so a new browser is never asked for something
// the machine that runs the agent already holds.
//
// The route may name an agent (`?agent=…`, core/router.js `conversationRoute`):
// a link to the conversation a message came from lands here, and the rail
// comes up standing on it.

import { $ } from "../dom.js";
import { mountAgentRail } from "./agentRail.js";
import { notifyError } from "./notify.js";

/**
 * Mount the project's rail beside the page. Resolves to the rail, or to null
 * when the machine would not answer (the reader has been told why) or the page
 * went away while the owner was being asked for (`disposed()`).
 */
export async function mountProjectAgentRail({ context, route, selection, projectName = "", disposed = () => false }) {
  try {
    const answer = await context.rpc("project.ensure_conversation", { project_id: route.projectId });
    if (disposed()) return null;
    return mountAgentRail($("#agent-rail"), railContext({ context, route, selection, projectName, answer }));
  } catch (error) {
    if (!disposed()) notifyError("No conversation for this project", error.message || String(error));
    return null;
  }
}

/** What the rail is mounted on: the project's conversation, on the owner the
 *  bridge answered with, through this machine's caller, cache and repository. */
const railContext = ({ context, route, selection, projectName, answer }) => ({
  kind: "project",
  projectId: route.projectId,
  projectName,
  entityId: answer?.entity_id || answer?.run_id || null,
  deviceId: context.deviceId,
  callRpc: context.rpc,
  cacheScope: context.cacheScope,
  chatRepository: context.chatRepository,
  selection,
  openAgentId: route.agent || null,
});
