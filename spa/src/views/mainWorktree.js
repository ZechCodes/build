// The primary-checkout "main" surface: a project's own live checkout as a
// worktree-backed surface. Tabs: Changes (uncommitted delta vs HEAD, read-only —
// the primary checkout is the user's own working tree, so no review actions),
// Files, and user terminal tabs with `+`. Scope: { project_id }.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { parseDiff, filterNoiseFiles } from "../core/diff.js";
import { diffFilesHtml } from "../core/diffRender.js";
import { App, go } from "../app.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab } from "../core/surfaceTabs.js";

export async function renderMain() {
  const root = $("#root");
  const projectId = App.route.projectId;
  const scope = { project_id: projectId };
  const terminals = terminalTabsController(scope);

  let tab = App.route.tab || "changes";
  let projectName = projectId;
  let shellCtl = null; // tab-row controller
  let aux = null; // current files/terminal pane controller
  let changesKey = null; // patch-keyed freeze for the Changes poll
  let meta = { branch: "", path: "" };

  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };
  const stopPoll = () => {
    if (App.poll) {
      clearInterval(App.poll);
      App.poll = null;
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
    stopPoll();
    changesKey = null;
    const body = $("#tabbody");
    if (id === "changes") {
      paintChanges();
      App.poll = setInterval(paintChanges, 1600);
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
    await terminals.close(termId);
    if (shellCtl) shellCtl.setTabs(staticTabs());
    if (tab === termId) selectTab("changes");
  };

  const paintChanges = async () => {
    if (App.offline) return;
    let res;
    try {
      res = await App.call("project.diff", { project_id: projectId });
    } catch {
      return; // transient — the poll retries
    }
    const nextMeta = { branch: res.branch, path: res.path };
    if (nextMeta.branch !== meta.branch || nextMeta.path !== meta.path) {
      meta = nextMeta;
      const h1 = root.querySelector(".thead h1");
      if (h1) h1.textContent = meta.branch || "(detached)";
      const metaEl = root.querySelector(".tmeta span");
      if (metaEl) metaEl.textContent = meta.path || "";
    }
    if (tab !== "changes") return;
    const key = res.patch || "";
    if (changesKey === key && $("#mainchanges")) return;
    changesKey = key;
    const files = filterNoiseFiles(parseDiff(res.patch));
    const totalIns = files.reduce((a, f) => a + f.add, 0),
      totalDel = files.reduce((a, f) => a + f.del, 0);
    $("#tabbody").innerHTML = `
      <div id="mainchanges">
        <div class="diffbar"><span>${files.length} files <span style="color:var(--green)">+${totalIns}</span> <span style="color:var(--red)">−${totalDel}</span></span></div>
        ${files.length ? diffFilesHtml(files) : '<div class="empty">No uncommitted changes.</div>'}</div>`;
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
  App.viewDispose = () => disposeAux();

  shell();
  await terminals.load();
  if (shellCtl) shellCtl.setTabs(staticTabs());
  selectTab(tab);
}
