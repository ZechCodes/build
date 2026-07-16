// The task view: plan tab (select-to-comment review) + the Changes tab, where
// the review diff lives as the commit rail's "All changes" entry (taskReview
// plug inside the git pane). Live-polled every 1.6s.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { mountSplitButton } from "../core/splitButton.js";
import { assemblePlanNotes } from "../core/notes.js";
import { App, go, loadModelCatalog } from "../app.js";
import { modelOptionsHtml, effortOptionsHtml, effortSupported, modelParams } from "../core/modelPicker.js";
import { STATE_LABEL, chipClass } from "./shared.js";
import { canDelete, canAbandon, bannerText } from "../core/taskActions.js";
import { openMessageAgent } from "../sheets/message.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";
import { renderStagesTab, stageActionBusy } from "./stages.js";
import { mountTabShell } from "../core/tabshell.js";
import { terminalTabsController, mountAuxTab, mountAgentPane } from "../core/surfaceTabs.js";
import { mountGitPane, taskAgentCommitOptions } from "../core/gitPane.js";
import { createTaskReview } from "./taskReview.js";
import { terminalManager } from "../terminal/manager.js";

export async function renderTask() {
  const root = $("#root");
  const id = App.route.id;
  let tab = App.route.tab || "plan";

  // The unified tab shell: Plan + Diff (existing work tabs, content untouched),
  // Files, Agent, then one tab per open user terminal, then `+`. Terminals are
  // scoped to this task's worktree ({task_id}). Files/Agent/terminal tabs are
  // fetch-/push-driven — the 1.6 s poll never wipes their bodies (§7.2).
  const terminals = terminalTabsController({ task_id: id });
  let shellCtl = null; // tab-row controller (mountTabShell)
  let aux = null; // the mounted files/terminal/agent pane controller

  const isAuxTab = (tabId) => tabId === "changes" || tabId === "files" || tabId === "agent" || /^term-/.test(tabId);
  // Plan-side states open on the plan; archived opens there too — the preserved
  // plan docs are all that remains of a worktree the user deleted.
  const PLAN_FIRST_STATES = ["created", "planning", "plan_review", "archived"];
  const defaultTab = () => (last && PLAN_FIRST_STATES.includes(last.state) ? "plan" : "changes");
  const staticTabs = () => [
    { id: "plan", label: "Plan" },
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

  // The tab bar IS the top of the view; the task's identity lives in the
  // sidebar. The bar's right cluster keeps the state chip (goal on hover) and
  // the header actions.
  const shell = (t) => {
    const m = t || {};
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="tabrow"></div>
        <div class="surface-meta">
          <span id="msgaction"></span>
          <span class="chip ${chipClass(m.state)}" title="${esc(m.goal || "")}">${STATE_LABEL[m.state] || m.state || ""}</span>
          <span class="taskactions" id="taskactions"></span>
        </div>
      </div>
      <div class="task-error" id="taskError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    wireActions(m);
    showBanner(bannerText(localError, m.last_error));
    shellCtl = mountTabShell($("#tabrow"), {
      tabs: staticTabs(),
      active: tab,
      onSelect: (tabId) => selectTab(tabId),
      onClose: (tabId) => closeTerminal(tabId),
      onNewTerminal: () => newTerminal(),
    });
    // A full shell rebuild (state/goal changed) wiped #tabbody — re-mount an aux
    // tab so the poll's early-return leaves a live pane in place.
    if (isAuxTab(tab)) mountAux(tab);
  };

  // Switch the active tab: plan/diff repaint through the poll machinery; the
  // aux tabs (files/agent/terminals) mount their own bodies and are never polled.
  const selectTab = (tabId) => {
    tab = tabId;
    App.route.tab = tabId;
    history.replaceState(null, "", `#/task/${encodeURIComponent(id)}/${tabId}`);
    if (shellCtl) shellCtl.setActive(tabId);
    disposeAux();
    if (tabId === "plan") {
      const body = $("#tabbody");
      if (body) body.classList.remove("bare");
      planKey = null;
      paint();
    } else {
      mountAux(tabId);
    }
  };

  const mountAux = (tabId) => {
    disposeAux();
    const body = $("#tabbody");
    if (!body) return;
    // Terminal-ish tabs go edge-to-edge; content tabs keep the body padding.
    body.classList.toggle("bare", tabId === "agent" || /^term-/.test(tabId));
    if (tabId === "agent") {
      aux = mountAgentTab(body);
      return;
    }
    if (tabId === "changes") {
      // The git surface for this task's worktree: the commit rail on the left,
      // and the review diff ("All changes", the taskReview plug), staging, or
      // a commit's detail on the right. The pane owns its own 1.6s poll; the
      // task poll never repaints aux tabs, so the two never double up. A state
      // change rebuilds the shell, which remounts this pane with options
      // matching the new state.
      aux = mountGitPane(body, {
        scope: { task_id: id },
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
      scope: { task_id: id },
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
  // chip over the retained last screen; an unknown task shows the chip alone. The
  // tab never breaks the rest of the view.
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
      () => setIdle(true), // unknown task or attach failure — chip alone, view intact
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

  // Leaving the task lands on its project page (nothing links to the board).
  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "board" });

  // The review surface (the Changes rail's "All changes" entry). ONE instance
  // for the view's whole life, so pending review comments survive tab switches
  // and shell rebuilds; the git pane mounts/unmounts it as the rail selection
  // moves.
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

  // The dismissible error banner (bridge task_view.last_error: merge failure,
  // harness crash). Lives outside the tab body so it survives tab switches; the
  // poll keeps it in sync with the task's current last_error (or a held localError).
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

  // Removal actions, mapped to the bridge RPCs by the task's state: Delete
  // (task.delete) for a terminal task; for a live task an Abandon (task.abandon)
  // button — and for a live *adopted* task a split button whose default is the
  // non-destructive Release (task.release, keeps the user's files).
  const wireActions = (m) => {
    const el = $("#taskactions");
    if (!el) return;
    const state = m && m.state;
    // Freeform channel to the agent: live sessions redirect, parked ones
    // resume. Gates keep their structured verbs, so no button there.
    const msgEl = $("#msgaction");
    if (msgEl) {
      const messageable = ["planning", "building", "blocked", "failed", "idle_unreported", "interrupted"];
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
          await App.call("task.delete", { task_id: id });
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
          await App.call("task.release", { task_id: id });
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
        await App.call("task.abandon", { task_id: id });
        paint();
      } catch (e) {
        localError = "error: " + e.message.slice(0, 80);
        showBanner(localError);
        throw e;
      }
    };
    mountSplitButton(el, { options, run });
  };

  loadModelCatalog(); // warm the selector catalog before plan_review needs it
  // Selection watchers are document-level; dispose the previous render's before
  // wiring new ones or the 1.6s poll accumulates listeners.
  let planSelDispose = null;
  // Plan-review feedback state, preserved across the 1.6s poll.
  const planComments = []; // { id, snippet, comment }
  let cid = 0,
    planKey = null;

  // Multi-stage plan tab state (task.stages / task.stage_doc), preserved across
  // the poll. `selectedStageId` null → the stage board; set → that stage's doc.
  let stagesKey = null,
    selectedStageId = null;

  // Render the multi-stage plan tab: the stage board, or one stage's doc + its
  // persisted comments. Comments and stage docs are server state, so this fetches
  // task.stages every poll (for comment bodies) and task.stage_doc for the open
  // stage, then rebuilds only when the payload changed and the user is not
  // mid-comment — the same freeze discipline as the diff tab.
  async function paintStages(t) {
    let stagesData;
    try {
      stagesData = await App.call("task.stages", { task_id: id });
    } catch {
      return; // not readable yet; the next poll retries
    }
    let stageDoc = null;
    if (selectedStageId) {
      try {
        stageDoc = await App.call("task.stage_doc", { task_id: id, stage_id: selectedStageId });
      } catch {
        /* doc not available yet — the view shows a loading placeholder */
      }
    }
    const key = t.state + " " + JSON.stringify(stagesData) + " " + selectedStageId + " " + (stageDoc ? stageDoc.contents.length : 0);
    const noteBox = $("#stage-general") || $("#fixnote");
    const busy = hasCommentPop() || stageActionBusy() || (noteBox && (noteBox.value.trim() || document.activeElement === noteBox));
    const rendered = $("#stagelist") || $("#stagedoc");
    if (rendered && (key === stagesKey || busy)) return;
    stagesKey = key;
    renderStagesTab({
      body: $("#tabbody"),
      task: t,
      stagesData,
      stageDoc,
      selectedStageId,
      catalog: App.modelCatalog || { models: [], efforts: [] },
      callRpc: (method, params) => App.call(method, params),
      repaint: () => {
        stagesKey = null;
        paint();
      },
      onSelectStage: (stageId) => {
        selectedStageId = stageId;
        stagesKey = null;
        hideCommentPop();
        paint();
      },
    });
  }

  // Build the plan tab: rendered markdown + select-to-comment + a general
  // comment box + an action button that morphs to "Request Updates".
  function renderPlanTab(t, plan) {
    $("#tabbody").onclick = null; // drop the diff tab's tap-to-comment handler
    planComments.length = 0; // a freshly (re)rendered plan starts with no comments
    const editable = t.state === "plan_review";
    const body = $("#tabbody");
    body.innerHTML = `
      <div class="plan" id="planbody">${renderMarkdown(plan)}</div>
      ${editable ? `<div class="plan-feedback"><div id="pclist"></div>
        <textarea id="pgeneral" class="plan-general" placeholder="Add a general comment about the plan and request updates…"></textarea></div>` : ""}
      <div class="actionbar"><span class="hint" id="phint"></span><div class="right" id="pactions"></div></div>`;
    const pclist = $("#pclist"),
      pactions = $("#pactions"),
      phint = $("#phint");

    const removeComment = (idc) => {
      const i = planComments.findIndex((c) => c.id === idc);
      if (i >= 0) planComments.splice(i, 1);
      const mark = document.querySelector(`mark.phl[data-cid="${idc}"]`);
      if (mark) {
        const parent = mark.parentNode;
        while (mark.firstChild) parent.insertBefore(mark.firstChild, mark);
        parent.removeChild(mark);
        parent.normalize();
      }
      refreshFeedback();
    };
    const updateActions = () => {
      const general = editable && $("#pgeneral") ? $("#pgeneral").value.trim() : "";
      if (!editable) {
        pactions.innerHTML = "";
        phint.textContent = "";
        return;
      }
      if (planComments.length || general) {
        phint.textContent = "Your comments will be sent to the planning agent to revise the plan.";
        pactions.innerHTML = `<button class="btn" id="clearfb">Clear</button><button class="btn primary" id="requestUpdates">Request Updates</button>`;
        $("#clearfb").onclick = () => {
          planComments.slice().forEach((c) => removeComment(c.id));
          if ($("#pgeneral")) $("#pgeneral").value = "";
          refreshFeedback();
        };
        $("#requestUpdates").onclick = async () => {
          const btn = $("#requestUpdates");
          btn.disabled = true;
          btn.textContent = "requesting updates…";
          const notes = assemblePlanNotes(planComments, $("#pgeneral") ? $("#pgeneral").value : "");
          try {
            await App.call("task.send_notes", { task_id: id, comments: notes });
            planComments.length = 0;
            planKey = null;
            hideCommentPop();
            paint();
          } catch (e) {
            btn.disabled = false;
            btn.textContent = "Request Updates";
            phint.textContent = "error: " + e.message.slice(0, 50);
          }
        };
      } else {
        phint.textContent = "Select text in the plan to comment, or approve to start the build.";
        // The coding agent's model defaults to the task's dispatch-time choice;
        // picking here overrides it for the build (and later revisions).
        const catalog = App.modelCatalog || { models: [], efforts: [] };
        pactions.innerHTML = `
          <select id="apModel" class="mini" title="Coding agent model">${modelOptionsHtml(catalog.models, t.model)}</select>
          <select id="apEffort" class="mini" title="Reasoning effort">${effortOptionsHtml(catalog.efforts, t.effort)}</select>
          <button class="btn primary" id="approvePlan">Approve plan &amp; start build</button>`;
        const syncEffort = () => {
          const supported = effortSupported(catalog.models, $("#apModel").value);
          $("#apEffort").disabled = !supported;
          if (!supported) $("#apEffort").value = "";
        };
        $("#apModel").onchange = syncEffort;
        syncEffort();
        $("#approvePlan").onclick = async () => {
          const approve = $("#approvePlan");
          approve.disabled = true;
          approve.textContent = "starting build…";
          const params = modelParams(catalog.models, $("#apModel").value, $("#apEffort").value);
          try {
            await App.call("task.approve_plan", { task_id: id, ...params });
          } catch (e) {
            // Restore the button — the poll's key-diffing skips repaints when
            // nothing changed, so a wedged button would otherwise stay dead.
            approve.disabled = false;
            approve.textContent = "Approve plan & start build";
            phint.textContent = "error: " + e.message.slice(0, 50);
            return;
          }
          planKey = null;
          selectTab("changes");
        };
      }
    };
    function refreshFeedback() {
      if (pclist) {
        pclist.innerHTML = planComments
          .map(
            (c) => `
          <div class="pcomment"><span class="pcx" data-id="${c.id}">×</span>
            <span class="psnip">${esc(c.snippet.replace(/\s+/g, " ").trim().slice(0, 160))}</span>
            <span class="pctext">${esc(c.comment)}</span></div>`,
          )
          .join("");
        pclist.querySelectorAll(".pcx").forEach((x) => (x.onclick = () => removeComment(+x.dataset.id)));
      }
      updateActions();
    }
    const addComment = (snippet, comment, range) => {
      const idc = ++cid;
      planComments.push({ id: idc, snippet, comment });
      try {
        const mark = document.createElement("mark");
        mark.className = "phl";
        mark.dataset.cid = idc;
        range.surroundContents(mark);
      } catch {
        /* selection spanned nodes — keep the comment without the highlight */
      }
      window.getSelection().removeAllRanges();
      refreshFeedback();
    };
    if (editable) {
      const planEl = $("#planbody");
      if (planSelDispose) planSelDispose();
      planSelDispose = watchSelection(planEl, (sel) => {
        const text = sel.toString().trim();
        const range = sel.getRangeAt(0).cloneRange();
        showCommentPop(range.getBoundingClientRect(), (comment) => addComment(text, comment, range));
      });
      $("#pgeneral").oninput = updateActions;
    }
    refreshFeedback();
  }

  const paint = async () => {
    if (App.offline) return; // freeze the view; resume() restarts the flow
    let t;
    try {
      t = await App.call("task.get", { task_id: id });
    } catch {
      return;
    }
    // Update `last` BEFORE any shell rebuild: shell() remounts aux tabs (the
    // Changes git pane builds its commit options from last.state/last.goal).
    const needShell = !last || last.state !== t.state || last.goal !== t.goal;
    last = t;
    if (needShell) shell(t);
    // Keep the error banner in sync even when the state is unchanged — a merge
    // failure leaves the task in review, so the shell won't re-render. A held
    // local RPC error (Abandon/Delete failure) wins over the polled last_error so
    // the poll can't wipe it before the user has read it.
    showBanner(bannerText(localError, t.last_error));
    // Changes, Files, Agent, and terminal tabs are fetch-/push-driven and own
    // their own bodies — the poll only keeps the shell + banner current for
    // them (§7.2). The review diff polls inside its taskReview plug.
    if (tab !== "plan") return;
    const body = $("#tabbody");
    // Multi-stage task (stages non-empty) → the stage board flow. Legacy
    // single-plan tasks (stages empty) keep the original single-doc flow below.
    if (t.stages && t.stages.length) {
      await paintStages(t);
      return;
    }
    if (t.state === "planning" || t.state === "created") {
      body.innerHTML = '<div class="plan plan-loading">✦ planning agent is drafting the plan…</div>';
      planKey = "drafting";
      planComments.length = 0;
      return;
    }
    let plan = "";
    try {
      plan = (await App.call("task.plan", { task_id: id })).contents;
    } catch {
      /* plan not readable yet */
    }
    const key = t.state + " " + plan;
    // Skip rebuild when nothing changed, so comments / typed text / selection survive the poll.
    if (planKey === key && $("#planbody")) return;
    planKey = key;
    renderPlanTab(t, plan);
  };
  // Tear down any mounted terminal/agent pane when navigating away.
  App.viewDispose = () => disposeAux();

  await terminals.load();
  shell(null);
  await paint();
  App.poll = setInterval(paint, 1600);
}
