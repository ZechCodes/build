// The plan cockpit (project-scoped review), keyed by plan_id — the plan-side twin
// of the run surface (views/task.js). The tab bar IS the top of the view (Review |
// Agent, the same shell run/worktree use); a compact bar carries the single-line
// goal, the state chip, and the lifecycle + gate actions (Message, Approve plan,
// Implement, Abandon/Delete). The Review body renders the plan's summary (markdown,
// clamped), then either a single doc (plan.doc, select-to-comment notes) or a
// multi-stage board (views/planStages.js). "The terminal is the basement": the
// Agent tab (the drafting session's PTY) shows only while the plan is non-terminal.
// Live-polled on the review cadence; in-flight comment/selection state survives
// ticks via the same freeze/rebuild key discipline task.js uses.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { mountSplitButton } from "../core/splitButton.js";
import { mountTabShell } from "../core/tabshell.js";
import { mountAgentPane } from "../core/surfaceTabs.js";
import { terminalManager } from "../terminal/manager.js";
import { assemblePlanNotes } from "../core/notes.js";
import { App, go, loadModelCatalog } from "../app.js";
import { PLAN_STATE_LABEL, PLAN_TERMINAL_STATES, planChipClass } from "./shared.js";
import {
  canImplement,
  implementBlockReason,
  planAbandonable,
  planDeletable,
  bannerText,
  shouldFetchPlanDoc,
  planDocPaneState,
} from "../core/taskActions.js";
import { openPlanMessage } from "../sheets/message.js";
import { openImplementOptions } from "../sheets/implement.js";
import { showCommentPop, hideCommentPop, hasCommentPop } from "../commentPop.js";
import { watchSelection } from "../selectWatch.js";
import { renderPlanStages, planStageActionBusy, DOCS_UNAVAILABLE } from "./planStages.js";

// Plan states whose planning session accepts a freeform message (the bridge
// gates the rest): a live drafting session redirects, a parked one resumes.
const MESSAGEABLE = ["drafting", "blocked", "failed", "idle_unreported", "interrupted"];
// States that carry no plan-level gate yet (no Approve/Implement) — nothing to
// approve or implement while the agent is still drafting, and nothing once abandoned.
const NO_GATE = new Set(["created", "drafting", "abandoned"]);

