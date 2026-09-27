// A task's durable agent identities are supplied by the bridge and cached
// with the task. Only identities marked available can open a conversation.
import { esc } from "./text.js";
import { hashFromRoute } from "./router.js";
import { actorName } from "./trackerLineWords.js";
import { harnessIconHtml } from "./harnessIcon.js";
import { isProjectActor, taskAvatarHtml } from "./taskAvatar.js";

const agentIdOf = (actor) => typeof actor === "string" ? actor : actor?.agent_id || "";
const projectActor = (actor, id) => isProjectActor(typeof actor === "string" ? { agent_id: id } : actor);

const projectHref = (id, context) => hashFromRoute({
  name: "project", deviceId: context.deviceId, projectId: context.projectId, agent: id || undefined,
});

const workspaceInCache = (context, workspaceId) =>
  !Array.isArray(context.workspaces) || context.workspaces.some((workspace) =>
    (workspace.workspace_id || workspace.id) === workspaceId);

const workspaceHref = (id, context) => {
  const identity = context.identities?.[id];
  const group = (context.agentGroups || []).find((one) => one.agents?.some((agent) => agent.id === id));
  const workspaceId = identity?.workspace_id || group?.workspaceId;
  if (identity && !identity.available) return "";
  if (!workspaceId) return "";
  if (!workspaceInCache(context, workspaceId)) return "";
  return hashFromRoute({ name: "workspace", deviceId: context.deviceId, projectId: context.projectId,
    workspaceId, tab: "changes", agent: id });
};

export function actorHref(actor, context = {}) {
  if (!context.deviceId || !context.projectId || !actor) return "";
  const id = agentIdOf(actor);
  if (context.identities?.[id]?.available === false) return "";
  return projectActor(actor, id) ? projectHref(id, context) : workspaceHref(id, context);
}

const actorMarkHtml = (actor, context, id) => {
  if (projectActor(actor, id)) return taskAvatarHtml({ kind: "project_agent" }, context);
  const provider = context.identities?.[id]?.provider || context.agentProviders?.[id];
  return provider ? harnessIconHtml(provider) : "";
};

/** A named actor, linked only while its destination still exists. */
export function actorIdentityHtml(actor, context = {}, { icon = false } = {}) {
  const name = esc(actorName(actor, context));
  const id = agentIdOf(actor);
  const artwork = icon ? actorMarkHtml(actor, context, id) : "";
  const mark = artwork ? `<span class="task-actor-icon" aria-hidden="true">${artwork}</span>` : "";
  const label = `${mark}${name}`;
  const href = actorHref(actor, context);
  return href ? `<a class="task-actor-link" href="${esc(href)}">${label}</a>` : label;
}
