// The primary-checkout "main" surface: a project's own live checkout as a
// worktree-backed surface. Tabs: Changes (the git surface — commit history
// plus per-file staging and commit for the user's own work; review actions
// still live on task/worktree surfaces), Files, and user terminal tabs with
// `+`. Scope: { project_id }.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab } from "../core/surfaceTabs.js";
import { mountGitPane } from "../core/gitPane.js";

export async function renderMain() {
  const root = $("#root");
  const projectId = App.route.projectId;
  const scope = { project_id: projectId };
  const terminals = terminalTabsController(scope);

  let tab = App.route.tab || "changes";
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

  const staticTabs = () => [
    { id: "changes", label: "Changes" },
    { id: "files", label: "Files" },
    ...terminals.tabs(),
  ];

  const shell = () => {
    root.innerHTML = `
      <div class="back" id="back">← ${esc(projectName)}</div>
      <div class="thead"><h1>${esc(meta.branch || "(detached)")}</h1>
        <div class="right"><span class="chip">MAIN</span></div></div>
      <div class="tmeta"><span>${esc(meta.path || "")}</span></div>
      <div class="tabrow" id="tabrow"></div>
      <div id="tabbody"></div>`;
    $("#back").onclick = () => go({ name: "project", projectId });
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (id) => selectTab(id),
      onClose: (id) => closeTerminal(id),
      onNewTerminal: () => newTerminal(),
    });
  };

  const selectTab = (id) => {
    tab = id;
    App.route.tab = id;
    history.replaceState(null, "", `#/main/${encodeURIComponent(projectId)}/${id}`);
    if (shellCtl) shellCtl.setActive(id);
    disposeAux();
    const body = $("#tabbody");
    if (id === "changes") {
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
    const h1 = root.querySelector(".thead h1");
    if (h1) h1.textContent = meta.branch || "(detached)";
    const metaEl = root.querySelector(".tmeta span");
    if (metaEl) metaEl.textContent = meta.path || "";
  };

  // Fetch the project name for the back link (best-effort; falls back to the id).
  try {
    const projectList = await App.call("project.list");
    const project = (projectList.projects || []).find((p) => p.project_id === projectId);
    if (project) projectName = project.name;
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
