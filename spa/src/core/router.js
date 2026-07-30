// Hash routes: #/notifications (the landing surface), #/settings,
// #/project/<projectId>       — one project's Inbox,
// #/project/<projectId>/<tab> — that project's primary checkout tabs.
// #/project/<projectId>/task/<runId>/<tab>
// #/project/<projectId>/issue/<issueId>/<tab>[/<stage>]
// Legacy plan routes parse to the same internal surface and canonicalize on write.
// #/project/<projectId>/worktree/<worktreeId>/<tab>
// The old top-level task/plan/worktree/main forms remain legacy aliases.
// Pure mapping both ways; the app shell owns the hashchange listener.

const isTermTab = (seg) => /^term-\d+$/.test(seg || "");

// Each surface has its own valid tab vocabulary; an unknown/absent segment
// falls back to that surface's Conversation default. Terminal tabs (`term-<n>`) are valid
// on every worktree-backed surface. A run's plan doc moved to the plan route, so
// a legacy #/task/<id>/plan (and the merged Diff tab) both land on Changes.
const runTab = (seg) =>
  seg === "conversation" || seg === "changes" || seg === "files" || seg === "agent" || seg === "stages" || isTermTab(seg)
    ? seg
    : seg === "diff" || seg === "plan"
      ? "changes"
      : "conversation";
// A plan is project-scoped: Conversation, Stages/Plan, and its disposable-worktree
// agent screen. Plans are not a terminal scope, so `term-<n>` falls back to Conversation.
const planTab = (seg) => (seg === "review" ? "stages" : seg === "agent" || seg === "stages" ? seg : "conversation");
// A worktree surface is Conversation + Changes + Files + its ONE Build-owned agent. The Agent
// tab is a fixture there (surfaceTabs.js AGENT_TAB) — always reachable means
// reachable by URL too, so a reload or a shared link stays on it.
const worktreeSurfaceTabs = new Set(["conversation", "changes", "files", "agent"]);
const worktreeTab = (seg) => (seg === "diff" ? "changes" : worktreeSurfaceTabs.has(seg) || isTermTab(seg) ? seg : "conversation");
// The project surface IS the primary checkout's worktree surface plus its
// project-scoped panes (Inbox, Issues, Archive). Same vocabulary for the canonical
// #/project/<id>/<tab> form and the legacy #/main/<id>/<tab> alias; they differ
// only in which tab an unknown segment falls back to.
const projectSurfaceTabs = new Set([...worktreeSurfaceTabs, "inbox", "issues", "archive"]);
const projectSurfaceTab = (seg, fallback) => (projectSurfaceTabs.has(seg) || isTermTab(seg) ? seg : fallback);
const mainTab = (seg) => projectSurfaceTab(seg, "changes");
const projectTab = (seg) => projectSurfaceTab(seg, "inbox");

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
    case "issue":
    case "plan": {
      if (!parts[1]) return { name: "notifications" };
      // An optional 4th segment deep-links one stage plan document.
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
    case "project": {
      if (!parts[1]) return { name: "notifications" };
      const projectId = decodeURIComponent(parts[1]);
      if (parts[2] === "task") {
        if (!parts[3]) return { name: "notifications" };
        return { name: "task", projectId, id: decodeURIComponent(parts[3]), tab: runTab(parts[4]) };
      }
      if (parts[2] === "issue" || parts[2] === "plan") {
        if (!parts[3]) return { name: "notifications" };
        const route = { name: "plan", projectId, id: decodeURIComponent(parts[3]), tab: planTab(parts[4]) };
        if (parts[5]) route.stage = decodeURIComponent(parts[5]);
        return route;
      }
      if (parts[2] === "worktree") {
        if (!parts[3]) return { name: "notifications" };
        return { name: "worktree", projectId, worktreeId: decodeURIComponent(parts[3]), tab: worktreeTab(parts[4]) };
      }
      return { name: "project", projectId, tab: projectTab(parts[2]) };
    }
    case "board":
      // The board is gone; stale bookmarks land on the notifications surface.
      return { name: "notifications" };
    default:
      return { name: "notifications" };
  }
}

export function hashFromRoute(route) {
  if (route.name === "task") {
    const leaf = `task/${encodeURIComponent(route.id)}/${route.tab || "conversation"}`;
    return route.projectId ? `#/project/${encodeURIComponent(route.projectId)}/${leaf}` : `#/${leaf}`;
  }
  if (route.name === "plan") {
    const leaf = `issue/${encodeURIComponent(route.id)}/${route.tab || "conversation"}`;
    const base = route.projectId ? `#/project/${encodeURIComponent(route.projectId)}/${leaf}` : `#/${leaf}`;
    return route.stage ? `${base}/${encodeURIComponent(route.stage)}` : base;
  }
  if (route.name === "worktree") {
    const leaf = `worktree/${encodeURIComponent(route.worktreeId)}/${route.tab || "conversation"}`;
    return `#/project/${encodeURIComponent(route.projectId)}/${leaf}`;
  }
  if (route.name === "main") return `#/project/${encodeURIComponent(route.projectId)}/${route.tab || "changes"}`;
  if (route.name === "project") {
    const base = `#/project/${encodeURIComponent(route.projectId)}`;
    return !route.tab || route.tab === "inbox" ? base : `${base}/${route.tab}`;
  }
  if (route.name === "notifications") return "#/notifications";
  if (route.name === "settings") return "#/settings";
  return "#/notifications";
}
