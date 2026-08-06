// The run view (a run is a "Task" in the UI): the worktree-scoped surface for
// implementing a plan. Tabs are Conversation, Stages (multi-stage runs only),
// Changes (the review diff + request-changes + the merge/git plug), Files, Agent, one per
// open terminal, and the tab bar's shared right cluster (Inbox, Issues, Archive, project settings). The plan doc left the run entirely — a compact reference header
// links back to the owning plan (an adopted run shows its goal). Live-polled every
// 1.6s; the aux tabs (Changes/Files/Agent/terminals) own their own bodies and are
// never repainted by the poll.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { App, go, loadModelCatalog, markEntityRead } from "../app.js";
import { RUN_STATE_LABEL, runChipClass } from "./shared.js";
import { canDelete, bannerText, deleteRunConfirm } from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { renderStagesTab, stageActionBusy, joinRunStages, runStagesFallback } from "./stages.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab, mountAgentTab, AGENT_TAB, NEW_TAB_KINDS } from "../core/surfaceTabs.js";
import { mountGitPane, taskAgentCommitOptions } from "../core/gitPane.js";
import { createTaskReview } from "./taskReview.js";
import { createThreadCache, threadHtml, wireThreadComposer, wireThreadLinks, wireThreadRevisionLinks } from "../core/thread.js";
import { hashFromRoute } from "../core/router.js";
import { RUN_TERMINAL_STATES } from "../core/board.js";
import { isProjectClusterTab, mountProjectClusterTab, projectClusterShellOptions } from "../core/projectCluster.js";

/** The task surface's tabs, in row order. Agent is a fixture here as it is on
 *  every worktree surface — the run's worktree has one agent and it is always
 *  reachable, whether or not a session is live in it right now. */
export const taskSurfaceTabs = ({ multiStage = false, terminalTabs = [] } = {}) => [
  { id: "conversation", label: "Conversation" },
  ...(multiStage ? [{ id: "stages", label: "Stages" }] : []),
  { id: "changes", label: "Changes" },
  { id: "files", label: "Files" },
  AGENT_TAB,
  ...terminalTabs,
];

export function taskRemovalAction(model = {}) {
  if (!canDelete(model.state)) return null;
  return {
    id: "deleteTask",
    label: "Delete",
    busyLabel: "deleting…",
  };
}