export async function renderPlan() {
  const root = $("#root");
  const id = App.route.id;
  let last = null;
  // The plan carries two surfaces: Review (the doc / stage board) and Agent (the
  // drafting session's live PTY). The Agent tab shows only while the plan is
  // non-terminal; Review is the default.
  let tab = App.route.tab === "agent" ? "agent" : "review";
  let tabShellCtl = null;
  let agentPane = null;
  // A locally-held RPC failure (abandon/delete/implement/approve) the bridge does
  // not record in last_error; it wins over the polled value so the poll can't wipe
  // it before the user reads it (same rule as task.js).
  let localError = null;

  // Multi-stage state preserved across the poll: the selected stage (null → the
  // board) and the last-rendered payload key. A stage deep-link (the run's Stages
  // tab routing a mid-run revision here) seeds the opened stage.
  let selectedStageId = App.route.stage || null;
  let stagesKey = null;
  // Single-doc review state preserved across the poll: pending comments, an id
  // counter, the last-rendered key, and the selection watcher's disposer.
  const planComments = [];
  let cid = 0;
  let planKey = null;
  let planSelDispose = null;
  // The summary block's freeze key — re-rendered only when the summary text
  // changes, so the expand/collapse toggle survives ticks.
  let summaryKey = null;
  // Doc-read error latches: any plan.doc / plan.stage_doc ERROR renders an error
  // state and stops that doc's refetch until the user re-navigates (mirrors the
  // planGone latch in task.js). singleDocError is the single-doc latch; stageDocError
  // holds the latched stage ids.
  let singleDocError = false;
  const stageDocError = new Set();

  loadModelCatalog(); // warm the catalog before the Implement options need it

  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "notifications" });

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

  // The Review body skeleton: a compact project·branch meta line, the summary
  // block, then the doc/stage host (#planbody — the freeze/rebuild target that
  // carries #plandoc / #stagelist / #stagedoc). Mounted once per shell rebuild or
  // tab switch; the poll refreshes meta/summary and repaints #planbody in place.
  const mountReviewSkeleton = () => {
    const body = $("#tabbody");
    if (!body) return;
    if (planSelDispose) {
      planSelDispose();
      planSelDispose = null;
    }
    body.classList.remove("bare");
    body.innerHTML = `
      <p class="mono projmeta" id="planmeta"></p>
      <div class="plan-summary" id="plansummary" hidden></div>
      <div id="planbody"></div>`;
    summaryKey = null; // force the summary to repaint into the fresh skeleton
  };

  const updateMeta = (p) => {
    const el = $("#planmeta");
    if (el) el.textContent = (p.project || "") + (p.base_branch ? ` · ${p.base_branch}` : "");
  };

  // The plan's last summary, rendered as markdown and clamped to a few lines with
  // an expand affordance shown only when it overflows. Re-rendered only when the
  // text changes, so the expand toggle persists across polls.
  const updateSummary = (p) => {
    const host = $("#plansummary");
    if (!host) return;
    const text = p.summary || "";
    if (summaryKey === text) return;
    summaryKey = text;
    if (!text) {
      host.hidden = true;
      host.innerHTML = "";
      return;
    }
    host.hidden = false;
    host.innerHTML = `<div class="ps-body clamped" id="psbody">${renderMarkdown(text)}</div><button class="ps-toggle" id="pstoggle" hidden>Show more</button>`;
    const bodyEl = host.querySelector("#psbody");
    const toggle = host.querySelector("#pstoggle");
    requestAnimationFrame(() => {
      if (!bodyEl.isConnected) return;
      if (bodyEl.scrollHeight - bodyEl.clientHeight > 4) {
        toggle.hidden = false;
        toggle.onclick = () => {
          const clamped = bodyEl.classList.toggle("clamped");
          toggle.textContent = clamped ? "Show more" : "Show less";
        };
      }
    });
  };

  // Mount the plan's drafting-session PTY edge-to-edge into #tabbody (agent.attach
  // is entity-agnostic — a plan id resolves to its planning session). Mirrors
  // task.js's Agent tab: a quiet idle chip over the retained last screen when no
  // session is live. The poll never repaints the body while this is mounted.
  const mountAgent = () => {
    const body = $("#tabbody");
    if (!body) return;
    if (planSelDispose) {
      planSelDispose();
      planSelDispose = null;
    }
    body.classList.add("bare");
    body.innerHTML = `<div class="agentwrap"><div class="agent-idle" id="agentIdle" hidden></div><div class="termpane" id="agentpane"></div></div>`;
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
      mountReviewSkeleton();
      planKey = null;
      stagesKey = null;
      paint();
    }
  };

  // The Review/Agent tab row (the same shell run/worktree use). Rebuilt with the
  // shell (identity/state changes), so the Agent tab appears/disappears as the
  // plan crosses into a terminal state.
  const wireTabs = (p) => {
    const host = $("#plantabs");
    if (!host) return;
    const tabs = [{ id: "review", label: "Review" }, ...(agentAvailable(p) ? [{ id: "agent", label: "Agent" }] : [])];
    tabShellCtl = mountTabShell(host, {
      tabs,
      active: tab,
      onSelect: (t) => selectTab(t),
      back: { title: p.project ? `Back to ${p.project}` : "Back to project" },
      onBack: () => goHome(),
    });
  };

  // The surface shell: the tab bar tops the view; the bar carries the single-line
  // goal, the state chip, and the lifecycle + gate actions. Rebuilt only when the
  // plan's identity/state/run-link changes, so a poll tick never wipes in-flight
  // comment state in the body.
  const shell = (p) => {
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="plantabs"></div>
        <span class="plan-ref quick" title="${esc(p.goal)}">${esc(p.goal)}</span>
        <div class="surface-meta">
          <span id="planmsgaction"></span>
          <span class="chip ${planChipClass(p.state)}" title="${esc(p.goal)}">${PLAN_STATE_LABEL[p.state] || p.state}</span>
          <span class="taskactions" id="planactions"></span>
        </div>
      </div>
      <div class="task-error" id="planError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    wireTabs(p);
    wireActions(p);
    showBanner(bannerText(localError, p.last_error));
    // A shell rebuild wiped #tabbody — re-mount the active surface so the poll's
    // early-return leaves a live pane/skeleton in place (mirrors task.js).
    if (tab === "agent") mountAgent();
    else mountReviewSkeleton();
  };

  // The bar's action cluster: Message (a live/parked planning session), the gate
  // (Approve plan → Implement, or a link to the run implementing it), and removal
  // (Abandon a live plan / Delete a terminal one) — the same slots the run view
  // gives its actions.
  const wireActions = (p) => {
    const msgEl = $("#planmsgaction");
    if (msgEl) {
      msgEl.innerHTML = MESSAGEABLE.includes(p.state) ? '<button class="btn mini" id="planmsg">Message agent</button>' : "";
      const b = $("#planmsg");
      if (b) b.onclick = () => openPlanMessage(p, paint);
    }
    const actions = $("#planactions");
    if (!actions) return;
    actions.innerHTML = "";
    wireGate(p, actions);
    wireRemoval(p, actions);
  };

  // The gate: while at plan_review, Approve plan (plan.approve) tears down the
  // planning worktree and unlocks Implement. A live run implementing the plan
  // links to it; a ready plan gets the Implement split-button; otherwise a disabled
  // Implement carrying the bridge's own rejection reason as its tooltip.
  const wireGate = (p, actions) => {
    if (NO_GATE.has(p.state)) return;

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
          localError = "error: " + e.message.slice(0, 80);
          showBanner(localError);
        }
      };
    }

    if (p.active_run_id) {
      const link = document.createElement("button");
      link.className = "btn primary";
      link.id = "viewrun";
      link.textContent = "View run →";
      link.title = "A run is implementing this plan.";
      link.onclick = () => go({ name: "task", id: p.active_run_id, tab: "changes" });
      actions.appendChild(link);
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
            // banner; either way the split-button restores itself for a retry.
            if (e && e.message && e.message !== "cancelled") {
              localError = "error: " + e.message.slice(0, 80);
              showBanner(localError);
            }
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
  };

  // Removal: Delete a terminal (abandoned) plan, or Abandon a live one.
  const wireRemoval = (p, actions) => {
    let btn;
    if (planDeletable(p.state)) {
      btn = document.createElement("button");
      btn.className = "btn danger mini";
      btn.id = "planremove";
      btn.textContent = "Delete";
    } else if (planAbandonable(p.state)) {
      btn = document.createElement("button");
      btn.className = "btn mini";
      btn.id = "planremove";
      btn.textContent = "Abandon";
    } else {
      return;
    }
    actions.appendChild(btn);
    const deleting = planDeletable(p.state);
    btn.onclick = async () => {
      localError = null;
      if (!deleting && !window.confirm("Abandon this plan? Its planning worktree is removed; the plan stays as history."))
        return;
      btn.disabled = true;
      btn.textContent = deleting ? "deleting…" : "abandoning…";
      try {
        await App.call(deleting ? "plan.delete" : "plan.abandon", { plan_id: id });
        if (deleting) goHome();
        else paint();
      } catch (e) {
        btn.disabled = false;
        btn.textContent = deleting ? "Delete" : "Abandon";
        localError = "error: " + e.message.slice(0, 80);
        showBanner(localError);
      }
    };
  };

  // ---- Single-doc review body (plan.doc + select-to-comment notes) ----------

  function renderSingleDoc(doc, paneState) {
    // Commenting requires the plan under review AND a readable doc — an
    // unavailable/errored doc renders an honest state with no composer.
    const editable = last.state === "plan_review" && paneState === "ready";
    planComments.length = 0;
    const body = $("#planbody");
    const docHtml =
      paneState === "ready" ? renderMarkdown(doc)
      : paneState === "unavailable" ? `<div class="plan-empty">${esc(DOCS_UNAVAILABLE)}</div>`
      : paneState === "error" ? `<div class="plan-empty warn">Couldn't load the plan document. Reopen the plan to retry.</div>`
      : '<div class="plan-loading">✦ loading plan document…</div>';
    body.innerHTML = `
      <div class="plan" id="plandoc">${docHtml}</div>
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
        phint.textContent = "Select text in the plan to comment, or approve the plan above.";
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
    // Fetch the open stage's doc only when it can succeed — never for docs that
    // predate canonical storage, never once a read has errored (latched off).
    let stageDoc = null;
    if (selectedStageId && shouldFetchPlanDoc({ docsAvailable: p.docs_available, errorLatched: stageDocError.has(selectedStageId) })) {
      try {
        stageDoc = await App.call("plan.stage_doc", { plan_id: id, stage_id: selectedStageId });
      } catch {
        stageDocError.add(selectedStageId); // latch: render an error state, stop refetching
      }
    }
    const stageDocState = selectedStageId
      ? planDocPaneState({
          docsAvailable: p.docs_available,
          errorLatched: stageDocError.has(selectedStageId),
          hasContents: !!(stageDoc && stageDoc.contents),
        })
      : "ready";
    const key =
      p.state + " " + JSON.stringify(stagesData) + " " + selectedStageId + " " + stageDocState + " " + (stageDoc ? stageDoc.contents.length : 0);
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
      stageDocState,
      selectedStageId,
      callRpc: (method, params) => App.call(method, params),
      repaint: () => {
        stagesKey = null;
        paint();
      },
      onSelectStage: (stageId) => {
        selectedStageId = stageId;
        // Re-navigating to a stage clears its error latch so the doc is retried.
        if (stageId) stageDocError.delete(stageId);
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

    updateMeta(p);
    updateSummary(p);

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
    // Fetch the single doc only when it can succeed (available + not latched off).
    let doc = "";
    if (shouldFetchPlanDoc({ docsAvailable: p.docs_available, errorLatched: singleDocError })) {
      try {
        doc = (await App.call("plan.doc", { plan_id: id })).contents;
      } catch {
        singleDocError = true; // latch: render an error state, stop refetching
      }
    }
    const paneState = planDocPaneState({ docsAvailable: p.docs_available, errorLatched: singleDocError, hasContents: !!doc });
    const key = p.state + " " + paneState + " " + doc;
    if (planKey === key && $("#plandoc")) return; // nothing changed — keep comments/selection
    planKey = key;
    renderSingleDoc(doc, paneState);
  };

  App.viewDispose = () => {
    if (planSelDispose) planSelDispose();
    disposeAgent();
  };

  await paint();
  App.poll = setInterval(paint, 1600);
}
