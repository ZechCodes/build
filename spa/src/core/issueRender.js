// Markup for the issue view. Pure — HTML strings in, no DOM — so each piece is
// unit-tested by string assertion, and every payload string is esc()d on the
// way out.
//
// The surface these draw is the one the UX Redesign Decisions doc specifies: a
// persistent two-column stages | stage viewer with no tabs and no drill-in. The
// left column is the issue's work item — its stages with their states, the gate
// and approve actions, the worktree/agent assignment control, and the
// implementations that carried the issue out. The right column is whichever
// stage doc is open, with its own actions and the comments already on it.

import "../styles/surfaces.css";
import { esc } from "./text.js";
import { anchorLocationLabel, slugifyHeading } from "./anchors.js";
import { PLAN_STATE_LABEL, planChipClass, RUN_STATE_LABEL, runChipClass } from "./entityPresentation.js";
import {
  STAGE_STATE_LABEL,
  stageStateToken,
  stageStateChipClass,
  plannedStageIds,
  assignmentSummary,
  WORKTREE_TARGETS,
  AGENT_TARGETS,
} from "./issueModel.js";
import { catalogForProvider, effortOptionsHtml, modelOptionsHtml, providerOptionsHtml, normalizeModelCatalog } from "./modelPicker.js";

/** The honest empty-state copy for a doc pane whose canonical contents are gone
 *  (a migrated issue predating canonical storage, its worktree pruned). */
export const DOCS_UNAVAILABLE =
  "This Issue's stage plans are unavailable — they predate canonical doc storage and their worktree is gone.";

/** The doc-read error pane, with an inline Retry that clears the read latch and
 *  refetches. `kind` is "stage" (the stage viewer) or "plan" (a single doc). */
export function docErrorPaneHtml(kind) {
  const isStage = kind === "stage";
  const buttonId = isStage ? "stagedocretry" : "docretry";
  const what = isStage ? "this stage document" : "the stage plan document";
  return `<div class="plan-empty warn">Couldn't load ${what}. <button class="btn mini" id="${buttonId}">Retry</button></div>`;
}

const commentBadge = (n) => (Number(n) > 0 ? `<span class="cbadge">${esc(String(n))} 💬</span>` : "");

const stateChip = (token) =>
  `<span class="chip stagechip ${stageStateChipClass(token)}">${esc(STAGE_STATE_LABEL[token] || String(token || "").toUpperCase())}</span>`;

/** One row of the stage list: its ordinal, its title, where it is, and what is
 *  waiting on it. The row is the navigation — selecting it swaps the viewer, so
 *  the list never leaves the column. */
export function stageRowHtml(stage, { index = 0, selected = false } = {}) {
  const token = stageStateToken(stage);
  return `<div class="stagerow${selected ? " sel" : ""}" data-stage="${esc(stage.id)}">
    <span class="stagenum">${String(index + 1).padStart(2, "0")}</span>
    <div class="stagemain">
      <div class="stagetop"><span class="stagetitle">${esc(stage.title || "")}</span>${stateChip(token)}${commentBadge(stage.open_comments)}</div>
      ${stage.summary ? `<div class="stagesummary">${esc(stage.summary)}</div>` : ""}
    </div></div>`;
}

/** The implementations this issue has had, as its children: state, branch, and
 *  a way into the branch's changes. An issue nobody implemented renders none of
 *  it rather than an empty heading. */
export function lineageHtml(lineage) {
  const rows = (lineage || [])
    .map((implementation) => {
      const state = implementation.state || "created";
      return `<div class="ivlin-row" data-run="${esc(implementation.run_id || implementation.implementation_id || "")}"${implementation.branch ? ` data-branch="${esc(implementation.branch)}"` : ""}>
        <span class="ivlin-branch mono">${esc(implementation.branch || "(no branch)")}</span>
        <span class="chip ${runChipClass(state)}">${esc(RUN_STATE_LABEL[state] || String(state).toUpperCase())}</span>
      </div>`;
    })
    .join("");
  if (!rows) return "";
  return `<div class="ivlineage"><div class="ivsec">Implementations</div>${rows}</div>`;
}

/** The assignment control as it stands in the rail: one line saying what the
 *  handoff would be, and nothing else. Pressing it opens the overlay that holds
 *  the fields — the rail is a list of stages, and a form unfolding inside it is
 *  what pushed the stages off the column. Open or shut, this stays one row. */
