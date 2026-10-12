// Ancestry comes only from the URL. Cache metadata supplies labels, never a
// different host because a task's assignee or workspace ownership changed.
import { hashFromRoute } from "./router.js";

const link = (label, route, current = false, hash = null) => ({
  label, route, current, hash: hash ?? hashFromRoute(route),
});

function tasksParent(route) {
  if (route.name === "workspace") return { ...route, taskId: undefined };
  return { name: "project", deviceId: route.deviceId, projectId: route.projectId, tab: "tasks" };
}

function currentLeaf(route, label, task, hash) {
  if (route.taskId) return link(task?.number ? `#${task.number}` : route.taskId, route, true, hash);
  return ["branch", "task"].includes(route.name) ? link(label, route, true, hash) : null;
}

export function toolbarBreadcrumbs({ route = {}, currentHash = null, project = "", label = "", task = null } = {}) {
  const projectRoute = route.name === "project" ? route
    : { name: "project", deviceId: route.deviceId, projectId: route.projectId };
  const currentProject = route.name === "project";
  const taskPage = route.name === "trackerTask" || (route.name === "workspace" && route.tab === "tasks" && !!route.taskId);
  return {
    project: link(project, projectRoute, currentProject, currentProject ? currentHash : null),
    workspace: route.name === "workspace" ? { label, current: !taskPage } : null,
    tasks: taskPage ? link("Tasks", tasksParent(route)) : null,
    leaf: currentLeaf(route, label, task, currentHash),
  };
}
