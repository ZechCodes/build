// Hash routes: #/board, #/notifications, #/settings, #/task/<id>/<tab>,
// #/worktree/<projectId>/<worktreeId>/<tab> (read-only external-worktree browse),
// #/main/<projectId>/<tab> (the primary-checkout surface),
// #/project/<projectId> (one project's tasks + worktrees).
// Pure mapping both ways; the app shell owns the hashchange listener.

const isTermTab = (seg) => /^term-\d+$/.test(seg || "");

// Each surface has its own valid tab vocabulary; an unknown/absent segment
// falls back to that surface's default tab. Terminal tabs (`term-<n>`) are valid
// on every worktree-backed surface. The task Diff tab merged into Changes —
// legacy #/task/<id>/diff links land on the changes tab.
const taskTab = (seg) => {
  if (seg === "diff") return "changes";
  return seg === "changes" || seg === "files" || seg === "agent" || isTermTab(seg) ? seg : "plan";
};
const worktreeTab = (seg) => (seg === "files" || isTermTab(seg) ? seg : "diff");
const mainTab = (seg) => (seg === "files" || isTermTab(seg) ? seg : "changes");

export function routeFromHash(hash) {
  const parts = (hash || "").replace(/^#\/?/, "").split("/").filter(Boolean);
  switch (parts[0]) {
    case "notifications":
      return { name: "notifications" };
    case "settings":
      return { name: "settings" };
    case "task":
      if (!parts[1]) return { name: "board" };
      return { name: "task", id: decodeURIComponent(parts[1]), tab: taskTab(parts[2]) };
    case "worktree":
      if (!parts[1] || !parts[2]) return { name: "board" };
      return {
        name: "worktree",
        projectId: decodeURIComponent(parts[1]),
        worktreeId: decodeURIComponent(parts[2]),
        tab: worktreeTab(parts[3]),
      };
    case "main":
      if (!parts[1]) return { name: "board" };
      return { name: "main", projectId: decodeURIComponent(parts[1]), tab: mainTab(parts[2]) };
    case "project":
      if (!parts[1]) return { name: "board" };
      return { name: "project", projectId: decodeURIComponent(parts[1]) };
    default:
      return { name: "board" };
  }
}

export function hashFromRoute(route) {
  if (route.name === "task") return `#/task/${encodeURIComponent(route.id)}/${route.tab || "plan"}`;
  if (route.name === "worktree")
    return `#/worktree/${encodeURIComponent(route.projectId)}/${encodeURIComponent(route.worktreeId)}/${route.tab || "diff"}`;
  if (route.name === "main") return `#/main/${encodeURIComponent(route.projectId)}/${route.tab || "changes"}`;
  if (route.name === "project") return `#/project/${encodeURIComponent(route.projectId)}`;
  if (route.name === "notifications") return "#/notifications";
  if (route.name === "settings") return "#/settings";
  return "#/board";
}
