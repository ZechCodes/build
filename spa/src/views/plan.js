// The plan cockpit (project-scoped review), keyed by plan_id — the plan-side twin
// of the run surface (views/task.js). The tab bar IS the top of the view (Review |
// Agent, the same shell run/worktree use); a compact bar carries state and gate
// actions. The Review body renders the plan's summary (markdown), then either a
// single doc or multi-stage board, followed by one persistent conversation and
// composer. "The terminal is the basement": the
// Agent tab (the drafting session's PTY) shows only while the plan is non-terminal.
// Live-polled on the review cadence; conversation drafts survive stage switches
// and thread refreshes.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { mountSplitButton } from "../core/splitButton.js";
import { mountTabShell } from "../core/tabshell.js";
import { mountAgentPane } from "../core/surfaceTabs.js";
import { terminalManager } from "../terminal/manager.js";
import { createThreadCache, threadHtml, wireThreadRevisionLinks } from "../core/thread.js";
import { planReviewSkeletonHtml } from "../core/planReview.js";
import { App, go, loadModelCatalog, markEntityRead } from "../app.js";
import { PLAN_STATE_LABEL, PLAN_TERMINAL_STATES, planChipClass } from "./shared.js";
import {
  canImplement,
  implementBlockReason,
  planAbandonable,
  planDeletable,
  bannerText,
  shouldFetchPlanDoc,
  planDocPaneState,
  approvePlanConfirm,
  implementConfirm,
  abandonPlanConfirm,
  deletePlanConfirm,
  planBackTarget,
} from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { openImplementOptions } from "../sheets/implement.js";
import { notifyError } from "../core/notify.js";
import { hideCommentPop, hasCommentPop } from "../commentPop.js";
import { renderPlanStages, planStageActionBusy, docErrorPaneHtml, DOCS_UNAVAILABLE } from "./planStages.js";
import { hashFromRoute } from "../core/router.js";

// Plan states whose planning session accepts a freeform message (the bridge
// gates the rest): a live drafting session redirects, a parked one resumes.
const MESSAGEABLE = ["drafting", "blocked", "failed", "idle_unreported", "interrupted"];
// States that carry no plan-level gate yet (no Approve/Implement) — nothing to
// approve or implement while the agent is still drafting, and nothing once abandoned.
const NO_GATE = new Set(["created", "drafting", "abandoned"]);

