// Sidebar wiring: renders the project rail on every page, keeps expand /
// collapse state, and routes clicks. Data arrives via the shared task feed.

import { $ } from "../dom.js";
import { App, go } from "../app.js";
import { buildSidebarModel, sidebarHtml } from "../core/sidebar.js";
import { subscribeFeed, refreshFeed } from "../core/taskFeed.js";
import { setBadge } from "./shared.js";
import { openBrowser } from "../sheets/browser.js";
import { openClone } from "../sheets/clone.js";
import { openNewRepo } from "../sheets/newRepo.js";

const COLLAPSED_KEY = "build.sidebar.collapsed";
const CLOSED_KEY = "build.sidebar.closedProjects";

const closedProjects = new Set(JSON.parse(localStorage.getItem(CLOSED_KEY) || "[]"));
const wtOpen = new Set(); // per-session: which projects' worktree lists are unfolded
let lastFeed = null;

function persistClosed() {
  localStorage.setItem(CLOSED_KEY, JSON.stringify([...closedProjects]));
}

export function setSidebarCollapsed(on) {
  document.body.classList.toggle("sidebar-collapsed", on);
  localStorage.setItem(COLLAPSED_KEY, on ? "1" : "");
}

// Navigating from a rail row on a narrow viewport collapses the sidebar so the
// destination surface isn't hidden behind the overlaid rail (tap the scrim, or
// the open toggle, to bring it back). On a wide viewport the rail is docked, so
// it stays put.
function goFromRail(route) {
  go(route);
  if (window.innerWidth < 900) setSidebarCollapsed(true);
}

function activeRunId() {
  return App.route.name === "task" ? App.route.id : null;
}

function activePlanId() {
  return App.route.name === "plan" ? App.route.id : null;
}

/** The project that owns whatever surface the route shows — the project page
 *  itself, external worktrees (project in the route), or a plan/run
 *  surface (owner resolved through the feed) — so the rail always shows where
 *  you are, not just which leaf row. */
function activeProjectId() {
  const route = App.route;
  if (route.name === "project" || route.name === "main" || route.name === "worktree") {
    return route.projectId || null;
  }
  if (!lastFeed) return null;
  if (route.name === "plan") {
    const plan = (lastFeed.plans || []).find((p) => p.plan_id === route.id);
    return plan ? plan.project_id : null;
  }
  if (route.name === "task") {
    const run = (lastFeed.runs || []).find((r) => r.run_id === route.id);
    return run ? run.project_id : null;
  }
  return null;
}

function activeMainProjectId() {
  const route = App.route;
  return route.name === "main" || (route.name === "project" && route.tab !== "inbox")
    ? route.projectId
    : null;
}

function activeWorktreeId() {
  return App.route.name === "worktree" ? App.route.worktreeId : null;
}

