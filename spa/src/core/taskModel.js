// The legacy plan page's decisions, apart from the DOM.
//
// A plan is a work item whose children are its stages and the implementations
// that carried them out. Everything the read-only surface has to decide from a
// payload — what state a stage is in, which stage it opens on, which heading a
// comment hangs off, and where an implementation's branch lives — is decided
// here, so the two-column view is only markup and wiring.
//
// Pure: no DOM, no RPC, no module state.

import { slugifyHeading } from "./anchors.js";

/** The one vocabulary the stage list speaks: the doc's approval until the agent
 *  starts, then what execution has reached. `complete` is the end of the line. */
export const STAGE_STATE_LABEL = {
  planned: "PLANNED",
  approved: "APPROVED",
  building: "BUILDING",
  complete: "COMPLETE",
  blocked: "BLOCKED",
  failed: "FAILED",
  incomplete: "INCOMPLETE",
};

// A completed stage is complete whether or not its stable diff can still be
// pinned — "COMPLETE · DIFF UNAVAILABLE" was a storage detail wearing a state's
// clothes, and the diff button's absence already says it.
const COMPLETED_EXECUTIONS = new Set(["complete", "legacy_unpinned"]);

/** One stage's state token. Approval (planned/approved) is the answer while
 *  execution is still pending — after that the execution is the state, because
 *  an approved stage that is building is not "approved" to anyone looking. */
export function stageStateToken(stage) {
  if (!stage) return "planned";
  const execution = stage.execution || "pending";
  if (COMPLETED_EXECUTIONS.has(execution)) return "complete";
  if (execution !== "pending") return execution;
  return stage.approval || stage.state || "planned";
}

/** The chip palette (styles.css .chip.*): a drafted stage is neutral, an
 *  approved one awaits the human, in-flight work reads as work, a complete
 *  stage is done, and every parked arm warns. */
export function stageStateChipClass(token) {
  if (token === "approved") return "attn";
  if (token === "building") return "work";
  if (token === "complete") return "done";
  if (token === "planned") return "";
  return "warn";
}

/** Whether this stage's plan was still awaiting approval. */
export function stageApprovable(stage) {
  return Boolean(stage) && (stage.approval || stage.state) === "planned";
}

/**
 * Where the open stage sits among the task's stages, and which stages are
 * either side of it: { index, total, previous, next }. `index` is 0-based, or
 * -1 when nothing is open (or the open stage is no longer in the manifest);
 * either neighbour is null at that end of the list. Order is board order, which
 * is the order the stages are built in.
 */
export function stageNeighbors(stages, selectedStageId) {
  const list = stages || [];
  const index = list.findIndex((stage) => stage && stage.id === selectedStageId);
  if (index < 0) return { index: -1, total: list.length, previous: null, next: null };
  return {
    index,
    total: list.length,
    previous: index > 0 ? list[index - 1] : null,
    next: index < list.length - 1 ? list[index + 1] : null,
  };
}

/**
 * The doc's comments gathered into the markers the viewer hangs in its margin:
 * one group per anchored heading (keyed by that heading's slug, which is the id
 * markdown.js gave the rendered heading), plus one keyed "" for comments that
 * name no passage. Order is first appearance, so the margin reads the way the
 * conversation happened.
 */
export function docMarkerGroups(comments) {
  const groups = new Map();
  for (const comment of comments || []) {
    const headingPath = (comment.anchor && comment.anchor.heading_path) || [];
    const key = headingPath.length ? slugifyHeading(headingPath[headingPath.length - 1]) : "";
    if (!groups.has(key)) groups.set(key, { key, headingPath, comments: [], open: 0, total: 0 });
    const group = groups.get(key);
    group.comments.push(comment);
    group.total += 1;
    if (comment.state !== "addressed") group.open += 1;
  }
  return [...groups.values()];
}

/** Where an implementation in the lineage lives: its branch's Changes surface
 *  (`#/project/<id>/branch/<name>/changes`). An implementation the bridge
 *  names no branch for has nowhere to open — the row reads, but does not
 *  navigate. */
export function lineageRoute(implementation, projectId) {
  if (!implementation || !implementation.branch || !projectId) return null;
  return { name: "branch", projectId, branch: implementation.branch, tab: "changes" };
}

/** The repaint-freeze key for one pass: everything the two columns draw. An
 *  unchanged key leaves the DOM — and the reader's selection and scroll —
 *  alone. */
export function taskViewKey({ task, stagesData, selectedStageId, docState, doc }) {
  return JSON.stringify([
    task.state,
    task.active_run_id || null,
    task.docs_available,
    task.implementation_activity || null,
    task.implementation_lineage || [],
    (stagesData && stagesData.stages) || [],
    selectedStageId || null,
    docState,
    (doc || "").length,
    doc || "",
  ]);
}
