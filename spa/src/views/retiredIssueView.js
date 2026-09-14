import { SMALLEST_THREAD_PAGE } from "../core/thread.js";
import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { workspaceRoute } from "../core/projectModel.js";
import { esc } from "../core/text.js";

const implementationId = (issue) => issue?.workspace_id || issue?.current_implementation_id || issue?.implementation?.workspace_id || null;

export async function renderRetiredIssue() {
  const root = $("#root");
  const route = App.route;
  const callRpc = App.call;
  root.innerHTML = '<div class="empty">loading…</div>';
  try {
    const answer = await callRpc("issue.get", { issue_id: route.id, ...SMALLEST_THREAD_PAGE });
    const issue = answer.issue || answer;
    const workspaceId = implementationId(issue);
    if (workspaceId) {
      const workspaceAnswer = await callRpc("workspace.get", { workspace_id: workspaceId, ...SMALLEST_THREAD_PAGE });
      const destination = workspaceRoute(workspaceAnswer.workspace || workspaceAnswer);
      if (destination) return go(destination);
    }
    root.innerHTML = `<div class="empty"><h2>${esc(issue.title || "Archived issue")}</h2>
      <p>Issues are read-only in this version. Create or open a workspace from the project menu to continue the work.</p></div>`;
  } catch (error) {
    root.innerHTML = `<div class="empty"><h2>Archived issue</h2><p>${esc(error.message || String(error))}</p></div>`;
  }
}