export function assignmentHtml({ assignment, open = false, worktrees = [] }) {
  return `<div class="ivassign${open ? " open" : ""}">
    <button class="ivassign-head" id="assigntoggle" aria-haspopup="dialog" aria-expanded="${open ? "true" : "false"}">
      <span class="ivassign-text"><span class="ivsec">Assignment</span><span class="ivassign-sum">${esc(assignmentSummary(assignment, worktrees))}</span></span>
      <span class="ivassign-caret" aria-hidden="true">▾</span>
    </button></div>`;
}

/** What the assignment overlay holds: the two targets and the overrides the
 *  dispatch carries.
 *
 *  Targeting an existing checkout swaps the base-branch field for the branch
 *  picker — a branch that already exists brings its own baseline, so there is no
 *  base to choose. The existing-agent option is offered and disabled: it is not
 *  a gap but a rule, and the copy below the fields says which. */
export function assignmentPanelHtml({ assignment, catalog = {}, worktrees = [] }) {
  const full = normalizeModelCatalog(catalog);
  const provider = assignment.provider || full.default_provider || "claude";
  const forProvider = catalogForProvider(full, provider);
  const options = (targets, selected) =>
    targets
      .map(
        (target) =>
          `<option value="${esc(target.id)}"${target.id === selected ? " selected" : ""}${target.supported ? "" : " disabled"}>${esc(target.label)}${target.supported ? "" : " (not yet)"}</option>`,
      )
      .join("");
  const rules = [...WORKTREE_TARGETS, ...AGENT_TARGETS].filter((target) => !target.supported).map((target) => target.reason);
  const branchField =
    assignment.worktree === "existing"
      ? `<label class="ivfield"><span>Branch</span><select id="assignworktreeid">
          <option value=""${assignment.worktreeId ? "" : " selected"}>choose a branch…</option>
          ${worktrees
            .map(
              (choice) =>
                `<option value="${esc(choice.id)}"${choice.id === assignment.worktreeId ? " selected" : ""}>${esc(choice.label)}</option>`,
            )
            .join("")}
        </select></label>`
      : `<label class="ivfield"><span>Base branch</span><input id="assignbase" value="${esc(assignment.base || "")}" placeholder="the Issue base branch"></label>`;
  return `<div class="assign-pop-head"><span class="ivsec">Assignment</span>
      <button class="btn mini" id="assignclose" data-assign-close>Done</button></div>
    <div class="assign-fields">
      <label class="ivfield"><span>Worktree</span><select id="assignworktree">${options(WORKTREE_TARGETS, assignment.worktree)}</select></label>
      ${branchField}
      <label class="ivfield"><span>Agent</span><select id="assignagent">${options(AGENT_TARGETS, assignment.agent)}</select></label>
      <label class="ivfield"><span>Provider</span><select id="assignprovider">${providerOptionsHtml(full.providers, provider)}</select></label>
      <label class="ivfield"><span>Model</span><select id="assignmodel">${modelOptionsHtml(forProvider.models, assignment.model || "")}</select></label>
      <label class="ivfield"><span>Effort</span><select id="assigneffort">${effortOptionsHtml(forProvider.efforts, assignment.effort || "")}</select></label>
      <div class="ivassign-gap">${rules.map((rule) => `<div>${esc(rule)}</div>`).join("")}</div>
    </div>`;
}

/** The left column: where the issue stands, its gate, its stages, the assignment
 *  control, and its implementations. The issue's own message is not here — it is
 *  the first thing the conversation beside this column says, and saying it twice
 *  is what made the surface busy. */
