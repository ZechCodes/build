// Keep references to the mounted review inside its existing reading surface.
import { routeFromHash } from "./router.js";

const sameTask = (route, scope) => route.name === "trackerTask"
  && route.deviceId === scope.deviceId && route.projectId === scope.projectId && route.taskId === scope.taskId;
const ordinaryClick = (event) => event.button === 0 && !event.metaKey && !event.ctrlKey && !event.shiftKey && !event.altKey;

export function installReviewLinkNavigation(scope, open, root = document) {
  const follow = (event) => {
    if (event.defaultPrevented || !ordinaryClick(event)) return;
    const anchor = event.target?.closest?.("a.md-ref");
    if (!anchor || !sameTask(routeFromHash(location.hash), scope)) return;
    const target = routeFromHash(anchor.getAttribute("href"));
    if (!target.review || !sameTask(target, scope)) return;
    event.preventDefault();
    open(target.review);
  };
  root.addEventListener("click", follow);
  return () => root.removeEventListener("click", follow);
}
