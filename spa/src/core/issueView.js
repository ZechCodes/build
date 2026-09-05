// The issue surface: a persistent two-column stages | stage viewer.
//
// An issue is a work item whose children are its stages and the implementations
// that carried them out, so the left column is the issue itself — its stages
// with their states, the gate and approve actions, the worktree/agent
// assignment control, and its implementation lineage — and the right column is
// whichever stage doc is open. There are no tabs and no drill-in: selecting a
// stage swaps the viewer and the list stays where it is.
//
// Owns a 1.6s issue.get + issue.stages poll with a keyed freeze; the freeze also
// holds while the reviewer is mid-comment or an action is in flight. The pure
// pieces live in issueModel.js (decisions) and issueRender.js (markup); this
// file is the wiring.

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
} from "./issueRender.js";
import {
  defaultAssignment,
  docMarkerGroups,
  implementParams,
  issueViewKey,
  lineageRoute,
  plannedStageIds,
  stageApprovable,
  stageStateToken,
  worktreeChoices,
} from "./issueModel.js";
import { createDocCommentLayer, headingForKey } from "./issueDocComments.js";
import { MUTATION_THREAD_PAGE, SMALLEST_THREAD_PAGE } from "./thread.js";
import { patchElement } from "./domPatch.js";
import { patchList } from "./patchList.js";
import { watchChanges } from "./changeEvents.js";
import { refreshFeed } from "./taskFeed.js";
import { entryKeyOf } from "./inbox.js";
import { INBOX_SCOPE } from "./inboxView.js";
import { removeRecord, runOptimistic } from "./optimistic.js";
import { rpcTimedOut } from "./session.js";

export const ISSUE_VIEW_POLL_MS = 1600;

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
 * mountIssueView(container, options) → { dispose }.
 *
 * `issueId` names the issue; `callRpc(method, params)` is the RPC channel;
 * `navigate(route)` opens another surface; `loadCatalog()` resolves the model
 * catalog for the assignment control and `loadWorkItems()` its feed rows (the
 * branches an implementation can be sent into); `onSelectStage(stageId)` lets
 * the host keep the URL on the open stage; `onGone()` is called when the issue
 * no longer exists.
 */
