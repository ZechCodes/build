// The task surface: a persistent two-column stages | stage viewer.
//
// A task is a work item whose children are its stages and the implementations
// that carried them out, so the left column is the task itself — its stages
// with their states, the gate and approve actions, the worktree/agent
// assignment control, and its implementation lineage — and the right column is
// whichever stage doc is open. There are no tabs and no drill-in: selecting a
// stage swaps the viewer and the list stays where it is.
//
// Paints from the task's own records with a keyed freeze; the freeze also
// holds while the reviewer is mid-comment or an action is in flight. Tasks
// left the board, so nothing fills those records but this surface: it reads
// through them (core/taskCache.js) on mount — the records paint the frame and
// the machine is asked behind them — when a `state` push says the task moved,
// and when a verb it sent changed something. There is no poll.
// The pure pieces live in taskModel.js (decisions) and taskRender.js
// (markup); this file is the wiring.

import { el } from "../dom.js";
import { esc } from "./text.js";
import { openAssignmentOverlay } from "./assignmentOverlay.js";
import { createAgentSelection } from "./agentSelection.js";
import { docCommentAnchor } from "./notes.js";
import { renderMarkdown } from "./markdown.js";
import { initPaneDrawer, paneDrawerHtml } from "./paneDrawer.js";
import { notifyError } from "./notify.js";
import { confirmAction } from "./confirm.js";
import {
  approvePlanConfirm,
  deletePlanConfirm,
  implementConfirm,
  implementBlockReason,
  planDeletable,
  planDocPaneState,
  shouldFetchPlanDoc,
  stageNotesTarget,
} from "./taskActions.js";
import {
  DOCS_UNAVAILABLE,
  docErrorPaneHtml,
  docMarkerParts,
  stageListHtml,
  stageRowHtml,
  stageViewerHtml,
} from "./taskRender.js";
import {
  defaultAssignment,
  docMarkerGroups,
  implementParams,
  taskViewKey,
  lineageRoute,
  plannedStageIds,
  stageApprovable,
  stageStateToken,
  worktreeChoices,
} from "./taskModel.js";
import { createDocCommentLayer, headingForKey } from "./taskDocComments.js";
import { MUTATION_THREAD_PAGE, SMALLEST_THREAD_PAGE } from "./thread.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { watchChanges } from "./changeEvents.js";
import { refreshFeed } from "./taskFeed.js";
import { entryKeyOf } from "./inbox.js";
import { INBOX_SCOPE } from "./inboxView.js";
import { removeRecord, runOptimistic } from "./optimistic.js";
import { replyOrNothing } from "./session.js";
import {
  forgetTaskRecords,
  TASK_RECORD_KIND,
  taskRecordsHeld,
  readTaskRecord,
  readStoredTaskRecord,
} from "./taskCache.js";
import { subscribeCache } from "./localCache.js";
import { scrollWithin } from "./scrollWithin.js";

/** Bind an async RPC to a button: disable + label while in flight, restore and
 *  raise a persistent expandable error notification on failure. */
function bindAction(button, busyLabel, run) {
  if (!button) return;
  button.onclick = async () => {
    const original = button.textContent;
    button.disabled = true;
    button.textContent = busyLabel;
    try {
      await run();
    } catch (e) {
      button.disabled = false;
      button.textContent = original;
      if (e && e.message !== "cancelled") notifyError(original + " failed", e.message);
    }
  };
}

/** The stage the view opens on: a deep-linked one when it still exists, else the
 *  first stage still awaiting approval, else the first stage. Pure. */
export function openingStageId(stages, deepLinked) {
  const list = stages || [];
  if (!list.length) return null;
  if (deepLinked && list.some((stage) => stage.id === deepLinked)) return deepLinked;
  const awaiting = list.find(stageApprovable);
  return (awaiting || list[0]).id;
}

/** Whether the doc can still take comments: the bridge accepts them on
 *  planned/approved stage docs only, and only a doc that actually rendered has
 *  passages to select. */
export function docAnnotatable(stage, paneState) {
  if (!stage || paneState !== "ready") return false;
  const approval = stage.approval || stage.state;
  return approval === "planned" || approval === "approved";
}

/**
 * mountTaskView(container, options) → { dispose }.
 *
 * `taskId` names the task; `callRpc(method, params)` is the RPC channel;
 * `navigate(route)` opens another surface; `loadCatalog()` resolves the model
 * catalog for the assignment control and `loadWorkItems()` its feed rows (the
 * branches an implementation can be sent into); `onSelectStage(stageId)` lets
 * the host keep the URL on the open stage; `onGone()` is called when the task
 * no longer exists.
 */