function draw() {
  if (!lastFeed) return;
  const aside = $("#sidebar-rail");
  if (!aside) return;
  const model = buildSidebarModel({
    projects: lastFeed.projects,
    runs: lastFeed.runs,
    plans: lastFeed.plans,
    externalWorktrees: lastFeed.externalWorktrees,
    primaryChanges: lastFeed.primaryChanges,
    readIds: App.readIds,
    nowMs: Date.now(),
  });
  const scroll = aside.scrollTop;
  aside.innerHTML = sidebarHtml(model, {
    closed: closedProjects,
    wtOpen,
    activeRunId: activeRunId(),
    activePlanId: activePlanId(),
    activeProjectId: activeProjectId(),
    activeMainProjectId: activeMainProjectId(),
    activeWorktreeId: activeWorktreeId(),
  });
  aside.scrollTop = scroll;
  setBadge(lastFeed.runs, lastFeed.plans);

  $("#side-add").onclick = openAddProjectMenu;
  // The chevron toggles; the name navigates to the project page and expands it.
  aside.querySelectorAll("[data-chev]").forEach((b) => {
    b.onclick = () => {
      const pid = b.dataset.chev;
      closedProjects.has(pid) ? closedProjects.delete(pid) : closedProjects.add(pid);
      persistClosed();
      draw();
    };
  });
  aside.querySelectorAll("[data-open]").forEach((el) => {
    el.onclick = () => {
      const pid = el.dataset.open;
      closedProjects.delete(pid);
      persistClosed();
      goFromRail({ name: "project", projectId: pid });
    };
  });
  aside.querySelectorAll(".srow[data-run]").forEach((r) => {
    r.onclick = () => goFromRail({ name: "task", id: r.dataset.run, tab: r.dataset.tab });
  });
  aside.querySelectorAll(".srow[data-plan]").forEach((r) => {
    r.onclick = () => goFromRail({ name: "plan", id: r.dataset.plan, tab: "review" });
  });
  aside.querySelectorAll(".srow[data-wtline]").forEach((r) => {
    r.onclick = () => {
      const pid = r.dataset.wtline;
      wtOpen.has(pid) ? wtOpen.delete(pid) : wtOpen.add(pid);
      draw();
    };
  });
  aside.querySelectorAll(".srow[data-wt]").forEach((r) => {
    r.onclick = () => goFromRail({ name: "worktree", projectId: r.dataset.project, worktreeId: r.dataset.wt });
  });
  aside.querySelectorAll(".srow[data-main]").forEach((r) => {
    r.onclick = () => goFromRail({ name: "project", projectId: r.dataset.main, tab: "changes" });
  });
}

/** The + menu: the three existing add flows, reused as-is. */
function openAddProjectMenu() {
  const done = () => {
    $("#scrim").classList.remove("show");
    refreshFeed();
  };
  $("#sheet").innerHTML = `
    <h3>Add a project</h3>
    <div class="sub">Point Build at a repo on this device — agents work in worktrees beside it.</div>
    <div class="addmenu">
      <button class="btn" id="ap-browse">Browse for an existing repo…</button>
      <button class="btn" id="ap-clone">Clone a remote…</button>
      <button class="btn" id="ap-new">Create a new repo…</button>
    </div>
    <div class="row"><button class="btn" id="ap-cancel" style="margin-left:auto">Cancel</button></div>
    <div class="adderr" id="berr"></div>`;
  $("#scrim").classList.add("show");
  $("#ap-cancel").onclick = () => $("#scrim").classList.remove("show");
  $("#ap-browse").onclick = () =>
    openBrowser({
      title: "Browse for a git repo",
      gitOnly: true,
      onChoose: async (path) => {
        try {
          await App.call("project.add", { path });
          done();
        } catch (e) {
          const err = $("#berr");
          if (err) err.textContent = e.message;
        }
      },
    });
  $("#ap-clone").onclick = () => openClone(done);
  $("#ap-new").onclick = () => openNewRepo(done);
}

let mounted = false;

/** Mount once: subscribe to the feed and wire the collapse/reopen toggles.
 *  Re-entrant — a reconnect calls this again and just repaints. */
export function initSidebar() {
  if (mounted) {
    draw();
    return;
  }
  mounted = true;
  setSidebarCollapsed(
    localStorage.getItem(COLLAPSED_KEY) === "1" ||
      (localStorage.getItem(COLLAPSED_KEY) === null && window.innerWidth < 900)
  );
  $("#side-collapse").onclick = () => setSidebarCollapsed(true);
  $("#side-open").onclick = () => setSidebarCollapsed(false);
  // On a narrow viewport the open rail overlays the content behind a scrim;
  // tapping it dismisses the rail (the sheet scrim precedent).
  const scrim = $("#side-scrim");
  if (scrim) scrim.onclick = () => setSidebarCollapsed(true);
  subscribeFeed((feed) => {
    lastFeed = feed;
    draw();
  });
}

/** Re-render on route changes so the active row tracks navigation. */
export function sidebarRouteChanged() {
  draw();
}
