import { esc } from "./text.js";
import { KEYED_LIST_ATTRIBUTE } from "./domPatch.js";
import { modalDialogHtml } from "./modal.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import {
  AGENT_ENTRY_KIND,
  BUILD_AGENTS_KEY,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
} from "./agentSurfacesModel.js";

const ROW_HEAD_CLASS = "surface-row-head";
const ROW_CLOCK_CLASS = "surface-row-clock";
const ROW_LABEL_CLASS = "surface-row-label";
const ROW_DETAIL_CLASS = "surface-row-detail";
const ROW_ERROR_CLASS = "surface-row-error";
const ROW_NOTE_CLASS = "surface-row-note";
const ROW_JUMP_CLASS = "surface-row-jump";
const AGENT_DETAILS_CLASS = "surface-agent-details";
const AGENT_FACTS_CLASS = "surface-agent-facts";
const WORKFLOW_HEAD_CLASS = "surface-workflow-head";
const PHASE_CLASS = "surface-phase";
const PHASE_HEAD_CLASS = "surface-phase-head";

const CLIP_CLASS = "surface-clip";
const CLIP_LINES_ATTRIBUTE = "data-clip-lines";
const RUNNING_SINCE_ATTRIBUTE = "data-running-since";

const COMPLETED_FOLD_CLASS = "surface-completed";
const COMPLETED_FOLD_HEAD_CLASS = "surface-completed-head";
const PILL_COUNT_CLASS = "surface-pill-count";

const VIEWER_CLASS = {
  viewer: "surface-viewer",
  workflowChoices: "surface-workflows",
  workflowPhases: "surface-phases",
  workflowDetail: "surface-workflow-detail",
  workflowAgents: "surface-phase-agents",
  running: "surface-running",
  completed: "surface-completed-rows",
  buildAgents: "surface-build-agents",
  subagentsGroup: "surface-group-subagents",
  buildAgentsGroup: "surface-group-build",
  [AGENT_ENTRY_KIND]: "surface-subagents",
  [SHELL_ENTRY_KIND]: "surface-shells",
  [CHECKLIST_ENTRY_KIND]: "surface-checklist",
  checklistRows: "surface-checklist-rows",
};

const CHECKLIST_CONTEXT_CLASS = "surface-checklist-context";

const SURFACE_OVERLAY_BODY_CLASS = "surface-overlay-body";

export const SURFACE_OVERLAY_BODY_SELECTOR = `.${SURFACE_OVERLAY_BODY_CLASS}`;
export const WORKFLOW_HEAD_SELECTOR = `.${WORKFLOW_HEAD_CLASS}`;
export const COMPLETED_FOLD_SELECTOR = `.${COMPLETED_FOLD_CLASS}`;
export const COMPLETED_FOLD_HEAD_SELECTOR = `.${COMPLETED_FOLD_HEAD_CLASS}`;
export const PILL_COUNT_SELECTOR = `.${PILL_COUNT_CLASS}`;
export const CLIP_SELECTOR = `[${CLIP_LINES_ATTRIBUTE}]`;
export const PRESSABLE_CLIP_SELECTOR = `${CLIP_SELECTOR}[role="button"]`;
export const TICKING_CLOCK_SELECTOR = `[${RUNNING_SINCE_ATTRIBUTE}]`;
export const CHECKLIST_CONTEXT_SELECTOR = `.${CHECKLIST_CONTEXT_CLASS}`;

export const SURFACE_SELECTOR = Object.fromEntries(
  Object.entries(VIEWER_CLASS).map(([name, className]) => [name, `.${className}`]),
);

export function clippedTextHtml(text, { className = "", lines = 1, pressable = true } = {}) {
  const classes = className ? `${CLIP_CLASS} ${className}` : CLIP_CLASS;
  const press = pressable ? ` role="button" tabindex="0" aria-expanded="false"` : "";
  return `<span class="${classes}" ${CLIP_LINES_ATTRIBUTE}="${esc(lines)}" title="${esc(text)}"${press}>${esc(text)}</span>`;
}

function stateMarkHtml(stateMark) {
  return stateMark ? outcomeMarkHtml(stateMark.mark, stateMark.label) : "";
}

function statHtml(text) {
  return `<span class="surface-row-stat">${esc(text)}</span>`;
}

function noteHtml(description, subject) {
  if (!description || description === subject) return "";
  return clippedTextHtml(description, { className: ROW_NOTE_CLASS, lines: 2 });
}

function clockHtml(clock, runningSince) {
  const ticking = Number.isFinite(runningSince) ? ` ${RUNNING_SINCE_ATTRIBUTE}="${esc(runningSince)}"` : "";
  if (!ticking && !clock) return "";
  return `<span class="${ROW_CLOCK_CLASS}"${ticking}>${esc(clock)}</span>`;
}

