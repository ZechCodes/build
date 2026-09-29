// Markup for the legacy plan page. Pure — HTML strings in, no DOM — so each
// piece is unit-tested by string assertion, and every payload string is esc()d
// on the way out.
//
// The surface these draw is a persistent two-column stages | stage viewer with
// no tabs and no drill-in, and it is read-only: the bridge refuses every verb
// that would move a plan. The left column is the plan's work item — its stages
// with their states and the implementations that carried it out. The right
// column is whichever stage doc is open, with the comments already on it and
// its stable diff.

import "../styles/surfaces.css";
import { esc } from "./text.js";
import { anchorLocationLabel, slugifyHeading } from "./anchors.js";
import { PLAN_STATE_LABEL, planChipClass, RUN_STATE_LABEL, runChipClass } from "./entityPresentation.js";
import { STAGE_STATE_LABEL, stageStateToken, stageStateChipClass, stageNeighbors } from "./taskModel.js";

/** The honest empty-state copy for a doc pane whose canonical contents are gone
 *  (a migrated task predating canonical storage, its worktree pruned). */
export const DOCS_UNAVAILABLE =
  "This Task's stage plans are unavailable — they predate canonical doc storage and their worktree is gone.";

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

/** The implementations this task has had, as its children: state, branch, and
 *  a way into the branch's changes. A task nobody implemented renders none of
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

/** The left column: where the plan stands, its stages, and its
 *  implementations. The plan's own message is not here — it is the first
 *  thing the conversation beside this column says. Nothing here acts on the
 *  plan: the page is a record. */
export function stageListHtml({ task, stagesData, selectedStageId = null }) {
  const stages = (stagesData && stagesData.stages) || [];
  const rows = stages
    .map((stage, index) => stageRowHtml(stage, { index, selected: stage.id === selectedStageId }))
    .join("");
  return `<div class="ivhead">
      <div class="ivmeta"><span class="chip ${planChipClass(task.state)}">${esc(PLAN_STATE_LABEL[task.state] || task.state || "")}</span>
        <span class="ivproject">${esc(task.project || "")}${task.base_branch ? ` · ${esc(task.base_branch)}` : ""}</span></div>
    </div>
    <div class="stagelist" id="stagelist">${rows || '<div class="empty">No stages yet.</div>'}</div>
    ${lineageHtml(task.implementation_lineage)}`;
}

/** Where a persisted doc comment points: the crumb naming its passage, and
 *  the passage it quotes. The comment carries the doc it was written on
 *  (comment_json's `path`); the anchor carries where in it. A passage under no
 *  heading falls back to naming the file, which is what the reader has to go
 *  on. */
function commentAnchorHtml(anchor, path) {
  const headingPath = (anchor && anchor.heading_path) || [];
  const location = anchorLocationLabel(anchor ? { ...anchor, path: anchor.path || path } : null);
  const markerKey = headingPath.length ? slugifyHeading(headingPath[headingPath.length - 1]) : "";
  const snippet = anchor && anchor.snippet
    ? `<span class="cc-snip">${esc(anchor.snippet.replace(/\s+/g, " ").trim().slice(0, 200))}</span>`
    : "";
  return `<span class="cc-crumb" data-marker="${esc(markerKey)}">${esc(location)}</span>
    ${snippet}`;
}

/** One persisted doc comment — which is a message on the task's conversation,
 *  so its id is the message's. An addressed one carries the agent's reply and
 *  is muted. */
export function docCommentCardHtml(comment) {
  const addressed = comment.state === "addressed";
  const reply = addressed && comment.agent_reply ? `<div class="cc-reply"><span class="cc-reply-k">agent</span> ${esc(comment.agent_reply)}</div>` : "";
  return `<div class="commentcard${addressed ? " addressed" : ""}" data-id="${esc(comment.id)}">
    ${commentAnchorHtml(comment.anchor, comment.path)}
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

/** Stepping to the stage either side of the open one, without going through the
 *  rail — which is a drawer on a phone, so reading the next stage cost a menu
 *  trip for the commonest move in a review. Each step carries the stage it would
 *  open, so the selection it makes is the one the rail makes; a step with
 *  nowhere to go is offered and disabled rather than removed, because a control
 *  that comes and goes is a control the reader has to look for. A stage with no
 *  siblings has nothing to walk, and renders no bar at all. */
export function stageNavHtml({ stages = [], selectedStageId = null } = {}) {
  const { index, total, previous, next } = stageNeighbors(stages, selectedStageId);
  if (index < 0 || total < 2) return "";
  const step = (which, label, neighbour) =>
    `<button class="btn mini stagenav-step" type="button" data-stage-step="${which}"` +
    (neighbour ? ` data-stage="${esc(neighbour.id)}" title="${esc(neighbour.title || "")}"` : " disabled") +
    ` aria-label="${which === "prev" ? "Previous" : "Next"} stage">${label}</button>`;
  return `<div class="stagenav" role="group" aria-label="Stage navigation">
      ${step("prev", "‹ Prev", previous)}
      <span class="stagenav-pos">${index + 1} / ${total}</span>
      ${step("next", "Next ›", next)}
    </div>`;
}

/** The right column: the open stage's doc, its state, the comments on it, and
 *  its stable diff. Nothing open is a line saying what to do, never a blank.
 *  `stages` is the whole manifest — the bar at the foot walks it. */
export function stageViewerHtml({ stage, stages = [], docHtml = "", paneState = "loading", comments = [] }) {
  if (!stage) return `<div class="empty ivplaceholder">Pick a stage to read its plan.</div>`;
  const token = stageStateToken(stage);
  const ordered = [...comments].sort((a, b) => (a.state === b.state ? 0 : a.state === "open" ? -1 : 1));
  const failure = stage.invalidation_reason
    ? `<div class="stage-invalidation"><strong>Stage incomplete</strong><div>${esc(stage.invalidation_reason)}</div></div>`
    : "";
  return `<div class="ivstagehead">
      <span class="ivstagetitle">${esc(stage.title || "")}</span>${stateChip(token)}
    </div>
    ${failure}
    <div class="plan${paneState === "ready" ? " markdown" : ""}" id="stagedoc">${docHtml}</div>
    <div class="stagecomments">${ordered.map(docCommentCardHtml).join("")}</div>
    <div class="actionbar">${stageNavHtml({ stages, selectedStageId: stage.id })}<span class="hint" id="stagehint"></span><div class="right" id="stageactions"></div></div>`;
}