export function mountIssueView(
  container,
  {
    issueId,
    projectId = null,
    callRpc,
    navigate = () => {},
    loadCatalog = async () => ({}),
    loadWorkItems = async () => [],
    onSelectStage = () => {},
    onProject = () => {},
    onGone = () => {},
    pollMs = ISSUE_VIEW_POLL_MS,
    initialStageId = null,
    // Whose conversation this surface is reading and writing into. An issue
    // carries exactly one agent session, so this all but always names it — but
    // it is the rail's bubble that says so, and the poll asks with it.
    agentSelection = createAgentSelection(),
  } = {},
) {
  let disposed = false;
  let gone = false;
  let issue = null;
  let stagesData = { stages: [] };
  let stageDoc = null; // { stage_id, contents } for the open stage
  let catalog = {};
  let workItems = [];
  let selectedStageId = initialStageId;
  let selectionSeeded = false;
  let assignment = defaultAssignment(null);
  let assignmentOverlay = null;
  let renderedKey = null;
  let actionsInFlight = 0;
  let threadCursor = 0;
  let threadAgentId = agentSelection.get(); // whose conversation the cursor is in
  let drawer = null;
  // The plan of an issue that has no stage manifest at all (a migrated issue
  // predating stages): one doc, read-only, so it is still readable here.
  let singleDoc = null;
  let singleDocError = false;
  // Per-stage doc-read latches: a stage_doc ERROR renders an error state and
  // stops that doc's refetch until the user re-selects it.
  const docErrors = new Set();

  container.innerHTML = '<div class="issueview"><div class="empty">loading…</div></div>';

  let project = projectId;
  const currentProjectId = () => project;
  /** The branches this issue's implementation could be sent into. */
  const worktrees = () => worktreeChoices(workItems, { projectId: currentProjectId(), issueId });
  const stages = () => stagesData.stages || [];
  const selectedStage = () => stages().find((stage) => stage.id === selectedStageId) || null;
  const docContents = () => (stageDoc && stageDoc.stage_id === selectedStageId ? stageDoc.contents || "" : "");
  const paneState = () =>
    selectedStageId
      ? planDocPaneState({
          docsAvailable: issue && issue.docs_available,
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
      for (const comment of comments) {
        await callRpc("issue.comment_add", {
          issue_id: issueId,
          stage_id: stage.id,
          body: comment.comment,
          anchor: docCommentAnchor(comment),
        });
      }
      if (general)
        await callRpc("thread.post", {
          entity_id: issueId,
          ...agentSelection.scope(),
          body: general,
          ...MUTATION_THREAD_PAGE,
        });
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
    }
  };

  // ---- the persistent skeleton ---------------------------------------------

  const paintSkeleton = () => {
    container.innerHTML = `<div class="issueview">
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

  const render = () => {
    if (disposed || gone || !issue) return;
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
        issue,
        stagesData,
        selectedStageId,
        assignment,
        assignmentOpen: Boolean(assignmentOverlay),
        worktrees: worktrees(),
        blockReason: implementBlockReason(issue) || "",
        deletable: planDeletable(issue.state),
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

  /** An issue with no stage manifest still has a plan: render it in the viewer
   *  instead of the pick-a-stage line, and say so while the agent is still
   *  drafting one. It takes no comments — the bridge anchors a doc comment to a
   *  stage, and there is no stage to anchor to. */
  const renderSingleDoc = (viewerHost) => {
    // A created issue is inert — the router files it and nothing runs until the
    // first message — so the two states say different things: only "drafting"
    // has an agent to speak of.
    if (issue.state === "created") {
      viewerHost.innerHTML =
        '<div class="plan plan-loading">No plan yet — your first message starts the planning agent.</div>';
      return;
    }
    if (issue.state === "drafting") {
      viewerHost.innerHTML = '<div class="plan plan-loading">✦ planning agent is drafting the plan…</div>';
      return;
    }
    const state = planDocPaneState({
      docsAvailable: issue.docs_available,
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
    bindAction(listHost.querySelector("#approveissue"), "approving…", async () => {
      if (!(await confirmAction(approvePlanConfirm()))) throw new Error("cancelled");
      await guarded(() => callRpc("issue.approve", { issue_id: issueId, ...MUTATION_THREAD_PAGE }));
      await refresh();
    });
    bindAction(listHost.querySelector("#approveall"), "approving…", async () => {
      await guarded(async () => {
        for (const stageId of plannedStageIds(stages())) {
          await callRpc("issue.stage_approve", { issue_id: issueId, stage_id: stageId, ...MUTATION_THREAD_PAGE });
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
            implementConfirm({ base: assignment.base || issue.base_branch || "the base branch", branch }),
          ))
        )
          throw new Error("cancelled");
        afterDispatch(await openImplementation("issue.implement_all", implementParams(issueId, assignment, { catalog })));
      });
    }
    wireAssignment(listHost);
    wireRemoval(listHost);
  };

  const wireRemoval = (listHost) => {
    bindAction(listHost.querySelector("#issuedelete"), "deleting…", async () => {
      if (!(await confirmAction(deletePlanConfirm()))) throw new Error("cancelled");
      gone = true;
      onGone();
      await runOptimistic({
        scope: INBOX_SCOPE,
        records: [removeRecord(entryKeyOf({ kind: "issue", issue_id: issueId, project_id: currentProjectId() }))],
        call: () => callRpc("issue.delete", { issue_id: issueId }),
        failureSummary: "Could not delete this issue",
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
    // was taken by another issue, must be offered — or stop being.
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
   *  control opens: a branch that appeared (or was taken by another issue)
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

  /** The doc pane's own way through the issue: each step opens the stage it
   *  carries, which is the same selection a rail row makes — so the rail marks
   *  it, the host's URL follows it, and the drawer never has to be opened to
   *  read the next stage. */
  const wireStageNav = (viewerHost) => {
    viewerHost.querySelectorAll(".stagenav-step[data-stage]").forEach((step) => {
      step.onclick = () => selectStage(step.dataset.stage);
    });
  };

  /** The open stage's own actions: approve its plan while it is still planned,
   *  send its open comments back for a revision, implement just this stage, send
   *  a failed one back to fix, and read a completed stage's stable diff. */
  const wireStageActions = (viewerHost, stage) => {
    const actions = viewerHost.querySelector("#stageactions");
    const stageHint = viewerHost.querySelector("#stagehint");
    if (!actions) return;
    const token = stageStateToken(stage);
    const openComments = stage.open_comments || 0;
    const notesTarget = stageNotesTarget(issue);
    const parts = [];
    if (openComments > 0)
      parts.push(`<button class="btn" id="sendnotes">Send ${openComments} comment${openComments === 1 ? "" : "s"}</button>`);
    if (token === "validated" && stage.start_sha && stage.completion_sha)
      parts.push('<button class="btn" id="stagediff">View stable diff</button>');
    if (token === "validation_failed") parts.push('<button class="btn primary" id="fixstage">Send stage back to fix</button>');
    if (stageApprovable(stage)) parts.push('<button class="btn primary" id="approvestage">Approve stage plan</button>');
    else if (issue.state === "approved" && (stage.execution || "pending") === "pending" && predecessorsComplete(stage))
      parts.push('<button class="btn primary" id="implementstage">Implement Stage</button>');
    actions.innerHTML = parts.join("");

    const retry = viewerHost.querySelector("#stagedocretry");
    if (retry)
      retry.onclick = () => {
        docErrors.delete(stage.id);
        stageDoc = null;
        renderedKey = null;
        refresh();
      };

    bindAction(viewerHost.querySelector("#approvestage"), "approving…", async () => {
      await guarded(() =>
        callRpc("issue.stage_approve", { issue_id: issueId, stage_id: stage.id, ...MUTATION_THREAD_PAGE }),
      );
      await refresh();
    });
    bindAction(viewerHost.querySelector("#implementstage"), "starting…", async () => {
      afterDispatch(
        await openImplementation("issue.implement_stage", implementParams(issueId, assignment, { catalog, stageId: stage.id })),
      );
    });
    bindAction(viewerHost.querySelector("#fixstage"), "sending…", async () => {
      await guarded(() =>
        callRpc("issue.stage_fix", { issue_id: issueId, stage_id: stage.id, note: "", ...MUTATION_THREAD_PAGE }),
      );
      await refresh();
    });
    const sendNotes = viewerHost.querySelector("#sendnotes");
    if (sendNotes) {
      if (!notesTarget) {
        sendNotes.disabled = true;
        sendNotes.title = "Issue ready — start an implementation to revise this stage.";
      } else {
        bindAction(sendNotes, "sending…", async () => {
          const params =
            notesTarget.method === "run.stage_send_notes"
              ? { run_id: notesTarget.entityId, stage_id: stage.id, ...MUTATION_THREAD_PAGE }
              : { issue_id: issueId, stage_id: stage.id, ...MUTATION_THREAD_PAGE };
          await guarded(() => callRpc(notesTarget.method, params));
          await refresh();
        });
      }
    }
    bindAction(viewerHost.querySelector("#stagediff"), "loading…", async () => {
      const diff = await callRpc("issue.stage_diff", { issue_id: issueId, stage_id: stage.id });
      let pane = viewerHost.querySelector("#stagediffpane");
      if (!pane) {
        pane = document.createElement("pre");
        pane.id = "stagediffpane";
        pane.className = "fsrc";
        viewerHost.querySelector("#stagedoc")?.after(pane);
      }
      pane.textContent = diff.status === "available" ? diff.patch || "No changes." : diff.reason || "Stable diff unavailable.";
    });
    if (stageHint)
      stageHint.textContent =
        token === "validation_failed" ? "Validation failed — send the stage back to fix."
        : ["building", "built", "validating"].includes(token) ? "Agent is working on this stage."
        : "";
  };

  const predecessorsComplete = (stage) => {
    const list = stages();
    const index = list.findIndex((candidate) => candidate.id === stage.id);
    return index >= 0 && list.slice(0, index).every((candidate) => stageStateToken(candidate) === "validated");
  };

  /** Ask the daemon to open an implementation, and answer with what it opened —
   *  or nothing, when the reply outlives the browser's timer. Cutting the
   *  checkout runs with the daemon's state lock released, so the answer can
   *  arrive after this browser has stopped waiting for it; the issue's own next
   *  answer names the run either way. */
  const openImplementation = async (method, params) => {
    try {
      return await guarded(() => callRpc(method, { ...params, ...MUTATION_THREAD_PAGE }));
    } catch (error) {
      if (!rpcTimedOut(error)) throw error;
      return null;
    }
  };

  const afterDispatch = (result) => {
    const runId = result && result.run_id;
    if (runId) navigate({ name: "task", projectId: currentProjectId(), id: runId, tab: "changes" });
    else refresh();
  };

  // ---- selection, clicks, polling ------------------------------------------

  const selectStage = (stageId) => {
    if (selectedStageId === stageId) return;
    selectedStageId = stageId;
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
      if (target && target.scrollIntoView) target.scrollIntoView({ behavior: "smooth", block: "center" });
      return;
    }
    const heading = headingForKey(container, key);
    if (heading && heading.scrollIntoView) heading.scrollIntoView({ behavior: "smooth", block: "center" });
  };

  const withdrawComment = async (commentId) => {
    try {
      await guarded(() => callRpc("issue.comment_delete", { issue_id: issueId, comment_id: commentId }));
    } catch (e) {
      notifyError("Withdrawing the comment failed", (e && e.message) || "error");
      return;
    }
    renderedKey = null;
    await refresh();
  };

  /** The issue was deleted out from under this view: latch a terminal state
   *  (nothing repaints over it, nothing else is fetched) with a way back. */
  const renderGone = () => {
    gone = true;
    if (assignmentOverlay) assignmentOverlay.close();
    if (drawer) {
      drawer.dispose();
      drawer = null;
    }
    const label = currentProjectId() ? "Back to project" : "Back to notifications";
    container.innerHTML = `<div class="issueview"><div class="empty gone">This Issue no longer exists.<div><button class="btn" id="goneback">${esc(label)}</button></div></div></div>`;
    const back = container.querySelector("#goneback");
    if (back) back.onclick = () => onGone();
  };

  /** One pass of the surface's payloads. `force` bypasses the freeze (a repaint
   *  after the user's own action must land). */
  const load = async (force = false) => {
    if (disposed || gone) return;
    let payload;
    let stagesPayload;
    // A cursor is a position in ONE conversation: opening a different agent's
    // makes what this view holds somebody else's, so the next read is whole.
    if (agentSelection.get() !== threadAgentId) {
      threadAgentId = agentSelection.get();
      threadCursor = 0;
    }
    try {
      [payload, stagesPayload] = await Promise.all([
        callRpc("issue.get", {
          issue_id: issueId,
          ...agentSelection.scope(),
          // The only thing this surface takes off the answer's thread is how
          // far the conversation has got; the rail beside it owns what gets
          // rendered. So a read with no cursor yet asks for the smallest page
          // the daemon will cut rather than naming no bound at all, which
          // would ship every item of a long conversation to compute one
          // integer — on the first read of every issue, and again after every
          // bubble switch.
          ...(threadCursor ? { thread_after_sequence: threadCursor } : SMALLEST_THREAD_PAGE),
        }),
        callRpc("issue.stages", { issue_id: issueId }),
      ]);
    } catch (e) {
      // A deleted issue is permanent; every other failure is transient and the
      // poll retries silently.
      if (/unknown (issue_id|plan_id)/.test((e && e.message) || "")) renderGone();
      return;
    }
    if (disposed || gone) return;
    threadCursor = threadSequence(payload) || threadCursor;
    const first = !issue;
    issue = payload;
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
    await loadStageDoc();
    await loadSingleDoc();
    if (disposed || gone) return;
    const key = issueViewKey({
      issue,
      stagesData,
      selectedStageId,
      docState: paneState(),
      doc: stages().length ? docContents() : singleDoc || "",
    });
    const rendered = Boolean(container.querySelector(".ivsplit"));
    if (!force && rendered && (key === renderedKey || actionsInFlight > 0 || commentLayer.busy() || assignmentBusy())) return;
    renderedKey = key;
    render();
  };

  /** Fetch the open stage's doc when it can succeed — never for docs that
   *  predate canonical storage, never once a read has errored (latched off),
   *  and never again while the held doc is the open stage's. */
  const loadStageDoc = async () => {
    if (!selectedStageId) return;
    if (stageDoc && stageDoc.stage_id === selectedStageId) return;
    if (!shouldFetchPlanDoc({ docsAvailable: issue && issue.docs_available, errorLatched: docErrors.has(selectedStageId) })) return;
    const wanted = selectedStageId;
    try {
      const doc = await callRpc("issue.stage_doc", { issue_id: issueId, stage_id: wanted });
      if (!disposed && selectedStageId === wanted) stageDoc = doc;
    } catch {
      docErrors.add(wanted); // latch: render an error state, stop refetching
    }
  };

  /** The single-doc issue's plan, fetched once and latched off on error — the
   *  same discipline the stage docs get. */
  const loadSingleDoc = async () => {
    if (stages().length || singleDoc !== null) return;
    if (!shouldFetchPlanDoc({ docsAvailable: issue && issue.docs_available, errorLatched: singleDocError })) return;
    try {
      singleDoc = (await callRpc("issue.doc", { issue_id: issueId })).contents || "";
    } catch {
      singleDocError = true;
    }
  };

  const threadSequence = (payload) => {
    const thread = payload && payload.thread;
    if (!thread) return 0;
    if (thread.thread_last_sequence) return thread.thread_last_sequence;
    return (thread.items || []).reduce(
      (highest, item) => Math.max(highest, Number(item.data && item.data.sequence) || 0),
      0,
    );
  };

  const refresh = () => load(true);

  load();
  // The issue is the entity: its own plan/stage/thread mutations are what stale
  // this surface. `pausesWhileHidden: false` keeps the events on exactly the
  // footing this poll has always had — it is the one detail surface that reads
  // while the tab is away, and a push must not do less than the tick it stood
  // down.
  const watcher = watchChanges({
    refresh: () => load(),
    intervalMs: pollMs,
    entity: issueId,
    pausesWhileHidden: false,
  });

  return {
    // The surface's own poll, handed back so a host that follows the app's
    // App.poll convention can hold it too. dispose() ends it either way.
    poll: watcher,
    dispose() {
      disposed = true;
      watcher.dispose();
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
