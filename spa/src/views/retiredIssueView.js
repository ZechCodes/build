// A legacy issue URL, redirected to the workspace its implementation lives in.
// Unwired: `issue` still renders its own surface (app.js VIEWS), and wiring this
// is a product switch nobody has thrown.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { workspaceRoute } from "../core/projectModel.js";
import { canAnswer, routeContext } from "../core/deviceContexts.js";
import { mountDeviceNotice } from "../core/deviceNotice.js";
import { esc } from "../core/text.js";

const implementationId = (issue) => issue?.workspace_id || issue?.current_implementation_id || issue?.implementation?.workspace_id || null;

export async function renderRetiredIssue() {
  const root = $("#root");
  const route = App.route;
  const context = routeContext(route);
  if (!canAnswer(context)) {
    mountDeviceNotice(root, route.deviceId);
    return;
  }
  const callRpc = context.rpc;
  root.innerHTML = '<div class="empty">loading…</div>';
  try {
    const answer = await callRpc("issue.get", { issue_id: route.id });
    const issue = answer.issue || answer;
    const workspaceId = implementationId(issue);
    if (workspaceId) {
      const workspaceAnswer = await callRpc("workspace.get", { workspace_id: workspaceId });
      const workspace = workspaceAnswer.workspace || workspaceAnswer;
      // The machine that answered is the machine it is on: a workspace read
      // off one bridge carries no device of its own.
      const destination = workspaceRoute({ ...workspace, deviceId: context.deviceId });
      if (destination) return go(destination);
    }
    root.innerHTML = `<div class="empty"><h2>${esc(issue.title || "Archived issue")}</h2>
      <p>Issues are read-only in this version. Create or open a workspace from the project menu to continue the work.</p></div>`;
  } catch (error) {
    root.innerHTML = `<div class="empty"><h2>Archived issue</h2><p>${esc(error.message || String(error))}</p></div>`;
  }
}
