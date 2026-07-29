// The external-worktree surface: the same shell every other worktree-shaped
// surface has — Changes, Files, and one tab per terminal. Changes is the full
// git GUI (commit rail, uncommitted staging, history, branch and sync verbs)
// scoped to this worktree, with the review diff plugged in as its pinned "All
// changes" entry. Nothing here adopts the worktree except the review plug's own
// verbs (Request Changes, Merge, Abandon), which bind it to a task first.
//
// Everything rendered from the worktree's branch and path is UNTRUSTED and
// escaped.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go } from "../app.js";
import { hashFromRoute } from "../core/router.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab, mountAgentTab, AGENT_TAB, NEW_TAB_KINDS } from "../core/surfaceTabs.js";
import { mountGitPane } from "../core/gitPane.js";
import { createAdoptingCall } from "../core/adoption.js";
import { createWorktreeReview } from "./worktreeReview.js";
import { takeNewWorktreeMark } from "../core/newWorktree.js";

/** The external-worktree surface's tabs, in row order. Agent is a fixture: this
 *  directory has one agent whether or not Build has ever adopted it, and it is
 *  always somewhere you can look. Mounting the tab starts nothing — the agent
 *  begins when a human→agent verb (the first Request Changes here) delivers a
 *  turn, which is also what adopts the worktree. */
export const worktreeSurfaceTabs = (terminalTabs = []) => [
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
  AGENT_TAB,
  ...terminalTabs,
];