export async function renderTask() {
  const root = $("#root");
  const id = App.route.id;
  let projectId = App.route.projectId || null;
  let tab = App.route.tab || "conversation";

  // Looking at it IS seeing it: the dot settles to grey until the run moves
  // again. Fire-and-forget — a missed stamp costs one stale dot, not an action.
  App.call("entity.seen", { entity_id: id }).catch(() => {});

  const replaceTaskHash = () => {
    App.route = { name: "task", ...(projectId ? { projectId } : {}), id, tab };
    history.replaceState(null, "", hashFromRoute(App.route));
  };

  // Terminals are scoped to this run's worktree ({ run_id }). Files/Agent/terminal
  // tabs are fetch-/push-driven — the 1.6s poll never wipes their bodies (§7.2).
  const terminals = terminalTabsController({ run_id: id });
  let shellCtl = null; // tab-row controller (mountTabShell)
  let aux = null; // the mounted files/terminal/agent pane controller

  // Aux tabs own their bodies; the poll only keeps the shell + banner current for
  // them. Conversation and Stages are poll-driven and are not aux tabs.
  // Cluster tabs own their bodies too — the poll must leave them alone.
  const isAuxTab = (tabId) =>
    tabId === "changes" || tabId === "files" || tabId === "agent" || isProjectClusterTab(tabId) || /^term-/.test(tabId);
  const isMultiStage = () => !!(last && last.stages && last.stages.length);
  // A run parked between stages opens on Stages; every other state opens on
  // Changes. A single-stage run never has a Stages tab, so fall back.
  const defaultTab = () => "conversation";
  const staticTabs = () => taskSurfaceTabs({ multiStage: isMultiStage(), terminalTabs: terminals.tabs() });
  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };

  // The tab bar IS the top of the view, and only that: the run's identity lives
  // in the sidebar, and its branch in the panes that act on it.
  const shell = (t) => {
    const m = t || {};
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="tabrow"></div>
      </div>
      <div class="task-error" id="taskError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    showBanner(bannerText(localError, m.last_error));
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (tabId) => selectTab(tabId),
      onClose: (tabId) => closeTerminal(tabId),
      newTabOptions: NEW_TAB_KINDS,
      onNewTab: () => newTerminal(),
      back: { title: m.project ? `Back to ${m.project}` : "Back to project" },
      onBack: () => goHome(),
      ...projectClusterShellOptions({ projectId, selectTab: (tabId) => selectTab(tabId) }),
    });
    // A full shell rebuild (state/goal changed) wiped #tabbody — re-mount an aux
    // tab so the poll's early-return leaves a live pane in place.
    if (isAuxTab(tab)) mountAux(tab);
  };

  // Switch the active tab: Stages repaints through the poll machinery; the aux
  // tabs (Changes/Files/Agent/terminals) mount their own bodies and are never polled.
  const selectTab = (tabId) => {
    tab = tabId;
    replaceTaskHash();
    if (shellCtl) shellCtl.setActive(tabId);
    disposeAux();
    if (tabId === "stages" || tabId === "conversation") {
      const body = $("#tabbody");
      if (body) body.classList.remove("bare", "flush");
      stagesKey = null;
      conversationKey = null;
      paint();
    } else {
      mountAux(tabId);
    }
  };

  const mountAux = (tabId) => {
    disposeAux();
    const body = $("#tabbody");
    if (!body) return;
    // Terminal-ish tabs go edge-to-edge; Changes/Files run flush (rail + detail
    // each scroll internally); the rest keep the body padding.
    body.classList.toggle("bare", tabId === "agent" || /^term-/.test(tabId));
    body.classList.toggle("flush", tabId === "changes" || tabId === "files");
    if (isProjectClusterTab(tabId)) {
      // The project-wide panes behind the tab bar's right cluster: the same
      // Inbox, Issues and Archive every project surface reaches, mounted here.
      aux = mountProjectClusterTab(body, tabId, {
        projectId,
        callRpc: (method, params) => App.call(method, params),
        navigate: go,
      });
      return;
    }
    if (tabId === "agent") {
      // The run addresses its own agent; the bridge resolves that to the
      // worktree the run works in, which is where the agent actually lives.
      // A card names the harness to start on; Restart names none and reruns the
      // one the run already holds.
      aux = mountAgentTab(body, { id }, {
        onStart: (provider) => App.call("agent.start", { id, ...(provider ? { provider } : {}) }),
      });
      return;
    }
    if (tabId === "changes") {
      // The git surface for this run's worktree: the commit rail on the left,
      // the review diff ("All changes", the taskReview plug), staging, or a
      // commit's detail on the right. The pane owns its own 1.6s poll.
      aux = mountGitPane(body, {
        scope: { run_id: id },
        callRpc: (method, params) => App.call(method, params),
        agentCommitOptions: last ? taskAgentCommitOptions(last.state, last.goal) : [],
        review: {
          getBase: () => (last && last.base_branch) || "main",
          mount: (host) => reviewPlug.mount(host),
          unmount: () => reviewPlug.unmount(),
        },
      });
      return;
    }
    aux = mountAuxTab(body, tabId, {
      scope: { run_id: id },
      callRpc: (method, params) => App.call(method, params),
      initialPath: tabId === "files" ? linkedFilePath : null,
      onExit: () => {
        terminals.drop(tabId);
        if (shellCtl) shellCtl.setTabs(staticTabs());
        selectTab(defaultTab());
      },
    });
    if (tabId === "files") linkedFilePath = null;
  };

  const newTerminal = async () => {
    let termId;
    try {
      termId = await terminals.create();
    } catch (e) {
      showBanner("error: " + e.message.slice(0, 80));
      return;
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    selectTab(termId);
  };

  const closeTerminal = async (termId) => {
    try {
      await terminals.close(termId);
    } catch {
      /* the reaper/close raced us — drop the tab regardless */
    }
    if (shellCtl) shellCtl.setTabs(staticTabs());
    if (tab === termId) selectTab(defaultTab());
  };

  let last = null;
  // Latched once run.get reports "unknown run_id": the run was deleted out from
  // under this view. We stop the poll and render a terminal gone-state so a stray
  // tick can never repaint over it.
  let gone = false;
  // Cursor cache for the conversation: each poll sends the last-held sequence
  // so the bridge ships only new items, not the whole thread every 1.6s. The
  // review plug reads the merged thread through getTask() → last.
  const threadCache = createThreadCache();

  // Leaving the run lands on its project page (or notifications when the
  // owning project was never learned).
  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "notifications" });

  // The run is gone (deleted while we were on it): stop the poll, tear down any
  // mounted aux pane, and render a latched terminal state with a way back — the
  // owning project if a prior paint learned it, else notifications.
  const renderGone = () => {
    gone = true;
    if (App.poll) {
      clearInterval(App.poll);
      App.poll = null;
    }
    disposeAux();
    const backLabel = last && last.project_id ? "Back to project" : "Back to notifications";
    root.innerHTML = `<div class="empty gone">This task no longer exists.<div><button class="btn" id="goneback">${backLabel}</button></div></div>`;
    const back = $("#goneback");
    if (back) back.onclick = () => goHome();
  };

  // A mid-run revision belongs to the plan (the doc home never moved): open the
  // owning plan's stage doc so the user comments / sends notes there.
  const openPlan = (stageId) => {
    if (last && last.plan_id) {
      // Mark that we entered the plan from this run so the plan's back chevron
      // returns here (the plan↔run round trip; core/taskActions.planBackTarget).
      sessionStorage.setItem("build.planReturn." + last.plan_id, id);
      go({ name: "plan", projectId: last.project_id || projectId, id: last.plan_id, tab: "stages", stage: stageId });
    }
  };

  // The review surface (the Changes rail's "All changes" entry). ONE instance for
  // the view's whole life, so pending review comments survive tab switches and
  // shell rebuilds; the git pane mounts/unmounts it as the rail selection moves.
  const reviewPlug = createTaskReview({
    taskId: id,
    callRpc: (method, params) => App.call(method, params),
    getTask: () => last,
    isOffline: () => App.offline,
    onMerged: () => goHome(),
  });

  // A local (client-side) RPC failure from Abandon/Delete. The bridge does not set
  // last_error for these, so without holding it here the 1.6s poll would call
  // showBanner(t.last_error) and clear the message within ~0–1.6s — too fast to
  // read. It takes precedence over the polled last_error until the next user action.
  let localError = null;

  // The dismissible error banner (bridge run_view.last_error: merge failure,
  // harness crash). Lives outside the tab body so it survives tab switches; the
  // poll keeps it in sync with the run's current last_error (or a held localError).
  const showBanner = (message) => {
    const el = $("#taskError");
    if (!el) return;
    if (message) {
      el.textContent = message;
      el.hidden = false;
    } else {
      el.textContent = "";
      el.hidden = true;
    }
  };

  // Live-task housekeeping does not belong in the conversation. Once a task is
  // terminal, keep only the small destructive Delete action for clearing its
  // history.
  const wireActions = (el, m) => {
    if (!el) return;
    const action = taskRemovalAction(m);
    if (!action) {
      el.innerHTML = "";
      return;
    }
    el.innerHTML = `<button class="btn danger mini" id="${esc(action.id)}">${esc(action.label)}</button>`;
    const btn = el.querySelector("button");
    btn.onclick = async () => {
      if (!(await confirmAction(deleteRunConfirm()))) return;
      localError = null;
      btn.disabled = true;
      btn.textContent = action.busyLabel;
      try {
        await App.call("run.delete", { run_id: id });
        goHome();
      } catch (e) {
        btn.disabled = false;
        btn.textContent = action.label;
        localError = "error: " + e.message.slice(0, 80);
        showBanner(localError);
      }
    };
  };

  loadModelCatalog(); // warm the selector catalog before the Stages Start control needs it

  // The Stages tab's poll freeze/rebuild key, preserved across ticks.
  let stagesKey = null;
  let conversationKey = null;
  let conversationDraft = "";
  let linkedFilePath = sessionStorage.getItem(`build.fileLink.${id}`);
  if (linkedFilePath) sessionStorage.removeItem(`build.fileLink.${id}`);
  // The owning plan can be deleted once the run is terminal; once plan.stages
  // returns "unknown plan_id" we latch this and stop re-fetching, rendering the
  // board from the run's own progress records alone.
  let planGone = false;

  // Render the run-side Stages tab: join the plan's stage docs (title, doc
  // sub-state, open-comment counts) with the run's execution progress, then hand
  // the board to stages.js. Comments/docs live on the plan, so this fetches
  // plan.stages every tick for the doc metadata and joins it with the run's own
  // stage progress (already in `t.stages`). Frozen while a fix note is in flight.
  async function paintStages(t) {
    // The first stage gate may paint before the catalog warm-up resolves. Wait
    // here so the provider selector never gets frozen on the legacy Claude-only
    // fallback by the view's render-key optimization.
    const modelCatalog = await loadModelCatalog();
    let planStages = null;
    if (!planGone) {
      try {
        planStages = await App.call("plan.stages", { plan_id: t.plan_id });
      } catch (e) {
        // A deleted plan is a permanent condition (the run is terminal): latch it
        // and fall through to the run-only board. Anything else is transient —
        // leave the current board and let the next poll retry.
        if (/unknown plan_id/.test((e && e.message) || "")) planGone = true;
        else return;
      }
    }
    const stagesData = planGone
      ? { stages: runStagesFallback(t.stages), auto_advance: t.auto_advance, planDeleted: true }
      : { stages: joinRunStages(planStages.stages, t.stages), auto_advance: t.auto_advance };
    const key = t.state + " " + JSON.stringify(stagesData);
    const noteBox = $("#fixnote");
    const busy = stageActionBusy() || (noteBox && (noteBox.value.trim() || document.activeElement === noteBox));
    if ($("#stagelist") && (key === stagesKey || busy)) return;
    stagesKey = key;
    renderStagesTab({
      body: $("#tabbody"),
      run: t,
      stagesData,
      catalog: modelCatalog,
      callRpc: (method, params) => App.call(method, params),
      repaint: () => {
        stagesKey = null;
        paint();
      },
      openPlan,
    });
  }

  function openThreadLink(link) {
    if (link.kind === "file" && link.path) {
      linkedFilePath = link.path;
      selectTab("files");
      return;
    }
    const issueId = link.issue_id || link.plan_id;
    if ((link.kind === "issue_stage" || link.kind === "plan_stage") && issueId && link.stage_id) {
      go({
        name: "plan",
        projectId: (last && last.project_id) || projectId,
        id: issueId,
        tab: "stages",
        stage: link.stage_id,
      });
      return;
    }
    const implementationId = link.implementation_id || link.run_id;
    if ((link.kind === "implementation" || link.kind === "run") && implementationId) {
      go({ name: "task", projectId: (last && last.project_id) || projectId, id: implementationId, tab: "conversation" });
    }
  }

  function paintConversation(t) {
    const body = $("#tabbody");
    if (!body) return;
    const key = JSON.stringify({
      state: t.state,
      harness: t.harness || "",
      sessions: t.thread?.sessions || [],
      items: t.thread?.items || [],
      revisions: t.thread?.revisions || [],
    });
    if (key === conversationKey && body.querySelector(".review-thread")) return;
    conversationKey = key;
    body.innerHTML = threadHtml(t.thread, {
      agentLabel: t.harness,
      status: { label: RUN_STATE_LABEL[t.state] || t.state || "", cls: runChipClass(t.state) },
      actionsId: "threadlifecycle",
      composer: !RUN_TERMINAL_STATES.has(t.state) && {
        inputId: "runthreadinput",
        sendId: "runthreadsend",
        hintId: "runthreadhint",
        placeholder: "Send a message to the coding agent…",
      },
    });
    wireActions(body.querySelector("#threadlifecycle"), t);
    wireThreadRevisionLinks(body, (revisionId) =>
      App.call("thread.revision", { entity_id: id, revision_id: revisionId }),
    );
    wireThreadLinks(body, openThreadLink);
    wireThreadComposer(body, {
      ids: { input: "runthreadinput", send: "runthreadsend", hint: "runthreadhint" },
      readDraft: () => conversationDraft,
      writeDraft: (value) => {
        conversationDraft = value;
      },
      onSubmit: (message) => App.call("thread.post", { entity_id: id, body: message }),
      afterSubmit: (view) => {
        last = { ...view, thread: threadCache.absorb(view.thread) };
        conversationKey = null;
        paint();
      },
      onError: (error) => showBanner("error: " + error.message.slice(0, 80)),
    });
  }

  let visitMarkedRead = false; // paint() marks the run read once per visit

  const paint = async () => {
    if (gone || App.offline) return; // latched gone-state / offline freeze: no repaint
    let t;
    try {
      t = await App.call("run.get", { run_id: id, ...threadCache.cursorParam() });
    } catch (e) {
      // A deleted run is permanent: latch the gone-state and stop polling.
      // Every other error is transient — stay silent and let the poll retry.
      if (/unknown run_id/.test((e && e.message) || "")) renderGone();
      return;
    }
    // Fold the cursored conversation delta back into a full thread before
    // `last` (and everything reading it) sees the payload.
    t = { ...t, thread: threadCache.absorb(t.thread) };
    // Visiting the run reads it: mark once, on the first successful fetch.
    if (!visitMarkedRead) {
      visitMarkedRead = true;
      markEntityRead(id);
    }
    // Update `last` BEFORE any shell rebuild: shell() remounts aux tabs (the
    // Changes git pane builds its commit options from last.state/last.goal) and
    // staticTabs() reads last.stages to decide whether a Stages tab exists.
    const needShell =
      !last || last.state !== t.state || last.goal !== t.goal || (last.stages || []).length !== (t.stages || []).length;
    last = t;
    if (t.project_id && t.project_id !== projectId) {
      projectId = t.project_id;
      replaceTaskHash();
    }
    if (needShell) shell(t);
    // A stale task Stages URL on a single-stage run (no Stages tab)
    // falls back to Changes rather than leaving an empty, tab-less body.
    if (tab === "stages" && !isMultiStage()) {
      selectTab("changes");
      return;
    }
    // Keep the error banner in sync even when the state is unchanged — a merge
    // failure leaves the run in review, so the shell won't re-render. A held local
    // RPC error (Abandon/Delete failure) wins over the polled last_error so the
    // poll can't wipe it before the user has read it.
    showBanner(bannerText(localError, t.last_error));
    if (tab === "conversation") {
      paintConversation(t);
      return;
    }
    // Only Conversation and Stages are poll-driven; the aux tabs own their own bodies.
    if (tab !== "stages") return;
    await paintStages(t);
  };
  // Tear down any mounted terminal/agent pane when navigating away.
  App.viewDispose = () => disposeAux();

  await terminals.load();
  shell(null);
  await paint();
  App.poll = setInterval(paint, 1600);
}
