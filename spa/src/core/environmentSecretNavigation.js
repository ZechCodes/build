// A single navigation intent contains IDs only. It is armed only after leave
// guards accept the navigation and expires at the following navigation.
import { onNavigationCommitted } from "./navigationLifetime.js";
import { conversationRoute } from "./router.js";

let pending = null;
let stop = () => {};

export function navigateToEnvironmentSecret({ deviceId, environmentId, project, navigate }) {
  const route = conversationRoute({ kind: "project", deviceId, projectId: project.project_id,
    agentId: project.conversations?.[0]?.conversation_id });
  return navigate(route, { beforeCommit: () => {
    stop();
    pending = { deviceId, projectId: project.project_id, environmentId };
    let opening = true;
    stop = onNavigationCommitted(() => {
      if (opening) { opening = false; return; }
      pending = null; stop();
    });
  } });
}

export function takeEnvironmentSecretNavigation({ deviceId, projectId, kind }) {
  if (kind !== "project" || pending?.deviceId !== deviceId || pending.projectId !== projectId) return null;
  const intent = pending;
  pending = null; stop(); stop = () => {};
  return intent;
}
