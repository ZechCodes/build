// The run view (a run is a "Task" in the UI): the worktree-scoped surface for
// implementing a plan. Tabs are Stages (multi-stage runs only), Changes (the
// review diff + request-changes + the merge/git plug), Files, Agent, and one per
// open terminal. The plan doc left the run entirely — a compact reference header
// links back to the owning plan (quick runs show their goal). Live-polled every
// 1.6s; the aux tabs (Changes/Files/Agent/terminals) own their own bodies and are
// never repainted by the poll.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { mountSplitButton } from "../core/splitButton.js";
import { App, go, loadModelCatalog, markEntityRead } from "../app.js";
import { RUN_STATE_LABEL, runChipClass } from "./shared.js";
import { canDelete, canAbandon, bannerText, defaultRunTab } from "../core/taskActions.js";
import { openMessageAgent } from "../sheets/message.js";
import { renderStagesTab, stageActionBusy, joinRunStages, runStagesFallback } from "./stages.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab, mountAgentPane } from "../core/surfaceTabs.js";
import { mountGitPane, taskAgentCommitOptions } from "../core/gitPane.js";
import { createTaskReview } from "./taskReview.js";
import { terminalManager } from "../terminal/manager.js";

export async function renderTask() {
  const root = $("#root");
  const id = App.route.id; // a run_id (the run route keeps the #/task grammar)
  let tab = App.route.tab || "changes";

  // Terminals are scoped to this run's worktree ({ run_id }). Files/Agent/terminal
  // tabs are fetch-/push-driven — the 1.6s poll never wipes their bodies (§7.2).
  const terminals = terminalTabsController({ run_id: id });
  let shellCtl = null; // tab-row controller (mountTabShell)
  let aux = null; // the mounted files/terminal/agent pane controller

  // Aux tabs own their bodies; the poll only keeps the shell + banner current for
  // them. Stages is poll-driven (like the old Plan tab) and is NOT an aux tab.
  const isAuxTab = (tabId) => tabId === "changes" || tabId === "files" || tabId === "agent" || /^term-/.test(tabId);
  const isMultiStage = () => !!(last && last.stages && last.stages.length);
  // A run parked between stages opens on Stages; every other state opens on
  // Changes. A single-stage/quick run never has a Stages tab, so fall back.
  const defaultTab = () => (isMultiStage() ? defaultRunTab(last) : "changes");
  const staticTabs = () => [
    ...(isMultiStage() ? [{ id: "stages", label: "Stages" }] : []),
    { id: "changes", label: "Changes" },
    { id: "files", label: "Files" },
    { id: "agent", label: "Agent" },
    ...terminals.tabs(),
  ];
  const disposeAux = () => {
    if (aux) {
      aux.dispose();
      aux = null;
    }
  };

  // The tab bar IS the top of the view; the run's identity lives in the sidebar.
  // The bar carries a compact plan reference (link back to the plan, or the goal
  // for a quick run), the state chip, and the header actions.
  const shell = (t) => {
    const m = t || {};
    const planRef = m.plan_id
      ? `<a class="plan-ref" id="planref" title="Open the plan">Plan: ${esc(m.goal || "")} →</a>`
      : `<span class="plan-ref quick">${esc(m.goal || "")}</span>`;
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="tabrow"></div>
        ${planRef}
        <div class="surface-meta">
          <span id="msgaction"></span>
          <span class="chip ${runChipClass(m.state)}" title="${esc(m.goal || "")}">${RUN_STATE_LABEL[m.state] || m.state || ""}</span>
          <span class="taskactions" id="taskactions"></span>
        </div>
      </div>
      <div class="task-error" id="taskError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    const planLink = $("#planref");
    if (planLink && m.plan_id) planLink.onclick = () => go({ name: "plan", id: m.plan_id, tab: "review" });
    wireActions(m);
    showBanner(bannerText(localError, m.last_error));
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (tabId) => selectTab(tabId),
      onClose: (tabId) => closeTerminal(tabId),
      onNewTerminal: () => newTerminal(),
      back: { title: m.project ? `Back to ${m.project}` : "Back to project" },
      onBack: () => goHome(),
    });
    // A full shell rebuild (state/goal changed) wiped #tabbody — re-mount an aux
    // tab so the poll's early-return leaves a live pane in place.
    if (isAuxTab(tab)) mountAux(tab);
  };

  // Switch the active tab: Stages repaints through the poll machinery; the aux
  // tabs (Changes/Files/Agent/terminals) mount their own bodies and are never polled.
  const selectTab = (tabId) => {
    tab = tabId;
    App.route.tab = tabId;
    history.replaceState(null, "", `#/task/${encodeURIComponent(id)}/${tabId}`);
    if (shellCtl) shellCtl.setActive(tabId);
    disposeAux();
    if (tabId === "stages") {
      const body = $("#tabbody");
      if (body) body.classList.remove("bare", "flush");
      stagesKey = null;
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
    if (tabId === "agent") {
      aux = mountAgentTab(body);
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
      onExit: () => {
        terminals.drop(tabId);
        if (shellCtl) shellCtl.setTabs(staticTabs());
        selectTab(defaultTab());
      },
    });
  };

  // The Agent tab: the live agent PTY (a full terminal on the user's machine —
  // input allowed). A dead/absent session shows a quiet "no active agent session"
  // chip over the retained last screen; an unknown run shows the chip alone.
  const mountAgentTab = (body) => {
    body.innerHTML = `<div class="agentwrap"><div class="agent-idle" id="agentIdle" hidden></div><div class="termpane" id="agentpane"></div></div>`;
    const chip = body.querySelector("#agentIdle");
    const setIdle = (on) => {
      if (!chip) return;
      chip.textContent = on ? "no active agent session" : "";
      chip.hidden = !on;
    };
    let pane = null;
    let disposed = false;
    mountAgentPane(body.querySelector("#agentpane"), id, {
      onLive: (live) => setIdle(!live),
      onExit: (reason) => {
        if (reason === "agent_session_ended") setIdle(true);
      },
    }).then(
      (p) => (disposed ? p.dispose() : (pane = p)),
      () => setIdle(true), // unknown run or attach failure — chip alone, view intact
    );
    return {
      dispose() {
        disposed = true;
        if (pane) pane.dispose();
        terminalManager().detach(`agent:${id}`);
      },
    };
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

  // Leaving the run lands on its project page (or notifications when the
  // owning project was never learned).
  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "notifications" });

  // A mid-run revision belongs to the plan (the doc home never moved): open the
  // owning plan's stage doc so the user comments / sends notes there.
  const openPlan = (stageId) => {
    if (last && last.plan_id) go({ name: "plan", id: last.plan_id, tab: "review", stage: stageId });
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

  // Removal actions, mapped to the bridge RPCs by the run's state: Delete
  // (run.delete) for a terminal run; for a live run an Abandon (run.abandon)
  // button — and for a live *adopted* run a split button whose default is the
  // non-destructive Release (run.release, keeps the user's files).
  const wireActions = (m) => {
    const el = $("#taskactions");
    if (!el) return;
    const state = m && m.state;
    // Freeform channel to the agent: live sessions redirect, parked ones resume.
    // Gates keep their structured verbs, so no button there.
    const msgEl = $("#msgaction");
    if (msgEl) {
      const messageable = ["building", "blocked", "failed", "idle_unreported", "interrupted"];
      msgEl.innerHTML = messageable.includes(state)
        ? '<button class="btn mini" id="msgagent">Message agent</button>'
        : "";
      const msgBtn = $("#msgagent");
      if (msgBtn) msgBtn.onclick = () => openMessageAgent(m, paint);
    }
    if (canDelete(state)) {
      el.innerHTML = `<button class="btn danger mini" id="deleteTask">Delete</button>`;
      $("#deleteTask").onclick = async () => {
        localError = null; // a fresh action clears any stale local error
        const btn = $("#deleteTask");
        btn.disabled = true;
        btn.textContent = "deleting…";
        try {
          await App.call("run.delete", { run_id: id });
          goHome();
        } catch (e) {
          btn.disabled = false;
          btn.textContent = "Delete";
          localError = "error: " + e.message.slice(0, 80);
          showBanner(localError);
        }
      };
      return;
    }
    if (!canAbandon(state)) {
      el.innerHTML = "";
      return;
    }
    const options = m.adopted
      ? [
          { id: "release", label: "Release", description: "un-adopt: drop the task, keep the worktree, branch, and all files", busyLabel: "releasing…" },
          { id: "abandon_delete", label: "Abandon & delete", description: "delete the worktree and branch; the task stays as history", busyLabel: "abandoning…", danger: true },
        ]
      : [{ id: "abandon_delete", label: "Abandon", description: "delete the worktree and branch; the task stays as history", busyLabel: "abandoning…" }];
    const run = async (optionId) => {
      localError = null; // a fresh action clears any stale local error
      if (optionId === "release") {
        try {
          await App.call("run.release", { run_id: id });
          goHome();
        } catch (e) {
          localError = "error: " + e.message.slice(0, 80);
          showBanner(localError);
          throw e;
        }
        return;
      }
      const confirmText = m.adopted
        ? "Delete this adopted worktree and its branch? This removes files Build did not create. The task stays as history."
        : "Abandon this task? Its worktree and branch are removed; the task stays as history.";
      if (!window.confirm(confirmText)) throw new Error("cancelled");
      try {
        await App.call("run.abandon", { run_id: id });
        paint();
      } catch (e) {
        localError = "error: " + e.message.slice(0, 80);
        showBanner(localError);
        throw e;
      }
    };
    mountSplitButton(el, { options, run });
  };

  loadModelCatalog(); // warm the selector catalog before the Stages Start control needs it

  // The Stages tab's poll freeze/rebuild key, preserved across ticks.
  let stagesKey = null;
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
      catalog: App.modelCatalog || { models: [], efforts: [] },
      callRpc: (method, params) => App.call(method, params),
      repaint: () => {
        stagesKey = null;
        paint();
      },
      openPlan,
    });
  }

  let visitMarkedRead = false; // paint() marks the run read once per visit

  const paint = async () => {
    if (App.offline) return; // freeze the view; resume() restarts the flow
    let t;
    try {
      t = await App.call("run.get", { run_id: id });
    } catch {
      return;
    }
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
    if (needShell) shell(t);
    // A stale #/task/<id>/stages URL on a single-stage/quick run (no Stages tab)
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
    // Only the Stages tab is poll-driven; the aux tabs own their own bodies.
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
