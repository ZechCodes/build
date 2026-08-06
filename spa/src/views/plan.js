// The plan cockpit (project-scoped review), keyed by plan_id — the plan-side twin
// of the run surface (views/task.js). Conversation is the first/default tab;
// Stages owns the plan documents and stage board; Agent owns the planning PTY.
// "The terminal is the basement": the
// Agent tab (the drafting session's PTY) shows only while the plan is non-terminal.
// Live-polled on the review cadence; conversation drafts survive stage switches
// and thread refreshes.

import { $ } from "../dom.js";
import { esc } from "../core/text.js";
import { renderMarkdown } from "../core/markdown.js";
import { mountSplitButton } from "../core/splitButton.js";
import { mountTabShell } from "../core/tabshell.js";
import { mountAgentTab } from "../core/surfaceTabs.js";
import { isProjectClusterTab, mountProjectClusterTab, projectClusterShellOptions } from "../core/projectCluster.js";
import { createThreadCache, threadHtml, wireThreadComposer, wireThreadLinks, wireThreadRevisionLinks } from "../core/thread.js";
import { planReviewSkeletonHtml } from "../core/planReview.js";
import { App, go, loadModelCatalog, markEntityRead } from "../app.js";
import { PLAN_STATE_LABEL, PLAN_TERMINAL_STATES, planChipClass } from "./shared.js";
import {
  canImplement,
  implementBlockReason,
  planDeletable,
  bannerText,
  shouldFetchPlanDoc,
  planDocPaneState,
  approvePlanConfirm,
  implementConfirm,
  deletePlanConfirm,
  planBackTarget,
} from "../core/taskActions.js";
import { confirmAction } from "../core/confirm.js";
import { openImplementOptions } from "../sheets/implement.js";
import { notifyError } from "../core/notify.js";
import { hideCommentPop, hasCommentPop } from "../commentPop.js";
import { renderPlanStages, planStageActionBusy, docErrorPaneHtml, DOCS_UNAVAILABLE } from "./planStages.js";
import { hashFromRoute } from "../core/router.js";

// States that carry no plan-level gate yet (no Approve/Implement) — nothing to
// approve or implement while the agent is still drafting, and nothing once abandoned.
const NO_GATE = new Set(["created", "drafting", "abandoned"]);

export function issueThreadLinkTarget(link, issue, fallbackProjectId) {
  const implementationId = link.implementation_id || link.run_id;
  if ((link.kind === "implementation" || link.kind === "run") && implementationId) {
    return { route: { name: "task", projectId: issue.project_id || fallbackProjectId, id: implementationId, tab: "conversation" } };
  }
  if (link.kind === "file" && link.path) {
    const runId = issue.current_implementation_id || issue.active_run_id;
    if (!runId) return null;
    return {
      route: { name: "task", projectId: issue.project_id || fallbackProjectId, id: runId, tab: "files" },
      filePath: link.path,
    };
  }
  return null;
}

