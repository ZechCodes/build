// The issue view's decisions, apart from the DOM.
//
// An issue is a work item whose children are its stages and the implementations
// that carried them out. Everything the surface has to decide from a payload —
// what state a stage is in, which stages an approve-all would touch, where a
// selected passage sits in the source doc, which heading a comment hangs off,
// where an implementation's branch lives, and what an implement dispatch sends
// — is decided here, so the two-column view below is only markup and wiring.
//
// Pure: no DOM, no RPC, no module state.

import { slugifyHeading } from "./anchors.js";
import { modelParams } from "./modelPicker.js";

/** The one vocabulary the stage list speaks: the doc's approval until the agent
 *  starts, then what execution has reached. `validated` is the end of the line. */
export const STAGE_STATE_LABEL = {
  planned: "PLANNED",
  approved: "APPROVED",
  building: "BUILDING",
  built: "BUILT",
  validating: "VALIDATING",
  validated: "VALIDATED",
  validation_failed: "VALIDATION FAILED",
  blocked: "BLOCKED",
  failed: "FAILED",
  incomplete: "INCOMPLETE",
};

// A completed stage is validated whether or not its stable diff can still be
// pinned — "COMPLETE · DIFF UNAVAILABLE" was a storage detail wearing a state's
// clothes, and the diff button's absence already says it.
const COMPLETED_EXECUTIONS = new Set(["complete", "legacy_unpinned"]);

/** One stage's state token. Approval (planned/approved) is the answer while
 *  execution is still pending — after that the execution is the state, because
 *  an approved stage that is building is not "approved" to anyone looking. */
export function stageStateToken(stage) {
  if (!stage) return "planned";
  const execution = stage.execution || "pending";
  if (COMPLETED_EXECUTIONS.has(execution)) return "validated";
  if (execution !== "pending") return execution;
  return stage.approval || stage.state || "planned";
}

/** The chip palette (styles.css .chip.*): a drafted stage is neutral, an
 *  approved one awaits the human, in-flight work reads as work, a validated
 *  stage is done, and every parked arm warns. */
export function stageStateChipClass(token) {
  if (token === "approved") return "attn";
  if (token === "building" || token === "built" || token === "validating") return "work";
  if (token === "validated") return "done";
  if (token === "planned") return "";
  return "warn";
}

/** The stages an approve-all sweep would touch, in board order. */
export function plannedStageIds(stages) {
  return (stages || []).filter(stageApprovable).map((stage) => stage.id);
}

/** Whether this stage's plan is still awaiting approval. */
export function stageApprovable(stage) {
  return Boolean(stage) && (stage.approval || stage.state) === "planned";
}

/**
 * Where the open stage sits among the issue's stages, and which stages are
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
 * Where a selected passage sits in the stage doc's SOURCE, 1-based and
 * inclusive, or null when the passage cannot be found there.
 *
 * The viewer renders markdown, so a selection carries rendered text; matching
 * it back to source lines is what lets a doc comment name a line range the way
 * a diff comment does. A passage whose markers were rendered away (bold, links)
 * simply has no range — the anchor still carries its heading path and snippet,
 * which is what actually resolves it.
 */
export function docLineRange(text, snippet) {
  const lines = String(text || "").split("\n");
  const wanted = String(snippet || "")
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean);
  if (!text || !wanted.length) return null;
  const start = lines.findIndex((line) => line.includes(wanted[0]));
  if (start < 0) return null;
  const last = wanted[wanted.length - 1];
  let end = start;
  for (let i = start; i < lines.length; i++) {
    if (lines[i].includes(last)) {
      end = i;
      break;
    }
  }
  return { line_start: start + 1, line_end: end + 1 };
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

// ---- the worktree/agent assignment control ---------------------------------
// The issue's handoff: which checkout the implementation runs in, and which
// agent runs it. The checkout is a real choice — a new `build/<slug>` or a
// branch that already exists, which the bridge implements into by having that
// branch's run adopt the implementation. The agent is not: implementation is
// always a handoff, so the bridge creates the implementing agent itself and no
// conversation already in flight can be given the build. That option is offered
// and refused rather than hidden, because the rule is worth saying out loud.

const AGENT_IS_ALWAYS_NEW =
  "Implementation always starts a fresh agent — the handoff is the point, so a conversation already in flight cannot take the build.";

export const WORKTREE_TARGETS = [
  { id: "new", label: "New worktree", supported: true },
  { id: "existing", label: "Existing worktree", supported: true },
];

export const AGENT_TARGETS = [
  { id: "new", label: "New agent", supported: true },
  { id: "existing", label: "Existing agent", supported: false, reason: AGENT_IS_ALWAYS_NEW },
];