export function stageListHtml({
  issue,
  stagesData,
  selectedStageId = null,
  assignment,
  assignmentOpen = false,
  worktrees = [],
}) {
  const stages = (stagesData && stagesData.stages) || [];
  const rows = stages
    .map((stage, index) => stageRowHtml(stage, { index, selected: stage.id === selectedStageId }))
    .join("");
  const allPlanned = stages.length > 0 && plannedStageIds(stages).length === stages.length;
  const ready = issue.state === "approved";
  const activity = issue.implementation_activity;
  const activityKind = activity && typeof activity === "object" ? Object.keys(activity)[0] : activity;
  const waiting = activityKind === "waiting_approval";
  const running = activityKind === "preparing" || activityKind === "running";
  const implementLabel =
    waiting ? "Implement All · waiting for stage-plan approval"
    : activityKind === "preparing" ? "Implement All · preparing"
    : activityKind === "running" ? "Implement All · running"
    : activityKind === "blocked" ? "Implement All · blocked"
    : "Implement All";
  const gate = [
    issue.state === "plan_review" ? `<button class="btn mini" id="approveissue">Mark issue ready</button>` : "",
    allPlanned ? `<button class="btn mini" id="approveall">Approve all stage plans</button>` : "",
    ready && stages.length
      ? `<button class="btn primary mini" id="implementall"${waiting || running ? " disabled" : ""}>${esc(implementLabel)}</button>`
      : "",
  ]
    .filter(Boolean)
    .join("");
  return `<div class="ivhead">
      <div class="ivmeta"><span class="chip ${planChipClass(issue.state)}">${esc(PLAN_STATE_LABEL[issue.state] || issue.state || "")}</span>
        <span class="ivproject">${esc(issue.project || "")}${issue.base_branch ? ` · ${esc(issue.base_branch)}` : ""}</span></div>
      <div class="ivgate">${gate}</div>
    </div>
    <div class="stagelist" id="stagelist">${rows || '<div class="empty">No stages yet.</div>'}</div>
    ${assignmentHtml({ assignment, open: assignmentOpen, worktrees })}
    ${lineageHtml(issue.implementation_lineage)}`;
}

/** One persisted doc comment — which is a message on the issue's conversation,
 *  so its id is the message's. An open one can be withdrawn; an addressed one
 *  carries the agent's reply and is muted. */
export function docCommentCardHtml(comment) {
  const anchor = comment.anchor;
  const headingPath = (anchor && anchor.heading_path) || [];
  // The comment carries the doc it was written on (comment_json's `path`); the
  // anchor carries where in it. A passage under no heading falls back to naming
  // the file, which is what the reader has to go on.
  const location = anchorLocationLabel(anchor ? { ...anchor, path: anchor.path || comment.path } : null);
  const snippet = anchor && anchor.snippet
    ? `<span class="cc-snip">${esc(anchor.snippet.replace(/\s+/g, " ").trim().slice(0, 200))}</span>`
    : "";
  const addressed = comment.state === "addressed";
  const reply = addressed && comment.agent_reply ? `<div class="cc-reply"><span class="cc-reply-k">agent</span> ${esc(comment.agent_reply)}</div>` : "";
  const remove = addressed ? "" : `<span class="cc-x" data-del="${esc(comment.id)}">×</span>`;
  const markerKey = headingPath.length ? slugifyHeading(headingPath[headingPath.length - 1]) : "";
  return `<div class="commentcard${addressed ? " addressed" : ""}" data-id="${esc(comment.id)}">
    ${remove}
    <span class="cc-crumb" data-marker="${esc(markerKey)}">${esc(location)}</span>
    ${snippet}
    <span class="cc-body">${esc(comment.body || "")}</span>
    ${reply}</div>`;
}

/** The margin marker for one heading's comments: how many, and whether any of
 *  them still need someone. Parts rather than markup — the marker is planted
 *  into an already-rendered doc, and building it as an element is what keeps
 *  doc text out of an HTML string it never has to enter. */
export function docMarkerParts(group) {
  const settled = group.open === 0;
  return {
    className: `docmarker${settled ? " addressed" : ""}`,
    label: `💬 ${group.total}`,
    title: `${group.total} comment${group.total === 1 ? "" : "s"}${settled ? " · addressed" : ""}`,
  };
}

/** The right column: the open stage's doc, its state, the comments on it, and
 *  its own actions. Nothing open is a line saying what to do, never a blank. */
export function stageViewerHtml({ stage, docHtml = "", paneState = "loading", comments = [] }) {
  if (!stage) return `<div class="empty ivplaceholder">Pick a stage to read its plan.</div>`;
  const token = stageStateToken(stage);
  const ordered = [...comments].sort((a, b) => (a.state === b.state ? 0 : a.state === "open" ? -1 : 1));
  const failure = stage.invalidation_reason
    ? `<div class="stage-validation fail"><strong>Stage incomplete</strong><div>${esc(stage.invalidation_reason)}</div></div>`
    : "";
  return `<div class="ivstagehead">
      <span class="ivstagetitle">${esc(stage.title || "")}</span>${stateChip(token)}
    </div>
    ${failure}
    <div class="plan${paneState === "ready" ? " markdown" : ""}" id="stagedoc">${docHtml}</div>
    <div class="stagecomments">${ordered.map(docCommentCardHtml).join("")}</div>
    <div class="ivstagefeedback"></div>
    <div class="actionbar"><span class="hint" id="stagehint"></span><div class="right" id="stageactions"></div></div>`;
}