export async function renderPlan() {
  const root = $("#root");
  const id = App.route.id;
  // Looking at an issue is seeing it — the dot settles until it moves again.
  App.call("entity.seen", { entity_id: id }).catch(() => {});
  let projectId = App.route.projectId || null;
  // If the user entered this plan from a run (task.js's openPlan stamped the
  // marker), the back chevron returns to that run's Stages tab (the plan↔run
  // round trip). Read once at entry; the chevron clears it on return.
  const returnRunId = sessionStorage.getItem("build.planReturn." + id);
  let last = null;
  // Conversation and plan artifacts are separate surfaces. The Agent tab shows
  // only while the plan is non-terminal.
  let tab =
    ["conversation", "stages", "agent"].includes(App.route.tab) || isProjectClusterTab(App.route.tab)
      ? App.route.tab
      : "conversation";
  let tabShellCtl = null;
  // The one mounted pane that owns #tabbody: the planning agent's screen, or a
  // right-cluster tab (Inbox/Archive). The poll never repaints under it.
  let mountedPane = null;
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
      ...(tab === "stages" && selectedStageId ? { stage: selectedStageId } : {}),
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
    disposePane();
    const backLabel = last && last.project_id ? "Back to project" : "Back to notifications";
    root.innerHTML = `<div class="empty gone">This Issue no longer exists.<div><button class="btn" id="goneback">${backLabel}</button></div></div>`;
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

  const disposePane = () => {
    if (mountedPane) {
      mountedPane.dispose();
      mountedPane = null;
    }
  };

  const mountStagesSkeleton = () => {
    const body = $("#tabbody");
    if (!body) return;
    body.classList.remove("bare");
    body.innerHTML = planReviewSkeletonHtml();
    summaryKey = null; // force the summary to repaint into the fresh skeleton
  };

  const mountConversationSkeleton = () => {
    const body = $("#tabbody");
    if (!body) return;
    body.classList.remove("bare");
    body.innerHTML = '<div id="planthread"></div>';
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
      const composer = p.state !== "abandoned" && {
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
      host.innerHTML = threadHtml(p.thread, {
        initialMessage: p.goal,
        composer,
        agentLabel: p.harness,
        status: { label: PLAN_STATE_LABEL[p.state] || p.state || "", cls: planChipClass(p.state) },
        actionsId: "issuelifecycle",
      });
      // Re-mounted per render: this body is rebuilt whenever the thread changes.
      wireActions(p, host.querySelector("#issuelifecycle"));
      wireThreadRevisionLinks(host, (revisionId) => App.call("thread.revision", { entity_id: id, revision_id: revisionId }));
      wireThreadLinks(host, (link) => {
        if ((link.kind === "issue_stage" || link.kind === "plan_stage") && link.stage_id) {
          selectedStageId = link.stage_id;
          selectTab("stages");
          return;
        }
        const target = issueThreadLinkTarget(link, p, projectId);
        if (!target) return;
        if (target.filePath) sessionStorage.setItem(`build.fileLink.${target.route.id}`, target.filePath);
        go(target.route);
      });
      wireThreadComposer(host, {
        ids: { input: "planthreadinput", send: "planthreadsend", hint: "planthreadhint" },
        readDraft: () => threadDraft,
        writeDraft: (value) => {
          threadDraft = value;
        },
        onSubmit: (message) => App.call("thread.post", { entity_id: id, body: message }),
        afterSubmit: () => {
          threadRenderKey = null;
          planKey = null;
          stagesKey = null;
          paint();
        },
        onError: (error) => notifyError("Message failed", error.message),
      });
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

  // Mount the plan's drafting-session PTY edge-to-edge into #tabbody. The same
  // Agent tab every worktree surface has: a quiet idle chip over the retained
  // last screen when no session is live, and never a spawn on mount. The poll
  // never repaints the body while this is mounted.
  const mountAgent = () => {
    const body = $("#tabbody");
    if (!body) return;
    body.classList.add("bare");
    // The plan addresses its own agent; the bridge resolves that to the
    // disposable planning worktree, which is where that agent lives — and which
    // is why an approved or abandoned plan has no agent tab at all.
    mountedPane = mountAgentTab(
      body,
      { id },
      {
        idleLabel: "No planning agent is currently running",
        onStart: (provider) => App.call("agent.start", { id, ...(provider ? { provider } : {}) }),
      },
    );
  };

  // The project-wide panes behind the tab bar's right cluster: the same Inbox
  // and Archive every project surface reaches, mounted here.
  const mountClusterTab = (tabId) => {
    const body = $("#tabbody");
    if (!body || !projectId) return;
    body.classList.remove("bare");
    mountedPane = mountProjectClusterTab(body, tabId, {
      projectId,
      callRpc: (method, params) => App.call(method, params),
      navigate: go,
    });
  };

  // Switch surfaces: Review repaints through the poll machinery; Agent and the
  // cluster tabs own their own bodies and are never touched by the poll.
  const selectTab = (next) => {
    tab = next;
    replacePlanHash();
    if (tabShellCtl) tabShellCtl.setActive(next);
    disposePane();
    if (isProjectClusterTab(next)) {
      mountClusterTab(next);
    } else if (next === "agent") {
      mountAgent();
    } else if (next === "conversation") {
      mountConversationSkeleton();
      paint();
    } else {
      mountStagesSkeleton();
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
    const tabs = [
      { id: "conversation", label: "Conversation" },
      { id: "stages", label: p && p.stages && p.stages.length ? "Stages" : "Stage plan" },
      ...(agentAvailable(p) ? [{ id: "agent", label: "Agent" }] : []),
    ];
    // The chevron returns to the originating run (if we came from one and it's
    // still this plan's active run), else up to the project — goHome's target.
    const backTarget = planBackTarget({ returnRunId, activeRunId: p && p.active_run_id, projectId: p && p.project_id });
    const backTitle =
      backTarget.name === "task" ? "Back to run" : p && p.project ? `Back to ${p.project}` : "Back to project";
    tabShellCtl = mountTabShell(host, {
      tabs,
      active: tab,
      onSelect: (t) => selectTab(t),
      ...projectClusterShellOptions({
        projectId: (p && p.project_id) || projectId,
        selectTab: (t) => selectTab(t),
      }),
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
      </div>
      <div class="task-error" id="planError" role="alert" hidden></div>
      <div id="tabbody"></div>`;
    wireTabs(p);
    // The banner only makes sense once a real plan is in hand; the skeleton
    // shell (p null, route entry) shows the bar + a loading body. The action
    // cluster is mounted by the conversation render, not here.
    if (p) showBanner(bannerText(localError, p.last_error));
    // A shell rebuild wiped #tabbody — re-mount the active surface so the poll's
    // early-return leaves a live pane/skeleton in place (mirrors task.js).
    if (isProjectClusterTab(tab)) mountClusterTab(tab);
    else if (tab === "agent" && p) mountAgent();
    else if (tab === "conversation") mountConversationSkeleton();
    else mountStagesSkeleton();
  };

  // The issue's action cluster, mounted at the end of its conversation: the gate
  // (Approve plan → Implement, or a link to the run implementing it) and removal
  // (Abandon/Delete). The surface bar carries tabs and nothing else, so the
  // decisions sit on the record that explains them.
  const wireActions = (p, actions) => {
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
      approve.textContent = "Mark issue ready";
      actions.appendChild(approve);
      approve.onclick = async () => {
        // A decisive gate: confirm what approval does; cancel leaves the
        // button (and any held error) untouched.
        if (!(await confirmAction(approvePlanConfirm()))) return;
        localError = null;
        approve.disabled = true;
        approve.textContent = "approving…";
        try {
          await App.call("issue.approve", { issue_id: id });
          planKey = null;
          stagesKey = null;
          paint();
        } catch (e) {
          approve.disabled = false;
          approve.textContent = "Mark issue ready";
          localError = "error: " + e.message.slice(0, 80);
          showBanner(localError);
        }
      };
    }

    if (p.active_run_id) {
      const link = document.createElement("button");
      link.className = "btn primary";
      link.id = "viewrun";
      link.textContent = "View implementation stages →";
      link.title = "This Issue has an active implementation.";
      link.onclick = () => selectTab("stages");
      actions.appendChild(link);
      return;
    }
    if (canImplement(p)) {
      const host = document.createElement("span");
      host.className = "splitbtn-host";
      actions.appendChild(host);
      mountSplitButton(host, {
        options: [
          { id: "implement", label: "Implement All", busyLabel: "starting…", description: "Create or reuse the Issue worktree and implement approved stage plans sequentially." },
          { id: "implement_opts", menuLabel: "Implement All with options…", busyLabel: "starting…", description: "Override the base branch, model, or effort for this implementation." },
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
              ? p.stages && p.stages.length
                ? await App.call("issue.implement_all", { issue_id: id })
                : await App.call("run.create", { issue_id: id })
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
          if (run.run_id) go({ name: "task", projectId: p.project_id, id: run.run_id, tab: "conversation" });
          else {
            planKey = null;
            stagesKey = null;
            paint();
          }
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
    disabled.textContent = "Implement All";
    actions.appendChild(disabled);
  };

  // Historical abandoned records remain deletable for compatibility. There is
  // deliberately no live Issue Abandon affordance.
  const wireRemoval = (p, actions) => {
    if (!planDeletable(p.state)) return;
    const btn = document.createElement("button");
    btn.className = "btn danger mini";
    btn.id = "planremove";
    btn.textContent = "Delete";
    actions.appendChild(btn);
    btn.onclick = async () => {
      if (!(await confirmAction(deletePlanConfirm()))) return;
      localError = null;
      btn.disabled = true;
      btn.textContent = "deleting…";
      try {
        await App.call("issue.delete", { issue_id: id });
        goHome();
      } catch (e) {
        btn.disabled = false;
        btn.textContent = "Delete";
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
      : '<div class="plan-loading">✦ loading stage plan document…</div>';
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
      stagesData = await App.call("issue.stages", { issue_id: id });
    } catch {
      return; // not readable yet; the poll retries
    }
    // Fetch the open stage's doc only when it can succeed — never for docs that
    // predate canonical storage, never once a read has errored (latched off).
    let stageDoc = null;
    if (selectedStageId && shouldFetchPlanDoc({ docsAvailable: p.docs_available, errorLatched: stageDocError.has(selectedStageId) })) {
      try {
        stageDoc = await App.call("issue.stage_doc", { issue_id: id, stage_id: selectedStageId });
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
      p = await App.call("issue.get", { issue_id: id, ...threadCache.cursorParam() });
    } catch (e) {
      // A deleted plan is permanent: latch the gone-state and stop polling.
      // Every other error is transient — stay silent and let the poll retry.
      if (/unknown (issue_id|plan_id)/.test((e && e.message) || "")) renderGone();
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
      selectTab("conversation");
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

    // The Agent pane and the cluster panes own the body; the poll never repaints
    // them. A cluster tab entered by URL mounts here instead, once this paint has
    // learned which project the plan belongs to.
    if (isProjectClusterTab(tab)) {
      if (!mountedPane) mountClusterTab(tab);
      return;
    }
    if (tab === "agent") return;

    if (tab === "conversation") {
      updateThread(p);
      return;
    }

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
      return;
    }
    // Fetch the single doc only when it can succeed (available + not latched off).
    let doc = "";
    if (shouldFetchPlanDoc({ docsAvailable: p.docs_available, errorLatched: singleDocError })) {
      try {
        doc = (await App.call("issue.doc", { issue_id: id })).contents;
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
    disposePane();
  };

  shell(null); // route entry: paint the surface-bar skeleton + a loading body at once
  await paint();
  App.poll = setInterval(paint, 1600);
}