// eslint-disable-next-line complexity -- ratchet: mountTaskView is at 12, cap 10 — reduce it, then drop this line
export function mountTaskView(
  container,
  {
    taskId,
    projectId = null,
    callRpc,
    navigate = () => {},
    loadCatalog = async () => ({}),
    loadWorkItems = async () => [],
    onSelectStage = () => {},
    onProject = () => {},
    onGone = () => {},
    deviceId = null,
    initialStageId = null,
    // Whose conversation this surface is reading and writing into. A task
    // carries exactly one agent session, so this all but always names it — but
    // it is the rail's bubble that says so, and the poll asks with it.
    agentSelection = createAgentSelection(),
    viewingContext = null,
  } = {},
) {
  let disposed = false;
  let gone = false;
  let task = null;
  let stagesData = { stages: [] };
  let stageDoc = null; // { stage_id, contents } for the open stage
  let openStageDiffId = null;
  let stageDiff = null;
  let catalog = {};
  let workItems = [];
  let selectedStageId = initialStageId;
  let selectionSeeded = false;
  let assignment = defaultAssignment(null);
  let assignmentOverlay = null;
  let renderedKey = null;
  let actionsInFlight = 0;
  let drawer = null;
  // The plan of a task that has no stage manifest at all (a migrated task
  // predating stages): one doc, read-only, so it is still readable here.
  let singleDoc = null;
  let singleDocError = false;
  // Per-stage doc-read latches: a stage_doc ERROR renders an error state and
  // stops that doc's refetch until the user re-selects it.
  const docErrors = new Set();
  let cacheDirty = false;
  let cachePaintGeneration = 0;

  container.innerHTML = '<div class="taskview"><div class="empty">loading…</div></div>';

  let project = projectId;
  const currentProjectId = () => project;
  /** The branches this task's implementation could be sent into. */
  const worktrees = () => worktreeChoices(workItems, { projectId: currentProjectId(), taskId });
  const stages = () => stagesData.stages || [];
  const selectedStage = () => stages().find((stage) => stage.id === selectedStageId) || null;
  const docContents = () => (stageDoc && stageDoc.stage_id === selectedStageId ? stageDoc.contents || "" : "");
  const paneState = () =>
    selectedStageId
      ? planDocPaneState({
          docsAvailable: task && task.docs_available,
          errorLatched: docErrors.has(selectedStageId),
          hasContents: Boolean(docContents()),
        })
      : "ready";

  const commentLayer = createDocCommentLayer({
    docText: () => docContents(),
    onChange: () => render(),
    submit: async ({ comments, general }) => {
      const stage = selectedStage();
      if (!stage) throw new Error("no stage is open");
      // One press is one addressed batch. Neither the stage picker nor the
      // agent rail may retarget its later posts while an earlier comment is
      // still crossing the bridge.
      const destination = { stageId: stage.id, agentScope: agentSelection.scope() };
      const context = viewingContext?.snapshot?.();
      for (const comment of comments) {
        await callRpc("task.comment_add", {
          task_id: taskId,
          stage_id: destination.stageId,
          body: comment.comment,
          anchor: docCommentAnchor(comment),
          ...(context ? { viewing_context: context } : {}),
        });
      }
      // The general note is a turn like any other: durable on the daemon's side
      // the moment it answers, and answered before the agent it wakes exists.
      // A reply that outlives the browser's timer leaves the comment posted, so
      // raising here would only have the human write it again.
      if (general)
        await replyOrNothing(
          callRpc("thread.post", {
            entity_id: taskId,
            ...destination.agentScope,
            body: general,
            ...(context ? { viewing_context: context } : {}),
            ...MUTATION_THREAD_PAGE,
          }),
        );
      viewingContext?.clearSelectionIfMatches?.(context);
      renderedKey = null;
      await refresh();
    },
  });

  /** The user is filling in the assignment overlay, so the poll waits: the panel
   *  is painted from what this view holds, and a pass that lands mid-choice is a
   *  pass that repaints under their cursor. */
  const assignmentBusy = () => Boolean(assignmentOverlay && assignmentOverlay.busy());

  const guarded = async (work) => {
    actionsInFlight += 1;
    try {
      return await work();
    } finally {
      actionsInFlight -= 1;
      if (actionsInFlight === 0 && cacheDirty) scheduleCachePaint();
    }
  };

  // ---- the persistent skeleton ---------------------------------------------

  const paintSkeleton = () => {
    container.innerHTML = `<div class="taskview">
      <div class="ivsplit pane-split">
        <aside class="ivstages pane-list"></aside>
        <section class="ivviewer"></section>
      </div></div>`;
    const split = container.querySelector(".ivsplit");
    split.insertAdjacentHTML("beforeend", paneDrawerHtml("stages"));
    if (drawer) drawer.dispose();
    drawer = initPaneDrawer(split, { list: split.querySelector(".ivstages"), closeOnSelect: ".stagerow, .ivlin-row" });
    container.onclick = handleClick;
  };

  // eslint-disable-next-line complexity -- ratchet: this callback is at 11, cap 10 — reduce it, then drop this line
  const render = () => {
    if (disposed || gone || !task) return;
    if (!container.querySelector(".ivsplit")) paintSkeleton();
    const listHost = container.querySelector(".ivstages");
    paintStageColumn(listHost);
    wireStageList(listHost);
    // The overlay is painted from the same held values, so a pass that changed
    // them reaches it too. Its own paint is a no-op when they did not.
    if (assignmentOverlay) assignmentOverlay.update();
    const viewerHost = container.querySelector(".ivviewer");
    if (!stages().length) {
      renderSingleDoc(viewerHost);
      return;
    }
    const stage = selectedStage();
    const state = paneState();
    viewerHost.innerHTML = stageViewerHtml({
      stage,
      stages: stages(),
      paneState: state,
      docHtml: docHtmlFor(state),
      comments: (stage && stage.comments) || [],
    });
    if (stage) {
      const feedback = viewerHost.querySelector(".ivstagefeedback");
      if (feedback) feedback.innerHTML = commentLayer.trayHtml();
      mountDocMarkers(viewerHost, stage);
      commentLayer.attach(viewerHost, { annotatable: docAnnotatable(stage, state) });
      wireStageNav(viewerHost);
      wireStageActions(viewerHost, stage);
    }
  };

  /// The left column, kept rather than rewritten.
  ///
  /// Its regions never trade places — head, stages, assignment, lineage — and
  /// only the lineage comes and goes, at the end, so each is patched where it
  /// stands. The stage rows are keyed by stage id, so picking a stage redraws
  /// the two rows that changed and leaves the column, its scroll and the
  /// assignment control exactly where they were.
  const paintStageColumn = (listHost) => {
    const next = el(
      `<aside>${stageListHtml({
        task,
        stagesData,
        selectedStageId,
        assignment,
        assignmentOpen: Boolean(assignmentOverlay),
        worktrees: worktrees(),
        blockReason: implementBlockReason(task) || "",
        deletable: planDeletable(task.state),
      })}</aside>`,
    );
    keepRegion(listHost, ".ivhead", next);
    paintStageRows(listHost, next);
    keepRegion(listHost, ".ivassign", next);
    keepRegion(listHost, ".ivlineage", next);
  };

  /** Make the column's copy of `selector` say what a freshly rendered column
   *  says, putting it there when it is new and taking it away when it is gone. */
  const keepRegion = (listHost, selector, next) => {
    const source = next.querySelector(selector);
    const live = listHost.querySelector(selector);
    if (!source) {
      live?.remove();
      return;
    }
    if (live) patchElement(live, source);
    else listHost.append(source);
  };

  /** The stage rows, matched by stage id. The "no stages yet" line is chrome
   *  rather than a row, so it is placed and taken away by hand — a row is put
   *  after the last row, and nothing else may be sitting down there. */
  const paintStageRows = (listHost, next) => {
    const source = next.querySelector("#stagelist");
    const emptyLine = source.querySelector(".empty");
    let host = listHost.querySelector("#stagelist");
    if (!host) {
      host = source;
      host.replaceChildren();
      listHost.append(host);
    }
    const standing = host.querySelector(".empty");
    if (emptyLine && !standing) host.append(emptyLine);
    if (!emptyLine && standing) standing.remove();
    const list = stages();
    const ordinals = new Map(list.map((stage, index) => [stage.id, index]));
    patchList(host, list, {
      keyOf: (stage) => stage.id,
      render: (stage) => stageRowHtml(stage, { index: ordinals.get(stage.id), selected: stage.id === selectedStageId }),
    });
  };

  /** A task with no stage manifest still has a plan: render it in the viewer
   *  instead of the pick-a-stage line, and say so while the agent is still
   *  drafting one. It takes no comments — the bridge anchors a doc comment to a
   *  stage, and there is no stage to anchor to. */
  const renderSingleDoc = (viewerHost) => {
    // A created task is inert — the router files it and nothing runs until the
    // first message — so the two states say different things: only "drafting"
    // has an agent to speak of.
    if (task.state === "created") {
      viewerHost.innerHTML =
        '<div class="plan plan-loading">No plan yet — your first message starts the planning agent.</div>';
      return;
    }
    if (task.state === "drafting") {
      viewerHost.innerHTML = '<div class="plan plan-loading">✦ planning agent is drafting the plan…</div>';
      return;
    }
    const state = planDocPaneState({
      docsAvailable: task.docs_available,
      errorLatched: singleDocError,
      hasContents: Boolean(singleDoc),
    });
    viewerHost.innerHTML = `<div class="plan${state === "ready" ? " markdown" : ""}" id="stagedoc">${
      state === "ready" ? renderMarkdown(singleDoc)
      : state === "unavailable" ? `<div class="plan-empty">${esc(DOCS_UNAVAILABLE)}</div>`
      : state === "error" ? docErrorPaneHtml("plan")
      : '<div class="plan-loading">✦ loading the plan…</div>'
    }</div>`;
    const retry = viewerHost.querySelector("#docretry");
    if (retry)
      retry.onclick = () => {
        singleDocError = false;
        renderedKey = null;
        refresh();
      };
  };

  const docHtmlFor = (state) => {
    if (state === "ready") return renderMarkdown(docContents());
    if (state === "unavailable") return `<div class="plan-empty">${esc(DOCS_UNAVAILABLE)}</div>`;
    if (state === "error") return docErrorPaneHtml("stage");
    return '<div class="plan-loading">✦ loading stage document…</div>';
  };

  /** The doc comments already on this stage, hung in the margin of the heading
   *  each one anchors to. A comment naming no passage marks the doc's top. */
  const mountDocMarkers = (viewerHost, stage) => {
    const docEl = viewerHost.querySelector("#stagedoc");
    if (!docEl) return;
    for (const group of docMarkerGroups(stage.comments)) {
      const target = group.key ? headingForKey(docEl, group.key) : docEl.firstElementChild;
      if (!target) continue;
      const parts = docMarkerParts(group);
      const marker = document.createElement("button");
      marker.className = parts.className;
      marker.dataset.marker = group.key;
      marker.title = parts.title;
      marker.textContent = parts.label;
      target.prepend(marker);
    }
  };

  // ---- the left column's wiring --------------------------------------------

  // A stage row and a lineage row outlive the paints, and a row that has just
  // arrived has never been wired — so neither is wired at all: the surface's
  // one click handler reads which row was pressed off the DOM.
  const wireStageList = (listHost) => {
    bindAction(listHost.querySelector("#approvetask"), "approving…", async () => {
      if (!(await confirmAction(approvePlanConfirm()))) throw new Error("cancelled");
      await guarded(() => callRpc("task.approve", { task_id: taskId, ...MUTATION_THREAD_PAGE }));
      await refresh();
    });
    bindAction(listHost.querySelector("#approveall"), "approving…", async () => {
      await guarded(async () => {
        for (const stageId of plannedStageIds(stages())) {
          await callRpc("task.stage_approve", { task_id: taskId, stage_id: stageId, ...MUTATION_THREAD_PAGE });
        }
      });
      await refresh();
    });
    // The dispatch wears its own block reason, so a disabled one is disabled
    // because the render said so — there is nothing left to bind.
    const implementAll = listHost.querySelector("#implementall");
    if (implementAll && !implementAll.disabled) {
      bindAction(implementAll, "starting…", async () => {
        const target = worktrees().find((choice) => choice.id === assignment.worktreeId);
        const branch = assignment.worktree === "existing" && target ? target.label : null;
        if (
          !(await confirmAction(
            implementConfirm({ base: assignment.base || task.base_branch || "the base branch", branch }),
          ))
        )
          throw new Error("cancelled");
        afterDispatch(await openImplementation("task.implement_all", implementParams(taskId, assignment, { catalog })));
      });
    }
    wireAssignment(listHost);
    wireRemoval(listHost);
  };

  const wireRemoval = (listHost) => {
    bindAction(listHost.querySelector("#taskdelete"), "deleting…", async () => {
      if (!(await confirmAction(deletePlanConfirm()))) throw new Error("cancelled");
      gone = true;
      onGone();
      await runOptimistic({
        scope: INBOX_SCOPE,
        records: [removeRecord(entryKeyOf({ kind: "task", task_id: taskId, project_id: currentProjectId() }))],
        call: () => callRpc("task.delete", { task_id: taskId }),
        failureSummary: "Could not delete this task",
      });
      await refreshFeed();
    });
  };

  /** The rail's assignment line: one row saying what the handoff would be, which
   *  opens the overlay holding the fields. The overlay is painted from the same
   *  held assignment the dispatch reads, so a poll repaint of the rail — or of
   *  the overlay — restores exactly what was chosen. */
  const wireAssignment = (listHost) => {
    const toggle = listHost.querySelector("#assigntoggle");
    if (toggle) toggle.onclick = () => (assignmentOverlay ? assignmentOverlay.close() : openAssignment());
  };

  const openAssignment = () => {
    if (assignmentOverlay) return;
    // Both are re-read every time the overlay opens: a branch that appeared, or
    // was taken by another task, must be offered — or stop being.
    if (!catalog.providers) loadCatalogOnce();
    loadWorkItemsOnce();
    assignmentOverlay = openAssignmentOverlay({
      // The rail repaints on the poll, so the button this is anchored to is a
      // different node by the next tick: it is looked up, never held.
      getAnchor: () => container.querySelector("#assigntoggle"),
      getAssignment: () => assignment,
      setAssignment: (next) => {
        assignment = next;
        render();
      },
      getCatalog: () => catalog,
      getWorktrees: () => worktrees(),
      onClose: () => {
        assignmentOverlay = null;
        render();
      },
    });
    render();
  };

  let catalogLoading = false;
  const loadCatalogOnce = async () => {
    if (catalogLoading) return;
    catalogLoading = true;
    try {
      catalog = (await loadCatalog()) || {};
    } catch {
      catalog = {};
    }
    if (disposed) return;
    if (assignmentOverlay) assignmentOverlay.update();
    render();
  };

  /** The feed's work items, for the branch picker. Re-read every time the
   *  control opens: a branch that appeared (or was taken by another task)
   *  since the last look must be offered — or stop being. */
  let workItemsLoading = false;
  const loadWorkItemsOnce = async () => {
    if (workItemsLoading) return;
    workItemsLoading = true;
    try {
      workItems = (await loadWorkItems()) || [];
    } catch {
      workItems = [];
    }
    workItemsLoading = false;
    if (disposed) return;
    if (assignmentOverlay) assignmentOverlay.update();
    render();
  };

  // ---- the right column's wiring -------------------------------------------

  /** The doc pane's own way through the task: each step opens the stage it
   *  carries, which is the same selection a rail row makes — so the rail marks
   *  it, the host's URL follows it, and the drawer never has to be opened to
   *  read the next stage. */
  const wireStageNav = (viewerHost) => {
    viewerHost.querySelectorAll(".stagenav-step[data-stage]").forEach((step) => {
      step.onclick = () => selectStage(step.dataset.stage);
    });
  };

  /** The open stage's own actions: approve its plan while it is still planned,
   *  send its open comments back for a revision, implement just this stage, and
   *  read a completed stage's stable diff. */
  // eslint-disable-next-line complexity -- ratchet: this callback is at 20, cap 10 — reduce it, then drop this line
  const wireStageActions = (viewerHost, stage) => {
    const actions = viewerHost.querySelector("#stageactions");
    const stageHint = viewerHost.querySelector("#stagehint");
    if (!actions) return;
    const token = stageStateToken(stage);
    const openComments = stage.open_comments || 0;
    const notesTarget = stageNotesTarget(task);
    const parts = [];
    if (openComments > 0)
      parts.push(`<button class="btn" id="sendnotes">Send ${openComments} comment${openComments === 1 ? "" : "s"}</button>`);
    if (token === "complete" && stage.start_sha && stage.completion_sha)
      parts.push('<button class="btn" id="stagediff">View stable diff</button>');
    if (stageApprovable(stage)) parts.push('<button class="btn primary" id="approvestage">Approve stage plan</button>');
    else if (task.state === "approved" && (stage.execution || "pending") === "pending" && predecessorsComplete(stage))
      parts.push('<button class="btn primary" id="implementstage">Implement Stage</button>');
    actions.innerHTML = parts.join("");
    if (openStageDiffId === stage.id && stageDiff) {
      const pane = document.createElement("pre");
      pane.id = "stagediffpane";
      pane.className = "fsrc";
      pane.textContent =
        stageDiff.status === "available" ? stageDiff.patch || "No changes." : stageDiff.reason || "Stable diff unavailable.";
      viewerHost.querySelector("#stagedoc")?.after(pane);
    }

    const retry = viewerHost.querySelector("#stagedocretry");
    if (retry)
      retry.onclick = async () => {
        docErrors.delete(stage.id);
        stageDoc = null;
        renderedKey = null;
        // The reader asked for that doc again, so the record it is held under
        // goes: a retry that answered the cache would answer nothing new.
        await forgetTaskRecords(deviceId, taskId);
        await refresh();
      };

    bindAction(viewerHost.querySelector("#approvestage"), "approving…", async () => {
      await guarded(() =>
        callRpc("task.stage_approve", { task_id: taskId, stage_id: stage.id, ...MUTATION_THREAD_PAGE }),
      );
      await refresh();
    });
    bindAction(viewerHost.querySelector("#implementstage"), "starting…", async () => {
      afterDispatch(
        await openImplementation("task.implement_stage", implementParams(taskId, assignment, { catalog, stageId: stage.id })),
      );
    });
    const sendNotes = viewerHost.querySelector("#sendnotes");
    if (sendNotes) {
      if (!notesTarget) {
        sendNotes.disabled = true;
        sendNotes.title = "Task ready — start an implementation to revise this stage.";
      } else {
        bindAction(sendNotes, "sending…", async () => {
          const params =
            notesTarget.method === "run.stage_send_notes"
              ? { run_id: notesTarget.entityId, stage_id: stage.id, ...MUTATION_THREAD_PAGE }
              : { task_id: taskId, stage_id: stage.id, ...MUTATION_THREAD_PAGE };
          await guarded(() => callRpc(notesTarget.method, params));
          await refresh();
        });
      }
    }
    bindAction(viewerHost.querySelector("#stagediff"), "loading…", async () => {
      openStageDiffId = stage.id;
      const held = await readStoredTaskRecord(deviceId, taskId, `stagediff:${stage.id}`);
      if (held !== undefined) {
        stageDiff = held;
        renderedKey = null;
        render();
        return;
      }
      // The answer is ignored. Its cache announcement re-reads the record and
      // paints the pane through the normal view render.
      await onDemandRecord(`stagediff:${stage.id}`, () =>
        callRpc("task.stage_diff", { task_id: taskId, stage_id: stage.id }),
      );
    });
    if (stageHint) stageHint.textContent = token === "building" ? "Agent is working on this stage." : "";
  };

  const predecessorsComplete = (stage) => {
    const list = stages();
    const index = list.findIndex((candidate) => candidate.id === stage.id);
    return index >= 0 && list.slice(0, index).every((candidate) => stageStateToken(candidate) === "complete");
  };

  /** Ask the daemon to open an implementation, and answer with what it opened —
   *  or nothing, when the reply outlives the browser's timer. The task's own
   *  next answer names the run either way. */
  const openImplementation = (method, params) =>
    replyOrNothing(guarded(() => callRpc(method, { ...params, ...MUTATION_THREAD_PAGE })));

  const afterDispatch = (result) => {
    const runId = result && result.run_id;
    if (runId) navigate({ name: "run", projectId: currentProjectId(), id: runId, tab: "changes" });
    else refresh();
  };

  // ---- selection, clicks, polling ------------------------------------------

  const selectStage = (stageId) => {
    if (selectedStageId === stageId) return;
    selectedStageId = stageId;
    openStageDiffId = null;
    stageDiff = null;
    docErrors.delete(stageId);
    stageDoc = null;
    // A pending doc comment names a passage of the stage it was written on, and
    // the verb that posts it names that stage too — so it does not travel to
    // another one.
    commentLayer.clear();
    onSelectStage(stageId);
    renderedKey = null;
    render();
    refresh();
  };

  const handleClick = (event) => {
    if (commentLayer.handleClick(event)) return;
    const marker = event.target.closest(".docmarker, .cc-crumb");
    if (marker) {
      scrollToMarker(marker.dataset.marker, marker.classList.contains("docmarker"));
      return;
    }
    const remove = event.target.closest(".cc-x[data-del]");
    if (remove) {
      withdrawComment(remove.dataset.del);
      return;
    }
    const stageRow = event.target.closest(".stagerow[data-stage]");
    if (stageRow) {
      selectStage(stageRow.dataset.stage);
      return;
    }
    const lineageRow = event.target.closest(".ivlin-row[data-run]");
    if (lineageRow) {
      const route = lineageRoute({ run_id: lineageRow.dataset.run, branch: lineageRow.dataset.branch }, currentProjectId());
      if (route) navigate(route);
    }
  };

  /** A marker jumps to its comments; a comment's crumb jumps back to the
   *  passage — the two directions of the same anchor. */
  const scrollToMarker = (key, fromDoc) => {
    if (fromDoc) {
      const crumb = [...container.querySelectorAll(".stagecomments .commentcard .cc-crumb")].find(
        (element) => element.dataset.marker === (key || ""),
      );
      const target = crumb ? crumb.closest(".commentcard") : null;
      if (target) scrollWithin(container.querySelector(".ivviewer"), target, { block: "center" });
      return;
    }
    const heading = headingForKey(container, key);
    if (heading) scrollWithin(container.querySelector(".ivviewer"), heading, { block: "center" });
  };

  const withdrawComment = async (commentId) => {
    try {
      await guarded(() => callRpc("task.comment_delete", { task_id: taskId, comment_id: commentId }));
    } catch (e) {
      notifyError("Withdrawing the comment failed", (e && e.message) || "error");
      return;
    }
    renderedKey = null;
    await refresh();
  };

  /** The task was deleted out from under this view: latch a terminal state
   *  (nothing repaints over it, nothing else is fetched) with a way back. */
  const renderGone = () => {
    gone = true;
    if (assignmentOverlay) assignmentOverlay.close();
    if (drawer) {
      drawer.dispose();
      drawer = null;
    }
    const label = currentProjectId() ? "Back to project" : "Back to notifications";
    container.innerHTML = `<div class="taskview"><div class="empty gone">This Task no longer exists.<div><button class="btn" id="goneback">${esc(label)}</button></div></div></div>`;
    const back = container.querySelector("#goneback");
    if (back) back.onclick = () => onGone();
  };

  /** One pass of the surface's payloads. `force` bypasses the freeze (a repaint
   *  after the user's own action must land). */
  /** One of this task's records, read through the cache. `force` is a push
   *  (or a verb this surface sent) saying the record is behind. */
  const taskRecord = (sub, read, force = false, cacheOnly = false) =>
    cacheOnly
      ? readStoredTaskRecord(deviceId, taskId, sub)
      : readTaskRecord({ deviceId, taskId, sub, read, force });

  // What a word that the task moved leaves behind for the records this surface
  // fills on demand — the open stage's doc, a single-doc task's plan, a
  // stage's stable diff. The task and its manifest are re-read as that word
  // arrives; these are read when the reader asks for them, which can be long
  // after, so the word is kept as a count and spent the next time each is read.
  //
  // It has to be kept, because nothing else ever fills these records: a plan
  // revised while the reader has it open (the daemon's `StageDocEvent::Revised`,
  // legal from both planned and approved) is news no later word carries. A doc
  // not read again on this word is never read again at all.
  let wordCount = 0;
  const readAt = new Map(); // sub → the word count its record was last read at

  /** Whether a word has landed since this record was last read. A record this
   *  session has never read is not behind — it is the frame the surface is
   *  about to paint, and it comes off the disk. */
  const behind = (sub) => readAt.has(sub) && readAt.get(sub) < wordCount;

  /** One of the on-demand records, read through the cache and past it once a
   *  word says so. */
  const onDemandRecord = async (sub, read) => {
    const answer = await taskRecord(sub, read, behind(sub));
    readAt.set(sub, wordCount);
    return answer;
  };

  /** The task itself and its stage manifest. Cold, both are read; warm, both
   *  are answered off disk and the surface paints on the frame it mounted in. */
  const readTaskAndStages = (force, cacheOnly = false) =>
    Promise.all([
      taskRecord(
        "get",
        () =>
          callRpc("task.get", {
            task_id: taskId,
            ...agentSelection.scope(),
            // The only thing this surface takes off the answer's thread is how
            // far the conversation has got; the rail beside it owns what gets
            // rendered. So a read asks for the smallest page the daemon will
            // cut rather than naming no bound at all, which would ship every
            // item of a long conversation to compute one integer.
            ...SMALLEST_THREAD_PAGE,
          }),
        force,
        cacheOnly,
      ),
      taskRecord("stages", () => callRpc("task.stages", { task_id: taskId }), force, cacheOnly),
    ]);

  /**
   * Read what this surface paints and draw it.
   *
   * `reread` says the records are behind — a push naming the task, or a verb
   * this surface sent — and is what makes the read go to the machine rather
   * than answering off disk. `repaint` overrides the keyed freeze, which is
   * for the verb's own answer and nothing else: a push that carried no change
   * must leave the step the reader is aiming at, and the passage they are
   * selecting, exactly where they are.
   */
  // eslint-disable-next-line complexity -- ratchet: this callback is at 22, cap 10 — reduce it, then drop this line
  const load = async ({ reread = false, repaint = false, cacheOnly = false, cacheGeneration = 0 } = {}) => {
    const force = repaint;
    if (disposed || gone || (cacheGeneration && cacheGeneration !== cachePaintGeneration)) return;
    let payload;
    let stagesPayload;
    try {
      [payload, stagesPayload] = await readTaskAndStages(reread || repaint, cacheOnly);
    } catch (e) {
      // A deleted task is permanent; anything else is a machine that could not
      // answer, and what is held stays on screen until it can.
      if (/unknown (task_id|plan_id)/.test((e && e.message) || "")) renderGone();
      return;
    }
    if (disposed || gone || (cacheGeneration && cacheGeneration !== cachePaintGeneration)) return;
    if (!payload || !stagesPayload) return;
    const first = !task;
    task = payload;
    stagesData = stagesPayload || { stages: [] };
    if (first) assignment = defaultAssignment(payload);
    if (payload.project_id && payload.project_id !== project) {
      project = payload.project_id;
      onProject(project);
    }
    if (!selectionSeeded && stages().length) {
      selectionSeeded = true;
      const opening = openingStageId(stages(), selectedStageId);
      if (opening !== selectedStageId) {
        selectedStageId = opening;
        onSelectStage(opening);
      }
    }
    await loadStageDoc(cacheOnly);
    await loadSingleDoc(cacheOnly);
    await loadStageDiff();
    if (disposed || gone || (cacheGeneration && cacheGeneration !== cachePaintGeneration)) return;
    const key = `${taskViewKey({
      task,
      stagesData,
      selectedStageId,
      docState: paneState(),
      doc: stages().length ? docContents() : singleDoc || "",
    })}|${JSON.stringify([openStageDiffId, stageDiff])}`;
    const rendered = Boolean(container.querySelector(".ivsplit"));
    if (!force && rendered && (key === renderedKey || actionsInFlight > 0 || commentLayer.busy() || assignmentBusy())) {
      if (cacheOnly && key !== renderedKey) cacheDirty = true;
      return;
    }
    cacheDirty = false;
    renderedKey = key;
    render();
  };

  /** Fetch the open stage's doc when it can succeed — never for docs that
   *  predate canonical storage, never once a read has errored (latched off),
   *  and never again while the held doc is the open stage's. */
  const stageDocIsCurrent = (wanted) =>
    Boolean(stageDoc && stageDoc.stage_id === wanted && !behind(`stage:${wanted}`));
  const stageDocCanLoad = (wanted) =>
    shouldFetchPlanDoc({ docsAvailable: task?.docs_available, errorLatched: docErrors.has(wanted) });
  const keepStageDoc = (wanted, doc) => {
    if (!disposed && selectedStageId === wanted && doc !== undefined) stageDoc = doc;
  };
  const readStageDoc = [
    (wanted) => onDemandRecord(`stage:${wanted}`, () => callRpc("task.stage_doc", { task_id: taskId, stage_id: wanted })),
    (wanted) => readStoredTaskRecord(deviceId, taskId, `stage:${wanted}`),
  ];
  const loadStageDoc = async (cacheOnly = false) => {
    if (!selectedStageId) return;
    const wanted = selectedStageId;
    if (stageDocIsCurrent(wanted) || !stageDocCanLoad(wanted)) return;
    try {
      keepStageDoc(wanted, await readStageDoc[Number(cacheOnly)](wanted));
    } catch {
      docErrors.add(wanted); // latch: render an error state, stop refetching
    }
  };

  /** The single-doc task's plan, fetched once and latched off on error — the
   *  same discipline the stage docs get. */
  const singleDocCanLoad = () =>
    !stages().length &&
    (singleDoc === null || behind("doc")) &&
    shouldFetchPlanDoc({ docsAvailable: task?.docs_available, errorLatched: singleDocError });
  const readSingleDoc = [
    () => onDemandRecord("doc", () => callRpc("task.doc", { task_id: taskId })),
    () => readStoredTaskRecord(deviceId, taskId, "doc"),
  ];
  const loadSingleDoc = async (cacheOnly = false) => {
    if (!singleDocCanLoad()) return;
    try {
      const doc = await readSingleDoc[Number(cacheOnly)]();
      if (doc !== undefined) singleDoc = doc.contents || "";
    } catch {
      singleDocError = true;
    }
  };

  const loadStageDiff = async () => {
    if (!openStageDiffId) return;
    const held = await readStoredTaskRecord(deviceId, taskId, `stagediff:${openStageDiffId}`);
    if (held !== undefined) stageDiff = held;
  };

  /** Read the task again from its machine and draw the answer: a verb this
   *  surface sent moved something, and the records it paints from are behind. */
  const refresh = () => load({ reread: true, repaint: true });

  /** A word from outside: a push naming the task, or the mount catching up on
   *  what happened while the tab was shut.
   *
   *  It is the one thing that says the plan itself may have been rewritten
   *  under the reader — the daemon revises a stage doc from planned and from
   *  approved alike — so it puts the records this surface fills on demand
   *  behind as well as the two it always reads. A verb this surface sent is
   *  not such a word: it moved what it named, and the answer to it is what
   *  `refresh` already reads. */
  const readOnWord = () => {
    wordCount += 1;
    return load({ reread: true });
  };

  /**
   * The first paint, and the catch-up behind it.
   *
   * Warm, the records paint the frame this mounted in and the machine is read
   * straight after: the mount is the one moment this surface can catch up on
   * what happened while it was closed — an approval given from another device
   * has already happened, so no push will ever name it, and no pass fills
   * these records either. Cold, the first read IS the machine's, and asking
   * again would be the same answer twice.
   */
  const mountRead = async () => {
    const held = await taskRecordsHeld(deviceId, taskId, ["get", "stages"]);
    await load();
    if (held) await readOnWord();
  };

  function scheduleCachePaint() {
    if (disposed || gone) return;
    const generation = ++cachePaintGeneration;
    queueMicrotask(() => void load({ cacheOnly: true, cacheGeneration: generation }));
  }

  const cacheWatcher = subscribeCache(
    { deviceId: deviceId || "", entityId: taskId, kind: TASK_RECORD_KIND },
    (changed) => {
      const sub = changed.sub || "";
      if (sub === "doc") {
        singleDocError = false;
        singleDoc = null;
      } else if (sub.startsWith("stage:")) {
        const stageId = sub.slice("stage:".length);
        docErrors.delete(stageId);
        if (selectedStageId === stageId) stageDoc = null;
      } else if (sub === `stagediff:${openStageDiffId}`) {
        stageDiff = null;
      }
      scheduleCachePaint();
    },
  );
  void mountRead();
  // The task is the entity: its own plan/stage/thread mutations are what stale
  // this surface, and the push naming it is what says so. There is nothing
  // behind this to poll. `pausesWhileHidden: false` because this is the one
  // detail surface that keeps up while the tab is away.
  const watcher = watchChanges({
    refresh: readOnWord,
    entity: taskId,
    pausesWhileHidden: false,
    // Focus tier: a task is plan, stage and conversation — no checkout.
    kinds: ["state", "thread"],
    mode: "realtime",
  });

  return {
    // The surface's subscription, handed back so a host that follows the app's
    // App.poll convention can hold it too. dispose() ends it either way.
    poll: watcher,
    dispose() {
      disposed = true;
      watcher.dispose();
      cacheWatcher?.();
      if (assignmentOverlay) assignmentOverlay.close();
      commentLayer.dispose();
      if (drawer) {
        drawer.dispose();
        drawer = null;
      }
      container.onclick = null;
    },
  };
}