export async function renderPlan() {
  const root = $("#root");
  const id = App.route.id;
  let projectId = App.route.projectId || null;
  // If the user entered this plan from a run (task.js's openPlan stamped the
  // marker), the back chevron returns to that run's Stages tab (the plan↔run
  // round trip). Read once at entry; the chevron clears it on return.
  const returnRunId = sessionStorage.getItem("build.planReturn." + id);
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
  const replacePlanHash = () => {
    App.route = {
      name: "plan",
      ...(projectId ? { projectId } : {}),
      id,
      tab,
      ...(tab === "review" && selectedStageId ? { stage: selectedStageId } : {}),
    };
    history.replaceState(null, "", hashFromRoute(App.route));
  };
  let stagesKey = null;
  // Single-doc review state preserved across the poll.
  let planKey = null;
  // The summary block's freeze key — re-rendered only when the summary text
  // changes, so the expand/collapse toggle survives ticks.
  let summaryKey = null;
  let threadRenderKey = null;
  let threadDraft = "";
  // Cursor cache for the conversation: each poll sends the last-held sequence
  // so the bridge ships only new items, not the whole thread every 1.6s.
  const threadCache = createThreadCache();
  // Doc-read error latches: any plan.doc / plan.stage_doc ERROR renders an error
  // state and stops that doc's refetch until the user re-navigates (mirrors the
  // planGone latch in task.js). singleDocError is the single-doc latch; stageDocError
  // holds the latched stage ids.
  let singleDocError = false;
  const stageDocError = new Set();
  // Latched once plan.get reports "unknown plan_id": the plan was deleted out
  // from under this view. We stop the poll and render a terminal gone-state so a
  // stray tick can never repaint over it (mirrors task.js's planGone latch).
  let gone = false;

  loadModelCatalog(); // warm the catalog before the Implement options need it

  const goHome = () =>
    go(last && last.project_id ? { name: "project", projectId: last.project_id } : { name: "notifications" });

  // The plan is gone (deleted while we were on it): stop everything and render a
  // latched terminal state with a way back — the owning project if a prior paint
  // learned it, else notifications. Tears down the agent pane so nothing lingers
  // under the replaced #root.
  const renderGone = () => {
    gone = true;
    if (App.poll) {
      clearInterval(App.poll);
      App.poll = null;
    }
    disposeAgent();
    const backLabel = last && last.project_id ? "Back to project" : "Back to notifications";
    root.innerHTML = `<div class="empty gone">This plan no longer exists.<div><button class="btn" id="goneback">${backLabel}</button></div></div>`;
    const back = $("#goneback");
    if (back) back.onclick = () => goHome();
  };

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

  // The Review body skeleton: project metadata, summary, doc/stage host, then the
  // persistent conversation. Mounted once per shell rebuild or tab switch; the
  // poll refreshes each region in place.
  const mountReviewSkeleton = () => {
    const body = $("#tabbody");
    if (!body) return;
    body.classList.remove("bare");
    body.innerHTML = planReviewSkeletonHtml();
    summaryKey = null; // force the summary to repaint into the fresh skeleton
    threadRenderKey = null;
  };

  const updateMeta = (p) => {
    const el = $("#planmeta");
    if (el) el.textContent = (p.project || "") + (p.base_branch ? ` · ${p.base_branch}` : "");
  };

  const updateThread = (p) => {
    const host = $("#planthread");
    if (host) {
      // The plan's own composer ids — passed explicitly so this composer can
      // never collide with another surface's (e.g. the diff composer's).
      const composer = (p.state === "plan_review" || MESSAGEABLE.includes(p.state)) && {
        inputId: "planthreadinput",
        sendId: "planthreadsend",
        hintId: "planthreadhint",
        placeholder: "Send a message to the planning agent…",
      };
      const key = JSON.stringify({
        goal: p.goal || "",
        state: p.state,
        harness: p.harness || "",
        sessions: p.thread?.sessions || [],
        items: p.thread?.items || [],
        revisions: p.thread?.revisions || [],
        completion: p.thread?.last_completion || null,
      });
      if (key === threadRenderKey && host.firstChild) return;
      threadRenderKey = key;
      host.innerHTML = threadHtml(p.thread, { initialMessage: p.goal, composer, agentLabel: p.harness });
      wireThreadRevisionLinks(host, (revisionId) => App.call("thread.revision", { entity_id: id, revision_id: revisionId }));
      const input = host.querySelector("#planthreadinput");
      const send = host.querySelector("#planthreadsend");
      const hint = host.querySelector("#planthreadhint");
      if (!input || !send) return;
      input.value = threadDraft;
      input.oninput = () => {
        threadDraft = input.value;
        if (hint) hint.textContent = "";
      };
      const submit = async () => {
        const message = input.value.trim();
        if (!message) {
          if (hint) hint.textContent = "Type a message first.";
          input.focus();
          return;
        }
        send.disabled = true;
        send.textContent = "sending…";
        try {
          if (p.state === "plan_review") {
            await App.call("plan.send_notes", { plan_id: id, messages: [{ body: message, anchor: null }] });
          } else {
            await App.call("plan.message", { plan_id: id, message });
          }
          threadDraft = "";
          threadRenderKey = null;
          planKey = null;
          stagesKey = null;
          await paint();
        } catch (error) {
          send.disabled = false;
          send.textContent = "Send";
          notifyError("Message failed", error.message);
        }
      };
      send.onclick = submit;
      input.onkeydown = (event) => {
        if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
          event.preventDefault();
          submit();
        }
      };
    }
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
    replacePlanHash();
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
    // The chevron returns to the originating run (if we came from one and it's
    // still this plan's active run), else up to the project — goHome's target.
    const backTarget = planBackTarget({ returnRunId, activeRunId: p && p.active_run_id, projectId: p && p.project_id });
    const backTitle =
      backTarget.name === "task" ? "Back to run" : p && p.project ? `Back to ${p.project}` : "Back to project";
    tabShellCtl = mountTabShell(host, {
      tabs,
      active: tab,
      onSelect: (t) => selectTab(t),
      back: { title: backTitle },
      onBack: () => {
        if (backTarget.name === "task") sessionStorage.removeItem("build.planReturn." + id);
        go(backTarget);
      },
    });
  };

  // The surface shell: the tab bar tops the view; the bar carries the state chip
  // and lifecycle + gate actions. Rebuilt only when the
  // plan's identity/state/run-link changes, so a poll tick never wipes in-flight
  // comment state in the body.
  const shell = (p) => {
    const m = p || {};
    root.innerHTML = `
      <div class="surface-bar">
        <div class="tabrow" id="plantabs"></div>
        <div class="surface-meta">
          <span class="chip ${planChipClass(m.state)}" title="${esc(m.goal || "")}">${PLAN_STATE_LABEL[m.state] || m.state || ""}</span>
          <span class="taskactions" id="planactions"></span>
        </div>
      </div>
      <div class="task-error" id="planError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    wireTabs(p);
    // Actions/banner/agent only make sense once a real plan is in hand; the
    // skeleton shell (p null, route entry) shows the bar + a loading body.
    if (p) {
      wireActions(p);
      showBanner(bannerText(localError, p.last_error));
    }
    // A shell rebuild wiped #tabbody — re-mount the active surface so the poll's
    // early-return leaves a live pane/skeleton in place (mirrors task.js).
    if (tab === "agent" && p) mountAgent();
    else mountReviewSkeleton();
  };

  // The bar's action cluster: the gate (Approve plan → Implement, or a link to
  // the run implementing it) and removal (Abandon/Delete). Agent messages live
  // in the persistent conversation below the plan.
  const wireActions = (p) => {
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
        // A decisive gate: confirm what approval does; cancel leaves the
        // button (and any held error) untouched.
        if (!(await confirmAction(approvePlanConfirm()))) return;
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
      // A multi-stage plan's run is driven from its Stages tab (the stage_gate
      // case); single-doc plans open on Changes.
      link.onclick = () =>
        go({ name: "task", projectId: p.project_id, id: p.active_run_id, tab: p.stages && p.stages.length ? "stages" : "changes" });
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
          // Plain Implement is a decisive gate: confirm with what-happens-next
          // framing before dispatch. The with-options path opens the options
          // sheet — the sheet IS the deliberate step, so no extra modal there.
          // Cancel throws before the RPC (split button restores, no banner).
          if (
            optionId === "implement" &&
            !(await confirmAction(implementConfirm({ base: p.base_branch || "the base branch" })))
          )
            throw new Error("cancelled");
          localError = null;
          let run;
          try {
            run = optionId === "implement"
              ? await App.call("run.create", { plan_id: id })
              : await openImplementOptions(p, await loadModelCatalog());
          } catch (e) {
            // A dispatch error (not the options-sheet/confirm cancel) raises a
            // persistent expandable notification (G2, full message); either way
            // the split-button restores itself for a retry.
            if (e && e.message && e.message !== "cancelled") {
              notifyError("Implement failed", e.message);
            }
            throw e;
          }
          go({ name: "task", projectId: p.project_id, id: run.run_id, tab: "changes" });
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
      // Both removal verbs confirm with their step outline; cancel leaves the
      // button (and any held error) untouched.
      if (!(await confirmAction(deleting ? deletePlanConfirm() : abandonPlanConfirm()))) return;
      localError = null;
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

  // ---- Single-doc review body ------------------------------------------------

  function renderSingleDoc(doc, paneState) {
    const body = $("#planbody");
    const docHtml =
      paneState === "ready" ? renderMarkdown(doc)
      : paneState === "unavailable" ? `<div class="plan-empty">${esc(DOCS_UNAVAILABLE)}</div>`
      : paneState === "error" ? docErrorPaneHtml("plan")
      : '<div class="plan-loading">✦ loading plan document…</div>';
    body.innerHTML = `<div class="plan" id="plandoc">${docHtml}</div>`;
    // A doc-read error latched the pane; the inline Retry clears the latch, forces
    // a refetch, and repaints (W15).
    if (paneState === "error") {
      const retry = $("#docretry");
      if (retry)
        retry.onclick = () => {
          singleDocError = false;
          planKey = null;
          paint();
        };
    }
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
    const busy = hasCommentPop() || planStageActionBusy();
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
        // Deep-link the open stage into the hash without re-routing so the
        // selection is shareable and survives a
        // reload; back-to-list drops the segment. App.route.stage stays in sync.
        replacePlanHash();
        // Re-navigating to a stage clears its error latch so the doc is retried.
        if (stageId) stageDocError.delete(stageId);
        stagesKey = null;
        hideCommentPop();
        paint();
      },
      // The stage doc's inline Retry (W15): clear that stage's read latch, force a
      // stages rebuild, and repaint so the doc is refetched.
      onRetryStageDoc: (stageId) => {
        stageDocError.delete(stageId);
        stagesKey = null;
        paint();
      },
    });
  }

  let visitMarkedRead = false; // paint() marks the plan read once per visit

  const paint = async () => {
    if (gone || App.offline) return; // latched gone-state / offline freeze: no repaint
    let p;
    try {
      p = await App.call("plan.get", { plan_id: id, ...threadCache.cursorParam() });
    } catch (e) {
      // A deleted plan is permanent: latch the gone-state and stop polling.
      // Every other error is transient — stay silent and let the poll retry.
      if (/unknown plan_id/.test((e && e.message) || "")) renderGone();
      return;
    }
    // Fold the cursored conversation delta back into a full thread before
    // anything below reads p.thread.
    p = { ...p, thread: threadCache.absorb(p.thread) };
    // Visiting the plan reads it: mark once, on the first successful fetch.
    if (!visitMarkedRead) {
      visitMarkedRead = true;
      markEntityRead(id);
    }
    // A plan that just crossed into a terminal state loses its Agent tab; fall
    // back to Review before the shell rebuild so the surface stays consistent.
    if (tab === "agent" && !agentAvailable(p)) {
      selectTab("review");
      return;
    }
    const needShell = !last || last.state !== p.state || last.goal !== p.goal || last.active_run_id !== p.active_run_id;
    last = p;
    if (p.project_id && p.project_id !== projectId) {
      projectId = p.project_id;
      replacePlanHash();
    }
    if (needShell) shell(p);
    showBanner(bannerText(localError, p.last_error));

    // The Agent pane owns the body; the poll never repaints it.
    if (tab === "agent") return;

    updateMeta(p);
    updateSummary(p);
    updateThread(p);

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
    if (planKey === key && $("#plandoc")) return;
    planKey = key;
    renderSingleDoc(doc, paneState);
  };

  App.viewDispose = () => {
    disposeAgent();
  };

  shell(null); // route entry: paint the surface-bar skeleton + a loading body at once
  await paint();
  App.poll = setInterval(paint, 1600);
}