function rowHeadHtml(row, { trailing = "" } = {}) {
  return `<div class="${ROW_HEAD_CLASS}">
      ${stateMarkHtml(row.stateMark)}
      ${clippedTextHtml(row.subject, { className: ROW_LABEL_CLASS })}
      ${trailing}
      ${clockHtml(row.clock, row.runningSince)}
    </div>`;
}

function surfaceRowHtml(rowClass, row, { trailing = "", body = "" } = {}) {
  return `<div class="surface-row ${rowClass}" data-key="${esc(row.key)}">
    ${rowHeadHtml(row, { trailing })}
    ${body}
  </div>`;
}

function agentFactHtml(label, value, { className = "" } = {}) {
  const classes = className ? ` class="${className}"` : "";
  return `<div class="surface-agent-fact">
      <dt>${esc(label)}</dt>
      <dd${classes}>${esc(value)}</dd>
    </div>`;
}

function optionalAgentFactHtml(label, value, options) {
  const unavailable = value === null || value === undefined || (typeof value === "string" && !value.trim());
  return unavailable ? "" : agentFactHtml(label, value, options);
}

function agentState(row) {
  return (row.stateMark && row.stateMark.label) || row.state;
}

const SPAWNING_CALL_TITLE = "Open the call that spawned this";

function spawningCallHtml(row) {
  return `<button type="button" class="${ROW_JUMP_CLASS}" data-call-sequence="${esc(row.callSequence)}"
    title="${SPAWNING_CALL_TITLE}" aria-label="${SPAWNING_CALL_TITLE}">Open spawning call&nbsp;↗</button>`;
}

function spawningCallFactHtml(row) {
  if (!Number.isFinite(row.callSequence)) return "";
  return `<div class="surface-agent-fact">
      <dt>Spawned by</dt>
      <dd>${spawningCallHtml(row)}</dd>
    </div>`;
}

function agentDetailsHtml(row) {
  const resultClass = row.error ? `${ROW_DETAIL_CLASS} ${ROW_ERROR_CLASS}` : ROW_DETAIL_CLASS;
  const result = row.error || row.result;
  return `<div class="${AGENT_DETAILS_CLASS}">
    <dl class="${AGENT_FACTS_CLASS}">
      ${optionalAgentFactHtml("Description", row.description)}
      ${optionalAgentFactHtml("State", agentState(row))}
      ${optionalAgentFactHtml("Model", row.model, { className: "surface-row-model" })}
      ${optionalAgentFactHtml("Reasoning effort", row.reasoningEffort, { className: "surface-row-effort" })}
      ${optionalAgentFactHtml("Current activity", row.lastTool)}
      ${optionalAgentFactHtml(row.error ? "Error" : "Result", result, { className: resultClass })}
      ${Number.isFinite(row.tokens) ? agentFactHtml("Tokens", String(row.tokens)) : ""}
      ${Number.isFinite(row.toolCalls) ? agentFactHtml("Tool calls", String(row.toolCalls)) : ""}
      ${Number.isFinite(row.attempt) ? agentFactHtml("Attempt", String(row.attempt)) : ""}
      ${spawningCallFactHtml(row)}
    </dl>
  </div>`;
}

export function agentRowHtml(row, { openedAgentKeys = new Set() } = {}) {
  const open = openedAgentKeys.has(row.key) ? " open" : "";
  const model = row.model
    ? clippedTextHtml(row.model, { className: "surface-row-model surface-row-head-model", pressable: false })
    : "";
  return `<details class="surface-row surface-agent" data-key="${esc(row.key)}"${open}>
    <summary class="${ROW_HEAD_CLASS} surface-agent-summary">
      ${stateMarkHtml(row.stateMark)}
      ${clippedTextHtml(row.subject, { className: ROW_LABEL_CLASS, pressable: false })}
      ${model}
      ${clockHtml(row.clock, row.runningSince)}
    </summary>
    ${agentDetailsHtml(row)}
  </details>`;
}

const pillCountCapHtml = (count) =>
  `<span class="${PILL_COUNT_CLASS}" data-motion${count ? "" : " hidden"}>${count ? esc(count) : ""}</span>`;

export function surfacePillHtml(pill, openKind) {
  return `<button type="button" class="surface-pill" data-motion data-surface-kind="${esc(pill.kind)}"
    aria-pressed="${pill.kind === openKind}">
    <span class="surface-pill-label">${esc(pill.label)}</span>
    ${pillCountCapHtml(pill.progress ?? pill.count)}
  </button>`;
}

