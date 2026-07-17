// The plan cockpit (project-scoped review), keyed by plan_id. Two review
// surfaces share one poll loop:
//   • single-doc plans — rendered markdown (plan.doc) with a select-to-comment
//     notes composer that batches into plan.send_notes;
//   • multi-stage plans — a stage board of docs (views/planStages.js) with
//     per-stage approve, anchored persisted comments, and per-stage send-notes.
// A persistent footer carries the plan-level gates: Approve plan (plan.approve)
// while at review, then the Implement split-button (run.create) once approved —
// disabled with an inline reason until every dispatch precondition holds, and
// replaced by a link to the run once one exists. The header carries lifecycle
// (message a live/parked planning session, abandon, delete). Live-polled on the
// review cadence; in-flight comment/selection state survives ticks via the same
// freeze/rebuild key discipline task.js uses.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { mountSplitButton } from "../core/splitButton.js";
import { mountTabShell } from "../core/tabshell.js";
import { mountAgentPane } from "../core/surfaceTabs.js";
import { terminalManager } from "../terminal/manager.js";
import { assemblePlanNotes } from "../core/notes.js";
import { App, go, loadModelCatalog } from "../app.js";
import { PLAN_STATE_LABEL, PLAN_TERMINAL_STATES, planChipClass, planPayloadFor } from "./shared.js";
import { canImplement, implementBlockReason, planAbandonable, planDeletable, bannerText } from "../core/taskActions.js";
import { openPlanMessage } from "../sheets/message.js";
import { openImplementOptions } from "../sheets/implement.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";
import { renderPlanStages, planStageActionBusy } from "./planStages.js";

// Plan states whose planning session accepts a freeform message (the bridge
// gates the rest): a live drafting session redirects, a parked one resumes.
const MESSAGEABLE = ["drafting", "blocked", "failed", "idle_unreported", "interrupted"];
// States that carry no plan-level gate yet (footer hidden) — nothing to approve
// or implement while the agent is still drafting, and nothing once abandoned.
const NO_GATE = new Set(["created", "drafting", "abandoned"]);

