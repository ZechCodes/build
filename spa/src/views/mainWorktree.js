// One unified project surface. The root tab is Inbox; Changes, Files, and user
// terminals operate on the project's primary checkout. Creation actions stay
// in the shared header so they are available on every project tab.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab, mountAgentTab, AGENT_TAB, NEW_TAB_KINDS } from "../core/surfaceTabs.js";
import { mountGitPane } from "../core/gitPane.js";
import { mountProjectInbox } from "./project.js";
import { mountIssuesTab } from "./issues.js";
import { mountArchiveTab } from "./archive.js";
import { openNewIssue } from "../sheets/newIssue.js";

/** The project surface's tabs, in row order. The primary checkout is a worktree
 *  like any other, so it carries the same Agent fixture — the one agent that
 *  can run in the repo root, always reachable, never started by opening it. */
export const projectSurfaceTabs = (terminalTabs = []) => [
  { id: "inbox", label: "Inbox" },
  { id: "issues", label: "Issues" },
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
  { id: "archive", label: "Archive" },
  AGENT_TAB,
  ...terminalTabs,
];

export async function renderMain() {
  const root = $("#root");
  const projectId = App.route.projectId;
  const scope = { project_id: projectId };
  const terminals = terminalTabsController(scope);

  let tab = App.route.tab || "inbox";
  let projectName = projectId;
  let shellCtl = null; // tab-row controller
  let aux = null; // current changes/files/terminal pane controller
  let meta = { branch: "", path: "" };
  let viewDisposed = false; // set on navigation — stale RPC responses must not touch #root

  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };

  const staticTabs = () => projectSurfaceTabs(terminals.tabs());

  // The tab bar is the top of the view; the checkout's branch rides the bar's
  // right cluster (path on hover) and stays live via refreshHeader.
  const shell = () => {
    root.innerHTML = `
      <div class="surface-bar project-surface">
        <div class="tabrow" id="tabrow"></div>
        <div class="surface-meta">
          <span class="mono dim" id="mainbranch" title="${esc(meta.path || "")}">${esc(meta.branch || "(detached)")}</span>
        </div>
      </div>
      <div id="tabbody"></div>`;
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (id) => selectTab(id),
      onClose: (id) => closeTerminal(id),
      newTabOptions: NEW_TAB_KINDS,
      onNewTab: () => newTerminal(),
    });
  };

  const selectTab = (id) => {
    tab = id;
    App.route = { name: "project", projectId, tab: id };
    history.replaceState(null, "", hashFromRoute(App.route));
    if (shellCtl) shellCtl.setActive(id);
    disposeAux();
    const body = $("#tabbody");
    // Terminal tabs go edge-to-edge; Changes/Files run flush (their own rail +
    // detail panes each scroll internally, so the body owns no padding/scroll).
    body.classList.toggle("bare", id === "agent" || /^term-/.test(id));
    body.classList.toggle("flush", id === "changes" || id === "files");
    if (id === "agent") {
      // The primary checkout's own agent, addressed by the project scope. Build
      // dispatches its runs into their own worktrees, so this is normally empty
      // — but a worktree the human works in directly can hold one, and the tab
      // is where it shows up.
      aux = mountAgentTab(body, scope, {
        idleLabel: "no agent is running in this checkout",
      });
    } else if (id === "inbox") {
      aux = mountProjectInbox(body, {
        projectId,
        callRpc: (method, params) => App.call(method, params),
      });
    } else if (id === "issues") {
      aux = mountIssuesTab(body, {
        projectId,
        callRpc: (method, params) => App.call(method, params),
        navigate: go,
        onNewIssue: () => openNewIssue({ projectId }),
      });
    } else if (id === "archive") {
      aux = mountArchiveTab(body, {
        projectId,
        callRpc: (method, params) => App.call(method, params),
        navigate: go,
      });
    } else if (id === "changes") {
      // The git pane owns its own poll; refreshHeader rides its git.status
      // responses so the branch/path header stays live while it runs.
      aux = mountGitPane(body, {
        scope,
        callRpc: (method, params) =>
          App.call(method, params).then((res) => {
            if (method === "git.status") refreshHeader(res);
            return res;
          }),
        agentCommitOptions: [],
      });
    } else {
      aux = mountAuxTab(body, id, {
        scope,
        callRpc: (method, params) => App.call(method, params),
        onExit: () => {
          terminals.drop(id);
          if (shellCtl) shellCtl.setTabs(staticTabs());
          selectTab("changes");
        },
      });
    }
  };

  const newTerminal = async () => {
    let termId;
    try {
      termId = await terminals.create();
    } catch (e) {
      const body = $("#tabbody");
      if (body) body.innerHTML = `<div class="empty">cannot open a terminal: ${esc((e && e.message) || "error")}</div>`;
      return;
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    selectTab(termId);
  };

  const closeTerminal = async (termId) => {
    try {
      await terminals.close(termId);
    } catch {
      /* raced with the reaper — drop the tab regardless */
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    if (tab === termId) selectTab("changes");
  };

  // Keep the header's branch/path current from the git pane's status responses.
  // Guarded against disposal: a git.status response resolving after navigation
  // must not overwrite the next view's header (#root now belongs to it).
  const refreshHeader = (status) => {
    if (viewDisposed) return;
    const nextMeta = { branch: status.branch, path: status.path };
    if (nextMeta.branch === meta.branch && nextMeta.path === meta.path) return;
    meta = nextMeta;
    const branchEl = root.querySelector("#mainbranch");
    if (branchEl) {
      branchEl.textContent = meta.branch || "(detached)";
      branchEl.title = meta.path || "";
    }
  };

  // Fetch project identity for the persistent header (best-effort; the id and
  // live git.status metadata remain usable while offline).
  try {
    const projectList = await App.call("project.list");
    const project = (projectList.projects || []).find((p) => p.project_id === projectId);
    if (project) {
      projectName = project.name;
      meta = { branch: project.base_branch || "", path: project.path || "" };
    }
  } catch {
    /* offline — the id stands in */
  }

  // Tear down the active terminal/files pane when the user navigates away.
  App.viewDispose = () => {
    viewDisposed = true;
    disposeAux();
  };

  shell();
  await terminals.load();
  if (shellCtl) shellCtl.setTabs(staticTabs());
  selectTab(tab);
}
