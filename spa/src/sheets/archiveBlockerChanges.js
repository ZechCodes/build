import { readCached } from "../core/localCache.js";

const projectKinds = new Set(["project-settings", "tracker-tasks", "tracker-tasks-query", "tracker-tasks-page", "tracker-task", "tracker-summary", "task-review"]);
const rowId = row => row?.project_id || row?.id;

// The sheet's existing device cache subscription owns this observer. Project
// task/review records are already scoped; device lists compare only our rows.
export function archiveBlockerChanges(deviceId, projectId, project, clear) {
  const signatures = new Map();
  let reads = Promise.resolve();
  const listed = kind => {
    reads = reads.then(async () => {
      const rows = (await readCached({ deviceId, entityId: "", kind }))?.value || [];
      const ours = rows.filter(row => kind === "projects" ? rowId(row) === projectId : row.project_id === projectId);
      const signature = JSON.stringify(ours);
      if (signatures.has(kind) && signatures.get(kind) !== signature) clear();
      signatures.set(kind, signature);
    });
  };
  listed("projects"); listed("workspaces");
  return address => {
    if (!address.entityId && ["projects", "workspaces"].includes(address.kind)) listed(address.kind);
    if (address.entityId === projectId && projectKinds.has(address.kind)) clear();
    if (address.kind === "project-agents" && address.entityId === (project()?.entity_id || project()?.run_id)) clear();
  };
}