export async function renderPlan() {
  const root = $("#root");
  const id = App.route.id;
  let last = null;
  // The plan carries two surfaces: Review (the doc / stage board) and Agent (the
  // drafting session's live PTY — "the terminal is the basement"). The Agent tab
  // shows only while the plan is non-terminal; Review is the default.
  let tab = App.route.tab === "agent" ? "agent" : "review";
  let tabShellCtl = null;
  let agentPane = null;
  // A locally-held RPC failure (abandon/delete/implement) the bridge does not
  // record in last_error; it wins over the polled value so the poll can't wipe
  // it before the user reads it (same rule as task.js).
  let localError = null;

  // Multi-stage state preserved across the poll: the selected stage (null → the
  // board) and the last-rendered payload key. A stage deep-link (the run's
  // Stages tab routing a mid-run revision here) seeds the opened stage.
  let selectedStageId = App.route.stage || null;
  let stagesKey = null;
  // Single-doc review state preserved across the poll: pending comments, an id
  // counter, the last-rendered key, and the selection watcher's disposer.
  const planComments = [];
  let cid = 0;
  let planKey = null;
  let planSelDispose = null;

  loadModelCatalog(); // warm the catalog before the Implement options need it

  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "board" });

  const showBanner = (message) => {
    const el = $("#planError");
    if (!el) return;
    el.textContent = message || "";
    el.hidden = !message;
  };

  // The Agent tab exists only while a planning session can be live — a terminal
  // (abandoned) plan has no session, so only Review remains.
  const agentAvailable = (p) => !!p && !PLAN_TERMINAL_STATES.has(p.state);

  const disposeAgent = () => {
    if (agentPane) {
      agentPane.dispose();
      agentPane = null;
    }
  };

  // Mount the plan's drafting-session PTY into the body (agent.attach is
  // entity-agnostic — a plan id resolves to its planning session). Mirrors
  // task.js's Agent tab: a quiet idle chip over the retained last screen when no
  // session is live. The poll never repaints the body while this is mounted.
  const mountAgent = () => {
    const body = $("#planbody");
    if (!body) return;
    if (planSelDispose) {
      planSelDispose();
      planSelDispose = null;
    }
    body.innerHTML = `<div class="agentwrap plan-agentwrap"><div class="agent-idle" id="agentIdle" hidden></div><div class="termpane" id="agentpane"></div></div>`;
    const chip = body.querySelector("#agentIdle");
    const setIdle = (on) => {
      if (!chip) return;
      chip.textContent = on ? "no active planning session" : "";
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
      () => setIdle(true), // unknown/absent session — chip alone, view intact
    );
    agentPane = {
      dispose() {
        disposed = true;
        if (pane) pane.dispose();
        terminalManager().detach(`agent:${id}`);
      },
    };
  };

  // Switch surfaces: Review repaints through the poll machinery; Agent owns its
  // own body and is never touched by the poll.
  const selectTab = (next) => {
    tab = next;
    App.route.tab = next;
    history.replaceState(null, "", `#/plan/${encodeURIComponent(id)}/${next}`);
    if (tabShellCtl) tabShellCtl.setActive(next);
    disposeAgent();
    if (next === "agent") {
      mountAgent();
    } else {
      planKey = null;
      stagesKey = null;
      paint();
    }
  };

  // The Review/Agent tab row. Rebuilt with the shell (identity/state changes),
  // so the Agent tab appears/disappears as the plan crosses into a terminal state.
  const wireTabs = (p) => {
    const host = $("#plantabs");
    if (!host) return;
    const tabs = [{ id: "review", label: "Review" }, ...(agentAvailable(p) ? [{ id: "agent", label: "Agent" }] : [])];
    tabShellCtl = mountTabShell(host, { tabs, active: tab, onSelect: (t) => selectTab(t) });
  };

  // The header shell: identity + state chip + lifecycle actions, an error
  // banner, the review body host, and the persistent gate footer. Rebuilt only
  // when the plan's identity/state/run-link changes, so a poll tick never wipes
  // in-flight comment state in the body.
  const shell = (p) => {
    const context = planPayloadFor(p);
    root.innerHTML = `
      <div class="board-head"><div>
          <h1>Plan: ${esc(p.goal)}</h1>
          <p class="mono projmeta">${esc(p.project || "")}${p.base_branch ? ` · ${esc(p.base_branch)}` : ""}</p></div>
        <div class="surface-meta" style="margin-left:auto">
          <span id="planmsgaction"></span>
          <span class="chip ${planChipClass(p.state)}">${PLAN_STATE_LABEL[p.state] || p.state}</span>
          <span class="taskactions" id="planactions"></span>
        </div></div>
      <div class="tabs" id="plantabs"></div>
      <div class="task-error" id="planError" role="alert" hidden></div>
      ${context ? `<div class="payload planctx">${esc(context)}</div>` : ""}
      <div id="planbody"></div>
      <div class="actionbar planfooter" id="planfooter" ${NO_GATE.has(p.state) ? "hidden" : ""}>
        <span class="hint" id="planhint"></span><div class="right" id="gateactions"></div></div>`;
    wireLifecycle(p);
    wireGate(p);
    wireTabs(p);
    showBanner(bannerText(localError, p.last_error));
    // A shell rebuild wiped #planbody — re-mount the Agent pane so the poll's
    // early-return leaves a live pane in place (mirrors task.js).
    if (tab === "agent") mountAgent();
  };

  // Header lifecycle: message a live/parked planning session; abandon a live
  // plan or delete a terminal (abandoned) one.
  const wireLifecycle = (p) => {
    const msgEl = $("#planmsgaction");
    if (msgEl) {
      msgEl.innerHTML = MESSAGEABLE.includes(p.state) ? '<button class="btn mini" id="planmsg">Message agent</button>' : "";
      const b = $("#planmsg");
      if (b) b.onclick = () => openPlanMessage(p, paint);
    }
    const el = $("#planactions");
    if (!el) return;
    if (planDeletable(p.state)) {
      el.innerHTML = '<button class="btn danger mini" id="planremove">Delete</button>';
    } else if (planAbandonable(p.state)) {
      el.innerHTML = '<button class="btn mini" id="planremove">Abandon</button>';
    } else {
      el.innerHTML = "";
      return;
    }
    const remove = $("#planremove");
    remove.onclick = async () => {
      localError = null;
      const deleting = planDeletable(p.state);
      if (!deleting && !window.confirm("Abandon this plan? Its planning worktree is removed; the plan stays as history."))
        return;
      remove.disabled = true;
      remove.textContent = deleting ? "deleting…" : "abandoning…";
      try {
        await App.call(deleting ? "plan.delete" : "plan.abandon", { plan_id: id });
        if (deleting) goHome();
        else paint();
      } catch (e) {
        remove.disabled = false;
        remove.textContent = deleting ? "Delete" : "Abandon";
        localError = "error: " + e.message.slice(0, 80);
        showBanner(localError);
      }
    };
  };

  // The gate footer: the review gate (Approve plan) while at plan_review, then
  // the Implement trigger. A run already implementing the plan replaces
  // Implement with a link to it.
  const wireGate = (p) => {
    if (NO_GATE.has(p.state)) return;
    const actions = $("#gateactions");
    const hint = $("#planhint");
    if (!actions) return;

    // Left: the coarse review gate. plan.approve is accepted throughout
    // plan_review (independent of per-stage approvals); it tears down the
    // planning worktree and unlocks Implement.
    if (p.state === "plan_review") {
      const approve = document.createElement("button");
      approve.className = "btn";
      approve.id = "approveplan";
      approve.textContent = "Approve plan";
      actions.appendChild(approve);
      approve.onclick = async () => {
        localError = null;
        approve.disabled = true;
        approve.textContent = "approving…";
        try {
          await App.call("plan.approve", { plan_id: id });
          planKey = null;
          stagesKey = null;
          paint();
        } catch (e) {
          approve.disabled = false;
          approve.textContent = "Approve plan";
          if (hint) hint.textContent = "error: " + e.message.slice(0, 60);
        }
      };
    }

    // Right: the Implement trigger. A live run → a link to it; ready → the
    // split-button; otherwise a disabled button with the bridge's own reason.
    if (p.active_run_id) {
      const link = document.createElement("button");
      link.className = "btn primary";
      link.id = "viewrun";
      link.textContent = "View run →";
      link.onclick = () => go({ name: "task", id: p.active_run_id, tab: "changes" });
      actions.appendChild(link);
      if (hint) hint.textContent = "A run is implementing this plan.";
      return;
    }
    if (canImplement(p)) {
      const host = document.createElement("span");
      host.className = "splitbtn-host";
      actions.appendChild(host);
      mountSplitButton(host, {
        options: [
          { id: "implement", label: "Implement", busyLabel: "starting…", description: "Create a worktree and run this plan on its own branch." },
          { id: "implement_opts", menuLabel: "Implement with options…", busyLabel: "starting…", description: "Override the base branch, model, or effort for this run." },
        ],
        run: async (optionId) => {
          localError = null;
          let run;
          try {
            run = optionId === "implement"
              ? await App.call("run.create", { plan_id: id })
              : await openImplementOptions(p, App.modelCatalog || { models: [], efforts: [] });
          } catch (e) {
            // A dispatch error (not the options-sheet cancel) surfaces on the
            // hint; either way the split-button restores itself for a retry.
            if (e && e.message && e.message !== "cancelled" && hint) hint.textContent = "error: " + e.message.slice(0, 60);
            throw e;
          }
          go({ name: "task", id: run.run_id, tab: "changes" });
        },
      });
      return;
    }
    const reason = implementBlockReason(p);
    const disabled = document.createElement("button");
    disabled.className = "btn primary";
    disabled.id = "implement";
    disabled.disabled = true;
    disabled.title = reason || "";
    disabled.textContent = "Implement";
    actions.appendChild(disabled);
    if (hint && reason) hint.textContent = reason;
  };

  // ---- Single-doc review body (plan.doc + select-to-comment notes) ----------

  function renderSingleDoc(doc) {
    const editable = last.state === "plan_review";
    planComments.length = 0;
    const body = $("#planbody");
    body.innerHTML = `
      <div class="plan" id="plandoc">${renderMarkdown(doc)}</div>
      ${editable ? `<div class="plan-feedback"><div id="pclist"></div>
        <textarea id="pgeneral" class="plan-general" placeholder="Add a general comment about the plan and request updates…"></textarea>
        <div class="actionbar"><span class="hint" id="phint"></span><div class="right" id="pactions"></div></div></div>` : ""}`;
    if (!editable) return;
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
      const general = $("#pgeneral") ? $("#pgeneral").value.trim() : "";
      if (planComments.length || general) {
        phint.textContent = "Your comments go to the planning agent to revise the plan.";
        pactions.innerHTML = `<button class="btn" id="clearfb">Clear</button><button class="btn primary" id="requestUpdates">Request updates</button>`;
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
            await App.call("plan.send_notes", { plan_id: id, comments: notes });
            planComments.length = 0;
            planKey = null;
            hideCommentPop();
            paint();
          } catch (e) {
            btn.disabled = false;
            btn.textContent = "Request updates";
            phint.textContent = "error: " + e.message.slice(0, 50);
          }
        };
      } else {
        phint.textContent = "Select text in the plan to comment, or approve the plan below.";
        pactions.innerHTML = "";
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
    const planEl = $("#plandoc");
    if (planSelDispose) planSelDispose();
    planSelDispose = watchSelection(planEl, (sel) => {
      const text = sel.toString().trim();
      const range = sel.getRangeAt(0).cloneRange();
      showCommentPop(range.getBoundingClientRect(), (comment) => addComment(text, comment, range));
    });
    $("#pgeneral").oninput = updateActions;
    refreshFeedback();
  }

  // ---- Multi-stage review body (plan.stages + plan.stage_doc) ---------------

  async function paintStages(p) {
    let stagesData;
    try {
      stagesData = await App.call("plan.stages", { plan_id: id });
    } catch {
      return; // not readable yet; the poll retries
    }
    let stageDoc = null;
    if (selectedStageId) {
      try {
        stageDoc = await App.call("plan.stage_doc", { plan_id: id, stage_id: selectedStageId });
      } catch {
        /* doc not available yet — the view shows a loading placeholder */
      }
    }
    const key = p.state + " " + JSON.stringify(stagesData) + " " + selectedStageId + " " + (stageDoc ? stageDoc.contents.length : 0);
    const noteBox = $("#stage-general");
    const busy = hasCommentPop() || planStageActionBusy() || (noteBox && (noteBox.value.trim() || document.activeElement === noteBox));
    const rendered = $("#stagelist") || $("#stagedoc");
    if (rendered && (key === stagesKey || busy)) return;
    stagesKey = key;
    renderPlanStages({
      body: $("#planbody"),
      plan: p,
      stagesData,
      stageDoc,
      selectedStageId,
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

  const paint = async () => {
    if (App.offline) return;
    let p;
    try {
      p = await App.call("plan.get", { plan_id: id });
    } catch {
      return; // not readable yet — the poll retries
    }
    // A plan that just crossed into a terminal state loses its Agent tab; fall
    // back to Review before the shell rebuild so the surface stays consistent.
    if (tab === "agent" && !agentAvailable(p)) {
      selectTab("review");
      return;
    }
    const needShell = !last || last.state !== p.state || last.goal !== p.goal || last.active_run_id !== p.active_run_id;
    last = p;
    if (needShell) shell(p);
    showBanner(bannerText(localError, p.last_error));

    // The Agent pane owns the body; the poll never repaints it.
    if (tab === "agent") return;

    const body = $("#planbody");
    if (!body) return;
    // Multi-stage plans (a non-empty manifest) drive the stage board; single
    // plans render one doc. A plan's shape is fixed at drafting, so this never
    // flip-flops within a plan's life.
    if (p.stages && p.stages.length) {
      await paintStages(p);
      return;
    }
    if (p.state === "created" || p.state === "drafting") {
      body.innerHTML = '<div class="plan plan-loading">✦ planning agent is drafting the plan…</div>';
      planKey = "drafting";
      planComments.length = 0;
      return;
    }
    let doc = "";
    try {
      doc = (await App.call("plan.doc", { plan_id: id })).contents;
    } catch {
      /* plan doc not readable yet */
    }
    const key = p.state + " " + doc;
    if (planKey === key && $("#plandoc")) return; // nothing changed — keep comments/selection
    planKey = key;
    renderSingleDoc(doc);
  };

  App.viewDispose = () => {
    if (planSelDispose) planSelDispose();
    disposeAgent();
  };

  await paint();
  App.poll = setInterval(paint, 1600);
}
