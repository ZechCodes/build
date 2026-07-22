// Hash routes: #/notifications (the landing surface), #/settings,
// #/task/<runId>/<tab>       — a run (worktree-scoped; "Task" in the UI),
// #/plan/<planId>/<tab>      — a plan (project-scoped review surface),
// #/worktree/<projectId>/<worktreeId>/<tab> (read-only external-worktree browse),
// #/project/<projectId>       — one project's Inbox,
// #/project/<projectId>/<tab> — that project's primary checkout tabs.
// #/main/<projectId>/<tab> is accepted only as a legacy alias.
// Pure mapping both ways; the app shell owns the hashchange listener.

const isTermTab = (seg) => /^term-\d+$/.test(seg || "");

// Each surface has its own valid tab vocabulary; an unknown/absent segment
// falls back to that surface's default tab. Terminal tabs (`term-<n>`) are valid
// on every worktree-backed surface. A run's plan doc moved to the plan route, so
// a legacy #/task/<id>/plan (and the merged Diff tab) both land on Changes.
const runTab = (seg) =>
  seg === "changes" || seg === "files" || seg === "agent" || seg === "stages" || isTermTab(seg) ? seg : "changes";
// A plan is project-scoped: the review doc plus its disposable-worktree agent
// screen. Plans are not a terminal scope, so `term-<n>` falls back to review.
const planTab = (seg) => (seg === "agent" ? "agent" : "review");
const worktreeTab = (seg) => (seg === "changes" || seg === "files" || isTermTab(seg) ? seg : "changes");
const mainTab = (seg) => (seg === "files" || isTermTab(seg) ? seg : "changes");
const projectTab = (seg) =>
  seg === "changes" || seg === "files" || isTermTab(seg) ? seg : "inbox";

export function routeFromHash(hash) {
  const parts = (hash || "").replace(/^#\/?/, "").split("/").filter(Boolean);
  switch (parts[0]) {
    case "notifications":
      return { name: "notifications" };
    case "settings":
      return { name: "settings" };
    case "task":
      if (!parts[1]) return { name: "notifications" };
      return { name: "task", id: decodeURIComponent(parts[1]), tab: runTab(parts[2]) };
    case "plan": {
      if (!parts[1]) return { name: "notifications" };
      // An optional 4th segment deep-links one stage's doc — the run's Stages
      // tab links here so a mid-run revision lands on the exact plan doc.
      const route = { name: "plan", id: decodeURIComponent(parts[1]), tab: planTab(parts[2]) };
      if (parts[3]) route.stage = decodeURIComponent(parts[3]);
      return route;
    }
    case "worktree":
      if (!parts[1] || !parts[2]) return { name: "notifications" };
      return {
        name: "worktree",
        projectId: decodeURIComponent(parts[1]),
        worktreeId: decodeURIComponent(parts[2]),
        tab: worktreeTab(parts[3]),
      };
    case "main":
      if (!parts[1]) return { name: "notifications" };
      return { name: "project", projectId: decodeURIComponent(parts[1]), tab: mainTab(parts[2]) };
    case "project":
      if (!parts[1]) return { name: "notifications" };
      return { name: "project", projectId: decodeURIComponent(parts[1]), tab: projectTab(parts[2]) };
    case "board":
      // The board is gone; stale bookmarks land on the notifications surface.
      return { name: "notifications" };
    default:
      return { name: "notifications" };
  }
}

export function hashFromRoute(route) {
  if (route.name === "task") return `#/task/${encodeURIComponent(route.id)}/${route.tab || "changes"}`;
  if (route.name === "plan") {
    const base = `#/plan/${encodeURIComponent(route.id)}/${route.tab || "review"}`;
    return route.stage ? `${base}/${encodeURIComponent(route.stage)}` : base;
  }
  if (route.name === "worktree")
    return `#/worktree/${encodeURIComponent(route.projectId)}/${encodeURIComponent(route.worktreeId)}/${route.tab || "changes"}`;
  if (route.name === "main") return `#/project/${encodeURIComponent(route.projectId)}/${route.tab || "changes"}`;
  if (route.name === "project") {
    const base = `#/project/${encodeURIComponent(route.projectId)}`;
    return !route.tab || route.tab === "inbox" ? base : `${base}/${route.tab}`;
  }
  if (route.name === "notifications") return "#/notifications";
  if (route.name === "settings") return "#/settings";
  return "#/notifications";
}