const checklistNotesHtml = (notes) => (notes || [])
  .map((note) => `<span class="agent-observation-note">${esc(note)}</span>`)
  .join("");

export function checklistContextHtml(checklist) {
  if (!checklist) return `<div class="${CHECKLIST_CONTEXT_CLASS}" hidden></div>`;
  const step = checklist.currentStep
    ? `<span class="agent-observation-step">${esc(checklist.currentStep)}</span>`
    : "";
  return `<div class="${CHECKLIST_CONTEXT_CLASS}${checklist.stale ? " is-stale" : ""}">
    ${step}
    <span class="agent-observation-checklist-meta">
      <span class="agent-observation-progress">${esc(checklist.progress)}</span>
      <span class="agent-observation-notes">${checklistNotesHtml(checklist.notes)}</span>
    </span>
  </div>`;
}

export function checklistViewerHtml(checklist = null) {
  return `<div class="${VIEWER_CLASS.viewer} ${VIEWER_CLASS[CHECKLIST_ENTRY_KIND]}">
    ${checklistContextHtml(checklist)}
    <div class="${VIEWER_CLASS.checklistRows}" ${KEYED_LIST_ATTRIBUTE}></div>
  </div>`;
}

export function phaseSectionHtml(phase) {
  return `<details class="${PHASE_CLASS}" data-key="${esc(phase.key)}" data-state="${esc(phase.state)}">
    <summary class="${PHASE_HEAD_CLASS}">
      ${clippedTextHtml(phase.title, { className: "surface-phase-title", pressable: false })}
      <span class="surface-phase-count">${esc(phase.done)}/${esc(phase.total)}</span>
      ${clockHtml(phase.clock, phase.runningSince)}
    </summary>
    <div class="${VIEWER_CLASS.workflowAgents}" ${KEYED_LIST_ATTRIBUTE}></div>
  </details>`;
}

export function workflowChoiceHtml(choice) {
  return `<button type="button" class="surface-workflow-choice" data-key="${esc(choice.key)}"
    data-workflow-index="${esc(choice.index)}" aria-pressed="${choice.selected}">
    ${stateMarkHtml(choice.stateMark)}
    ${clippedTextHtml(choice.subject, { className: "surface-choice-name", pressable: false })}
  </button>`;
}

export function workflowHeadHtml(workflow) {
  return `<div class="${WORKFLOW_HEAD_CLASS}">
    ${rowHeadHtml(workflow)}
    ${noteHtml(workflow.description, workflow.subject)}
  </div>`;
}

export function workflowViewerHtml(workflow, choices, phases = []) {
  return `<div class="${VIEWER_CLASS.viewer} surface-workflow">
    <div class="${VIEWER_CLASS.workflowChoices}">${choices.map(workflowChoiceHtml).join("")}</div>
    <div class="${VIEWER_CLASS.workflowDetail}">
      ${workflowHeadHtml(workflow)}
      <div class="${VIEWER_CLASS.workflowPhases}">${phases.map(phaseSectionHtml).join("")}</div>
    </div>
  </div>`;
}

export function kindViewerHtml(kind, rows, renderRow) {
  return `<div class="${VIEWER_CLASS.viewer} ${VIEWER_CLASS[kind]}">${rows
    .map(renderRow)
    .join("")}</div>`;
}

export function completedFoldHeadHtml(count) {
  return `<summary class="${COMPLETED_FOLD_HEAD_CLASS}">Completed (${esc(count)})</summary>`;
}

export function completedFoldHtml(count, rowsHtml = "") {
  return `<details class="${COMPLETED_FOLD_CLASS}">
    ${completedFoldHeadHtml(count)}
    <div class="${VIEWER_CLASS.completed}">${rowsHtml}</div>
  </details>`;
}

/** One of the Agents viewer's two groups (#216): a heading, and the list it
 *  heads. Hidden until it has rows, so an agent that made no Build agents is
 *  not told it has none. */
const agentGroupHtml = (group, className, head, listHtml) => `<section class="surface-group ${className}" data-group="${esc(group)}" hidden>
    <h4 class="surface-group-head">${esc(head)}</h4>
    ${listHtml}
  </section>`;

/** The Agents viewer: the harness's sub-agents, their finished ones folded
 *  inside the same group, then the Build agents this agent made. */
export function agentsViewerHtml() {
  return `<div class="${VIEWER_CLASS.viewer} ${VIEWER_CLASS[AGENT_ENTRY_KIND]}">
    ${agentGroupHtml(AGENT_ENTRY_KIND, VIEWER_CLASS.subagentsGroup, "Sub-agents", `<div class="${VIEWER_CLASS.running}"></div>`)}
    ${agentGroupHtml(BUILD_AGENTS_KEY, VIEWER_CLASS.buildAgentsGroup, "Build agents", `<div class="${VIEWER_CLASS.buildAgents}" ${KEYED_LIST_ATTRIBUTE}></div>`)}
  </div>`;
}