export async function renderWorktree() {
  const root = $("#root");
  const projectId = App.route.projectId;
  const worktreeId = App.route.worktreeId;
  // Seeing a worktree settles its dot. It does NOT count as interacting with it
  // — a hand-made worktree still needs a real action before it joins the rail.
  App.call("entity.seen", { entity_id: worktreeId }).catch(() => {});
  const adopting = createAdoptingCall((method, params) => App.call(method, params), projectId, worktreeId);
  const scope = { project_id: projectId, worktree_id: worktreeId };

  // A worktree Build just minted carries the answer the sheet asked for: WHICH
  // AGENT runs here. It seeds the provider adoption dispatches with, and it lands
  // this surface on its Agent tab rather than on an empty diff. The mark is
  // one-shot, so a reload of the same surface behaves like any other visit.
  const pendingProvider = takeNewWorktreeMark(worktreeId);
  let tab = App.route.tab || "changes";
  const terminals = terminalTabsController(scope);
  let shellCtl = null;
  let aux = null;
  let meta = { branch: "", path: "" };
  // Latched once this surface has handed the worktree over (adopted into a task,
  // merged, abandoned) or found it gone: a late RPC rejection must not repaint
  // over the destination.
  let leaving = false;

  const staticTabs = () => worktreeSurfaceTabs(terminals.tabs());
  const replaceWorktreeHash = () => {
    App.route = { name: "worktree", projectId, worktreeId, tab };
    history.replaceState(null, "", hashFromRoute(App.route));
  };
  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };

  const goHome = () => {
    leaving = true;
    go({ name: "project", projectId });
  };

  // Once adoption has succeeded this worktree is bound to a task, so every
  // worktree-scoped RPC (and poll) will report "unknown worktree_id" — that is
  // the EXPECTED post-adoption state, not "the worktree vanished". Hand off to
  // the freshly minted task, which now holds any merge_failed reason.
  const handoffToTask = () => {
    leaving = true;
    go({ name: "task", projectId, id: adopting.adoptedRunId(), tab: "changes" });
  };

  const renderNotFound = () => {
    leaving = true;
    disposeAux();
    root.innerHTML = `
      <div class="back" id="back">← Project</div>
      <div class="empty">This worktree is no longer available — it may have been adopted or removed.</div>`;
    $("#back").onclick = () => go({ name: "project", projectId });
  };

  // The worktree stopped resolving: either this surface adopted it (hand off to
  // the task that now owns it) or it was genuinely removed.
  const worktreeGone = () => {
    if (leaving) return;
    if (adopting.adoptedRunId()) handoffToTask();
    else renderNotFound();
  };

  // Every worktree-scoped RPC goes through here: git.status keeps the bar's
  // branch current (the git pane polls it anyway, so the header needs no poll of
  // its own), and a worktree that stops resolving ends the surface.
  const callRpc = async (method, params) => {
    try {
      const res = await App.call(method, params);
      if (method === "git.status") refreshHeader(res);
      return res;
    } catch (e) {
      if (String(e && e.message).includes("unknown worktree_id")) worktreeGone();
      throw e;
    }
  };

  // The review surface (the Changes rail's "All changes" entry). ONE instance
  // for the view's whole life, so pending review comments survive tab switches
  // and the rail selection moving away and back.
  const reviewPlug = createWorktreeReview({
    projectId,
    worktreeId,
    callRpc: (method, params) => App.call(method, params),
    adopting,
    initialProvider: pendingProvider,
    isOffline: () => App.offline,
    onAdopted: () => handoffToTask(),
    onFinished: () => goHome(),
    onGone: () => worktreeGone(),
    onMeta: (m) => refreshHeader(m),
  });

  // The tab bar is the top of the view; the worktree's identity (branch) rides
  // the bar's right cluster, path on hover.
  const shell = () => {
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="tabrow"></div>
        <div class="surface-meta">
          <span class="mono dim" id="wtbranch" title="${esc(meta.path || "")}">${esc(meta.branch || "(detached)")}</span>
        </div>
      </div>
      <div id="tabbody"></div>`;
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (tabId) => selectTab(tabId),
      onClose: (tabId) => closeTerminal(tabId),
      newTabOptions: NEW_TAB_KINDS,
      onNewTab: (kind) => newTerminal(kind),
      back: { title: "Back to project" },
      onBack: () => goHome(),
    });
  };

  // Keep the bar's branch/path current from whatever reported it last — the git
  // pane's status polls on Changes, the review plug's diff elsewhere. Guarded
  // against departure: a response resolving after navigation must not overwrite
  // the next view's header (#root now belongs to it).
  const refreshHeader = (source) => {
    if (leaving || !source) return;
    const next = { branch: source.branch || "", path: source.path || meta.path || "" };
    if (next.branch === meta.branch && next.path === meta.path) return;
    meta = next;
    const branchEl = root.querySelector("#wtbranch");
    if (branchEl) {
      branchEl.textContent = meta.branch || "(detached)";
      branchEl.title = meta.path || "";
    }
  };

  const selectTab = (tabId) => {
    tab = tabId;
    replaceWorktreeHash();
    if (shellCtl) shellCtl.setActive(tabId);
    disposeAux();
    const body = $("#tabbody");
    if (!body) return;
    // Terminal tabs go edge-to-edge; Changes and Files run flush (their own rail
    // + detail panes each scroll internally, so the body owns no padding).
    body.classList.toggle("bare", tabId === "agent" || /^term-/.test(tabId));
    body.classList.toggle("flush", tabId === "changes" || tabId === "files");
    if (tabId === "agent") {
      // This worktree's one agent. Mounting is a look, never a start: an agent
      // that has not been asked for anything yet shows the idle label until the
      // first delivered turn adopts the worktree and spawns it.
      // Starting the agent is a mutating act on this directory, so it adopts —
      // the same transparent adoption the first Request Changes performs, and
      // the same reason: an agent needs an owner for `done` to report to.
      aux = mountAgentTab(body, scope, {
        onStart: () => adopting.runCall("agent.start", {}),
      });
      return;
    }
    if (tabId === "changes") {
      // The git surface for this worktree: the commit rail on the left, the
      // review diff ("All changes", the review plug), staging, or a commit's
      // detail on the right. The pane owns its own 1.6s poll.
      aux = mountGitPane(body, {
        scope,
        callRpc,
        review: {
          getBase: () => reviewPlug.getBase(),
          mount: (host) => reviewPlug.mount(host),
          unmount: () => reviewPlug.unmount(),
        },
      });
      return;
    }
    aux = mountAuxTab(body, tabId, {
      scope,
      callRpc: (method, params) => App.call(method, params),
      onExit: () => {
        terminals.drop(tabId);
        if (shellCtl) shellCtl.setTabs(staticTabs());
        selectTab("changes");
      },
    });
  };

  const newTerminal = async (kind) => {
    let termId;
    try {
      termId = await terminals.create(kind);
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

  // Tear down the mounted pane (and, with it, the review plug's poll) when the
  // user navigates away.
  App.viewDispose = () => {
    leaving = true;
    disposeAux();
  };

  replaceWorktreeHash();
  // Paint the surface first: neither the header seed nor the terminal list is
  // worth an empty screen while a socket answers.
  shell();
  selectTab(tab);
  // Seed the header so the bar names the branch even on a tab that never polls
  // git.status. A rejection here is the worktree not resolving, which callRpc
  // has already turned into the gone/handoff path.
  callRpc("git.status", { ...scope }).catch(() => {});
  // Then the worktree's open terminals, as tabs beside Changes and Files.
  await terminals.load();
  if (leaving) return;
  if (shellCtl) shellCtl.setTabs(staticTabs());
  // The sheet already named the agent that runs here, so land on the tab it will
  // appear in — after the first paint, which is what gives the pane a tab row and
  // a body to mount into.
  if (pendingProvider) selectTab("agent");
}
