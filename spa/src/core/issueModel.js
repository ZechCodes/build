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

/** Where an implementation in the lineage lives. Today that is its run's
 *  Changes surface; the branch route the Decisions doc specifies replaces this
 *  target once the shell ships it, and this is the one place it changes. */
export function lineageRoute(implementation, projectId) {
  const runId = implementation && (implementation.run_id || implementation.implementation_id);
  if (!runId) return null;
  return { name: "task", projectId, id: runId, tab: "changes" };
}

// ---- the worktree/agent assignment control ---------------------------------
// The issue's handoff: which checkout the implementation runs in, and which
// agent runs it. Both offers are real; only the new-worktree/new-agent handoff
// is something the bridge's implement verbs can express today, so the existing-*
// options are rendered and disabled rather than hidden — the shape of the choice
// is the honest part, and hiding it would hide the gap.

const NO_WORKTREE_TARGET = "Targeting an existing worktree needs a param issue.implement_all/implement_stage does not take yet.";
const NO_AGENT_TARGET = "Targeting an existing agent needs a param issue.implement_all/implement_stage does not take yet.";

export const WORKTREE_TARGETS = [
  { id: "new", label: "New worktree", supported: true },
  { id: "existing", label: "Existing worktree", supported: false, reason: NO_WORKTREE_TARGET },
];

export const AGENT_TARGETS = [
  { id: "new", label: "New agent", supported: true },
  { id: "existing", label: "Existing agent", supported: false, reason: NO_AGENT_TARGET },
];

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
    agent: "new",
    base: "",
    provider: (issue && issue.provider) || "",
    model: (issue && issue.model) || "",
    effort: (issue && issue.effort) || "",
  };
}

/** The one line the collapsed control shows: the two targets, then whatever the
 *  user has actually overridden. */
export function assignmentSummary(assignment) {
  const worktree = targetEntry(WORKTREE_TARGETS, assignment.worktree);
  const agent = targetEntry(AGENT_TARGETS, assignment.agent);
  const base = (assignment.base || "").trim();
  return [worktree && worktree.label, agent && agent.label, base || assignment.provider || null]
    .filter(Boolean)
    .join(" · ");
}

/**
 * The params one implement dispatch sends: the issue, the stage when a single
 * stage is being implemented, and the assignment's overrides. Throws on a target
 * the bridge cannot honour — the control disables those, so reaching here means
 * something else went wrong, and a silent drop would dispatch the WRONG handoff.
 */
export function implementParams(issueId, assignment, { models = [], stageId = null } = {}) {
  const worktreeGap = unsupportedTargetReason(WORKTREE_TARGETS, assignment.worktree);
  if (worktreeGap) throw new Error(worktreeGap);
  const agentGap = unsupportedTargetReason(AGENT_TARGETS, assignment.agent);
  if (agentGap) throw new Error(agentGap);
  const base = (assignment.base || "").trim();
  return {
    issue_id: issueId,
    ...(stageId ? { stage_id: stageId } : {}),
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
    issue.goal,
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