/** Why a Build agent's row opens nothing, by the kind of row it is on (#226):
 *  what the reader can do instead, where there is anything. */
const UNREACHABLE_BUILD_AGENT_TITLES = {
  branch: (name) => `${name} works on a branch outside any workspace. Its chat opens from that branch, not from here.`,
};
const NOWHERE_TO_OPEN_TITLE = (name) => `Build has nowhere to open ${name}'s chat yet.`;

/** One Build agent this agent made: a press opens its chat. The whole row is
 *  the button, so nothing inside it is pressable on its own. A Build agent
 *  whose chat has no page to open on (#221) is drawn as a plain row that says
 *  why, never as a button that does nothing. */
export function buildAgentRowHtml(row) {
  const where = row.workspaceName
    ? `<span class="surface-row-stat surface-build-agent-where">${esc(row.workspaceName)}</span>`
    : "";
  const model = row.model
    ? clippedTextHtml(row.model, { className: "surface-row-model surface-row-head-model", pressable: false })
    : "";
  const head = `<span class="${ROW_HEAD_CLASS}">
      ${stateMarkHtml(row.stateMark)}
      ${clippedTextHtml(row.subject, { className: ROW_LABEL_CLASS, pressable: false })}
      ${model}
      ${where}
      ${clockHtml(row.clock, row.runningSince)}
    </span>`;
  if (!row.chatKind) {
    const why = (UNREACHABLE_BUILD_AGENT_TITLES[row.kind] || NOWHERE_TO_OPEN_TITLE)(row.subject);
    return `<div class="surface-row surface-build-agent surface-build-agent-unreachable" data-key="${esc(row.key)}"
    title="${esc(why)}">
    ${head}
  </div>`;
  }
  return `<button type="button" class="surface-row surface-build-agent" data-key="${esc(row.key)}"
    data-build-agent="${esc(row.id)}" data-entity-id="${esc(row.entityId || "")}"
    data-workspace-id="${esc(row.workspaceId || "")}" data-kind="${esc(row.chatKind)}"
    title="${esc(`Open ${row.subject}'s chat`)}">
    ${head}
  </button>`;
}

export function runningAndCompletedViewerHtml(kind, { running, completed }, renderRow) {
  const fold = completed.length ? completedFoldHtml(completed.length, completed.map(renderRow).join("")) : "";
  return `<div class="${VIEWER_CLASS.viewer} ${VIEWER_CLASS[kind]}">
    <div class="${VIEWER_CLASS.running}">${running.map(renderRow).join("")}</div>
    ${fold}
  </div>`;
}

function shellTailHtml(tail) {
  if (!tail.length) return "";
  return `<details class="surface-shell-tail">
    <summary class="surface-shell-tail-head">Output</summary>
    <pre class="surface-shell-tail-body">${esc(tail.join("\n"))}</pre>
  </details>`;
}

export function shellRowHtml(row) {
  return surfaceRowHtml("surface-shell", row, {
    trailing: Number.isFinite(row.exitCode) ? statHtml(`exit ${row.exitCode}`) : "",
    body: shellTailHtml(row.tail),
  });
}

/**
 * One task on the agent's Tasks surface (#34).
 *
 * The state's own word is the trailing stat — "In progress", "Assigned",
 * "Tracking", "In review" — because the mark beside the subject is a colour
 * and four of the six states share two colours between them. The row carries
 * no link: the surfaces are a viewer, and where a task is opened FROM is the
 * Tasks tab and the task's page.
 */
export function taskSurfaceRowHtml(row) {
  return surfaceRowHtml("surface-task", row, {
    trailing: row.stateMark ? statHtml(row.stateMark.label) : "",
  });
}

export function checklistItemHtml(row) {
  const unknownState = !row.stateMark && row.state
    ? `<span class="surface-row-stat agent-observation-unknown">${esc(row.state)}</span>`
    : "";
  const completedClass = row.state === "completed" ? " is-completed" : "";
  return surfaceRowHtml(`surface-checklist-item${completedClass}`, row, {
    trailing: unknownState,
    body: noteHtml(row.description, row.subject),
  });
}

export function surfaceOverlayHtml(label) {
  return modalDialogHtml(`<h3>${esc(label)}</h3><div class="${SURFACE_OVERLAY_BODY_CLASS}"></div>`, {
    className: "modal-surface",
  });
}
