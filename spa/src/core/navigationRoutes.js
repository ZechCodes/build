// Destination identity follows the URL host. Selection and ancestry have
// separate contracts: a Current row is a no-op; a project return restores its
// saved face. Neither needs a wire read or a new route kind.
import { routeProjectKey, routeWorkspaceKey } from "./deviceKey.js";
import { projectRoute } from "./projectModel.js";
import { projectReturnRoute } from "./projectRailState.js";
import { hashFromRoute } from "./router.js";

const PROJECT_HOSTS = new Set(["project", "trackerTask"]);

/** Branch, legacy plan and account routes are not visit destinations. */
export function navigationDestinationOf(route = {}) {
  const projectKey = routeProjectKey(route);
  if (!projectKey) return null;
  if (route.name === "workspace" && routeWorkspaceKey(route)) {
    return { kind: "workspace", key: routeWorkspaceKey(route), projectKey };
  }
  return PROJECT_HOSTS.has(route.name) ? { kind: "project", key: projectKey, projectKey } : null;
}

/** Pass the actual hash to keep its spelling/query order as well as its
 * parsed place. Without it, the existing canonical writer supplies the link.
 * A controller closes the picker and skips navigation when current is true. */
export function destinationTarget(destination, { route = {}, currentHash = null } = {}) {
  const standing = navigationDestinationOf(route);
  const current = standing?.kind === destination.kind && standing?.key === destination.key;
  const target = current ? route : destination.route;
  return { current, route: target, hash: current && currentHash !== null ? currentHash : hashFromRoute(target) };
}

/** The ancestor breadcrumb and local Project workspace return row. On the
 * project page itself it is a self-link; a project-hosted task still has a
 * real parent link. Return validation stays in projectRailState. */
export async function projectReturnTarget(project, { route = {}, currentHash = null } = {}) {
  const target = projectRoute(project);
  const key = routeProjectKey(route);
  if (route.name === "project" && key && key === routeProjectKey(target)) {
    return { current: true, route, hash: currentHash ?? hashFromRoute(route) };
  }
  const returned = await projectReturnRoute(project);
  return { current: false, route: returned, hash: hashFromRoute(returned) };
}

/** Picker rows advertise returnToProject only in the local sibling list.
 * Global/rail choices use ordinary Tasks/Changes/Files destinations. */
export async function navigationSelectionTarget(destination, context = {}) {
  const target = destinationTarget(destination, context);
  return destination.returnToProject
    ? projectReturnTarget({ id: destination.projectId, deviceId: destination.deviceId }, context)
    : target;
}