/** No worktree named, when one had to be. */
export const NO_WORKTREE_CHOSEN = "Choose the branch to implement into.";

/**
 * The checkouts an implementation can be sent into: the project's branch rows,
 * minus the ones that cannot host one. The primary checkout is the repository
 * itself, not a worktree to hand over, and a branch already carrying another
 * issue's implementation would make neither issue's diff readable — the bridge
 * refuses both, so neither is offered.
 */
export function worktreeChoices(items, { projectId, issueId = null } = {}) {
  return (items || [])
    .filter((row) => row && row.kind === "branch" && row.project_id === projectId)
    .filter((row) => row.worktree_id && !row.primary)
    .filter((row) => !row.issue_id || row.issue_id === issueId)
    .map((row) => ({ id: row.worktree_id, label: row.branch || row.title || row.worktree_id }));
}

const targetEntry = (targets, id) => (targets || []).find((target) => target.id === id) || null;

/** Whether this target can actually be dispatched. */
export function targetSupported(targets, id) {
  const entry = targetEntry(targets, id);
  return Boolean(entry && entry.supported);
}

/** Why a target cannot be taken, or null when it can. */
export function unsupportedTargetReason(targets, id) {
  const entry = targetEntry(targets, id);
  if (!entry || entry.supported) return null;
  return entry.reason;
}

/** The assignment an issue opens on: the handoff the bridge implements, with
 *  the issue's own model choice already in it. The base is left empty so the
 *  issue's own base branch shows through as the field's placeholder. */
export function defaultAssignment(issue) {
  return {
    worktree: "new",
    worktreeId: "",
    agent: "new",
    base: "",
    provider: (issue && issue.provider) || "",
    model: (issue && issue.model) || "",
    effort: (issue && issue.effort) || "",
  };
}

/** The one line the collapsed control shows: the two targets, then whatever the
 *  user has actually overridden. An existing checkout is named by its branch,
 *  which is the only part of it the reviewer thinks in — `choices` is what turns
 *  the held id back into that name. */
export function assignmentSummary(assignment, choices = []) {
  const targetingExisting = assignment.worktree === "existing";
  const chosen = targetingExisting
    ? (choices || []).find((choice) => choice.id === assignment.worktreeId)
    : null;
  const worktree =
    targetingExisting && chosen ? chosen.label : (targetEntry(WORKTREE_TARGETS, assignment.worktree) || {}).label;
  const agent = targetEntry(AGENT_TARGETS, assignment.agent);
  const base = targetingExisting ? "" : (assignment.base || "").trim();
  return [worktree, agent && agent.label, base || assignment.provider || null].filter(Boolean).join(" · ");
}

/**
 * The params one implement dispatch sends: the issue, the stage when a single
 * stage is being implemented, and the assignment's overrides. Throws on a target
 * the bridge cannot honour — the control disables those, so reaching here means
 * something else went wrong, and a silent drop would dispatch the WRONG handoff.
 *
 * A named checkout carries no base branch: the branch it implements into already
 * exists, and its current HEAD is the baseline.
 */
export function implementParams(issueId, assignment, { models = [], stageId = null } = {}) {
  const worktreeGap = unsupportedTargetReason(WORKTREE_TARGETS, assignment.worktree);
  if (worktreeGap) throw new Error(worktreeGap);
  const agentGap = unsupportedTargetReason(AGENT_TARGETS, assignment.agent);
  if (agentGap) throw new Error(agentGap);
  const targetingExisting = assignment.worktree === "existing";
  const worktreeId = targetingExisting ? (assignment.worktreeId || "").trim() : "";
  if (targetingExisting && !worktreeId) throw new Error(NO_WORKTREE_CHOSEN);
  const base = targetingExisting ? "" : (assignment.base || "").trim();
  return {
    issue_id: issueId,
    ...(stageId ? { stage_id: stageId } : {}),
    ...(worktreeId ? { worktree_id: worktreeId } : {}),
    ...(base ? { base_branch: base } : {}),
    ...modelParams(models, assignment.model, assignment.effort, assignment.provider),
  };
}

/** The repaint-freeze key for one poll: everything the two columns draw. An
 *  unchanged key leaves the DOM — and the reviewer's selection, open control and
 *  typed comment — alone. */
export function issueViewKey({ issue, stagesData, selectedStageId, docState, doc }) {
  return JSON.stringify([
    issue.state,
    issue.active_run_id || null,
    issue.docs_available,
    issue.implementation_activity || null,
    issue.implementation_lineage || [],
    (stagesData && stagesData.stages) || [],
    selectedStageId || null,
    docState,
    (doc || "").length,
    doc || "",
  ]);
}
