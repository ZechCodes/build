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

import { esc } from "./text.js";
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
} from "./issueModel.js";
import { createDocCommentLayer, headingForKey } from "./issueDocComments.js";

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
 * catalog for the assignment control; `onSelectStage(stageId)` lets the host
 * keep the URL on the open stage; `onGone()` is called when the issue no longer
 * exists.
 */
export function mountIssueView(
  container,
  {
    issueId,
    projectId = null,
    callRpc,
    navigate = () => {},
    loadCatalog = async () => ({}),
    onSelectStage = () => {},
    onProject = () => {},
    onGone = () => {},
    pollMs = ISSUE_VIEW_POLL_MS,
    initialStageId = null,
  } = {},
) {
  let disposed = false;
  let gone = false;
  let issue = null;
  let stagesData = { stages: [] };
  let stageDoc = null; // { stage_id, contents } for the open stage
  let catalog = {};
  let selectedStageId = initialStageId;
  let selectionSeeded = false;
  let assignment = defaultAssignment(null);
  let assignmentOpen = false;
  let renderedKey = null;
  let actionsInFlight = 0;
  let threadCursor = 0;
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
      if (general) await callRpc("thread.post", { entity_id: issueId, body: general });
      renderedKey = null;
      await refresh();
    },
  });

  /** The user is filling in the assignment control: a repaint would replace the
   *  field under their cursor, so the poll waits. */
  const assignmentBusy = () => {
    if (!assignmentOpen) return false;
    const focused = document.activeElement;
    return Boolean(focused && container.contains(focused) && focused.closest(".ivassign"));
  };

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
    listHost.innerHTML = stageListHtml({
      issue,
      stagesData,
      selectedStageId,
      assignment,
      assignmentOpen,
      catalog,
    });
    wireStageList(listHost);
    const viewerHost = container.querySelector(".ivviewer");
    if (!stages().length) {
      renderSingleDoc(viewerHost);
      return;
    }
    const stage = selectedStage();
    const state = paneState();
    viewerHost.innerHTML = stageViewerHtml({
      stage,
      paneState: state,
      docHtml: docHtmlFor(state),
      comments: (stage && stage.comments) || [],
    });
    if (stage) {
      const feedback = viewerHost.querySelector(".ivstagefeedback");
      if (feedback) feedback.innerHTML = commentLayer.trayHtml();
      mountDocMarkers(viewerHost, stage);
      commentLayer.attach(viewerHost, { annotatable: docAnnotatable(stage, state) });
      wireStageActions(viewerHost, stage);
    }
  };

  /** An issue with no stage manifest still has a plan: render it in the viewer
   *  instead of the pick-a-stage line, and say so while the agent is still
   *  drafting one. It takes no comments — the bridge anchors a doc comment to a
   *  stage, and there is no stage to anchor to. */
  const renderSingleDoc = (viewerHost) => {
    if (issue.state === "created" || issue.state === "drafting") {
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

  const wireStageList = (listHost) => {
    listHost.querySelectorAll(".stagerow[data-stage]").forEach((row) => {
      row.onclick = () => selectStage(row.dataset.stage);
    });
    listHost.querySelectorAll(".ivlin-row[data-run]").forEach((row) => {
      row.onclick = () => {
        const route = lineageRoute({ run_id: row.dataset.run, branch: row.dataset.branch }, currentProjectId());
        if (route) navigate(route);
      };
    });
    bindAction(listHost.querySelector("#approveissue"), "approving…", async () => {
      if (!(await confirmAction(approvePlanConfirm()))) throw new Error("cancelled");
      await guarded(() => callRpc("issue.approve", { issue_id: issueId }));
      await refresh();
    });
    bindAction(listHost.querySelector("#approveall"), "approving…", async () => {
      await guarded(async () => {
        for (const stageId of plannedStageIds(stages())) {
          await callRpc("issue.stage_approve", { issue_id: issueId, stage_id: stageId });
        }
      });
      await refresh();
    });
    const implementAll = listHost.querySelector("#implementall");
    if (implementAll && !implementAll.disabled) {
      const blocked = implementBlockReason(issue);
      if (blocked) {
        implementAll.disabled = true;
        implementAll.title = blocked;
      } else {
        bindAction(implementAll, "starting…", async () => {
          if (!(await confirmAction(implementConfirm({ base: assignment.base || issue.base_branch || "the base branch" }))))
            throw new Error("cancelled");
          const result = await guarded(() =>
            callRpc("issue.implement_all", implementParams(issueId, assignment, { models: providerModels() })),
          );
          afterDispatch(result);
        });
      }
    }
    wireAssignment(listHost);
    wireRemoval(listHost);
  };

  const wireRemoval = (listHost) => {
    if (!planDeletable(issue.state)) return;
    const gate = listHost.querySelector(".ivgate");
    if (!gate) return;
    gate.insertAdjacentHTML("afterbegin", '<button class="btn danger mini" id="issuedelete">Delete</button>');
    bindAction(listHost.querySelector("#issuedelete"), "deleting…", async () => {
      if (!(await confirmAction(deletePlanConfirm()))) throw new Error("cancelled");
      await guarded(() => callRpc("issue.delete", { issue_id: issueId }));
      gone = true;
      onGone();
    });
  };

  const providerModels = () => {
    const providers = (catalog && catalog.providers) || [];
    const entry = providers.find((provider) => provider.id === assignment.provider) || providers[0];
    return (entry && entry.models) || [];
  };

  /** The assignment control: a summary that opens onto the two targets and the
   *  overrides. Every field writes straight into the held assignment, so a poll
   *  repaint restores exactly what was chosen. */
  const wireAssignment = (listHost) => {
    const toggle = listHost.querySelector("#assigntoggle");
    if (toggle)
      toggle.onclick = () => {
        assignmentOpen = !assignmentOpen;
        if (assignmentOpen && !catalog.providers) loadCatalogOnce();
        render();
      };
    const field = (id, key) => {
      const element = listHost.querySelector(id);
      if (!element) return;
      element.onchange = () => {
        assignment = { ...assignment, [key]: element.value };
        render();
      };
      if (element.tagName === "INPUT")
        element.oninput = () => {
          assignment = { ...assignment, [key]: element.value };
        };
    };
    field("#assignworktree", "worktree");
    field("#assignagent", "agent");
    field("#assignbase", "base");
    field("#assignprovider", "provider");
    field("#assignmodel", "model");
    field("#assigneffort", "effort");
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
    if (!disposed) render();
  };

  // ---- the right column's wiring -------------------------------------------

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
      await guarded(() => callRpc("issue.stage_approve", { issue_id: issueId, stage_id: stage.id }));
      await refresh();
    });
    bindAction(viewerHost.querySelector("#implementstage"), "starting…", async () => {
      const result = await guarded(() =>
        callRpc("issue.implement_stage", implementParams(issueId, assignment, { models: providerModels(), stageId: stage.id })),
      );
      afterDispatch(result);
    });
    bindAction(viewerHost.querySelector("#fixstage"), "sending…", async () => {
      await guarded(() => callRpc("issue.stage_fix", { issue_id: issueId, stage_id: stage.id, note: "" }));
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
              ? { run_id: notesTarget.entityId, stage_id: stage.id }
              : { issue_id: issueId, stage_id: stage.id };
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
    try {
      [payload, stagesPayload] = await Promise.all([
        callRpc("issue.get", { issue_id: issueId, ...(threadCursor ? { thread_after_sequence: threadCursor } : {}) }),
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
  const timer = setInterval(() => load(), pollMs);

  return {
    // The surface's own poll, handed back so a host that follows the app's
    // App.poll convention can hold it too. dispose() clears it either way.
    poll: timer,
    dispose() {
      disposed = true;
      clearInterval(timer);
      commentLayer.dispose();
      if (drawer) {
        drawer.dispose();
        drawer = null;
      }
      container.onclick = null;
    },
  };
}
