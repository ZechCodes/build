import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { directoryId } from "../core/workspaceModel.js";
import { esc } from "../core/text.js";

function destination(route, workspaces) {
  for (const workspace of workspaces) {
    const directory = (workspace.directories || []).find((entry) => entry.branch === route.branch);
    if (!directory) continue;
    return {
      name: "workspace",
      projectId: route.projectId,
      workspaceId: workspace.id || workspace.workspace_id,
      sourceId: directoryId(directory),
      tab: route.tab || "changes",
      ...(route.file ? { file: route.file, line: route.line } : null),
    };
  }
  return null;
}

export async function renderRetiredBranch() {
  const root = $("#root");
  const route = App.route;
  root.innerHTML = '<div class="empty">loading…</div>';
  try {
    const answer = await App.call("workspace.list", { project_id: route.projectId });
    const next = destination(route, answer.workspaces || []);
    if (next) return go(next);
    root.innerHTML = `<div class="empty"><h2>${esc(route.branch || "Legacy checkout")}</h2>
      <p>This checkout has no workspace. Open the project menu and create one to continue.</p></div>`;
  } catch (error) {
    root.innerHTML = `<div class="empty"><h2>Legacy checkout</h2><p>${esc(error.message || String(error))}</p></div>`;
  }
}
