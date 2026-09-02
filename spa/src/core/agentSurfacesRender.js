import { esc } from "./text.js";
import { KEYED_LIST_ATTRIBUTE } from "./domPatch.js";
import { modalDialogHtml } from "./modal.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import {
  AGENT_ENTRY_KIND,
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
  workflowAgents: "surface-phase-agents",
  running: "surface-running",
  completed: "surface-completed-rows",
  [AGENT_ENTRY_KIND]: "surface-subagents",
  [SHELL_ENTRY_KIND]: "surface-shells",
  [CHECKLIST_ENTRY_KIND]: "surface-checklist",
};

const SURFACE_OVERLAY_BODY_CLASS = "surface-overlay-body";

export const SURFACE_OVERLAY_BODY_SELECTOR = `.${SURFACE_OVERLAY_BODY_CLASS}`;
export const WORKFLOW_HEAD_SELECTOR = `.${WORKFLOW_HEAD_CLASS}`;
export const COMPLETED_FOLD_SELECTOR = `.${COMPLETED_FOLD_CLASS}`;
export const COMPLETED_FOLD_HEAD_SELECTOR = `.${COMPLETED_FOLD_HEAD_CLASS}`;
export const PILL_COUNT_SELECTOR = `.${PILL_COUNT_CLASS}`;
export const CLIP_SELECTOR = `[${CLIP_LINES_ATTRIBUTE}]`;
export const PRESSABLE_CLIP_SELECTOR = `${CLIP_SELECTOR}[role="button"]`;
export const TICKING_CLOCK_SELECTOR = `[${RUNNING_SINCE_ATTRIBUTE}]`;

export const SURFACE_SELECTOR = Object.fromEntries(
  Object.entries(VIEWER_CLASS).map(([name, className]) => [name, `.${className}`]),
);

/** Text the viewer shows a few lines of. The whole of it rides in the title, so
 *  a hover reads it, and a press opens it out — which is why every clipped
 *  thing in every viewer is built here and nowhere else. A clip inside a fold
 *  or a button is for hovering alone: the press there belongs to what holds
 *  it, and only a press target says so in its role. */
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

function agentStatsHtml(row) {
  const stats = [
    Number.isFinite(row.tokens) ? `${row.tokens} tokens` : "",
    Number.isFinite(row.toolCalls) ? `${row.toolCalls} calls` : "",
  ].filter(Boolean);
  if (!stats.length) return "";
  return `<div class="surface-row-stats">${stats.map(statHtml).join("")}</div>`;
}

function agentLineHtml(row) {
  const detail = row.error || row.result || row.lastTool;
  if (!row.model && !detail) return "";
  const detailClass = row.error ? `${ROW_DETAIL_CLASS} ${ROW_ERROR_CLASS}` : ROW_DETAIL_CLASS;
  return `<div class="surface-row-line">
      ${row.model ? `<span class="surface-row-model">${esc(row.model)}</span>` : ""}
      ${detail ? clippedTextHtml(detail, { className: detailClass }) : ""}
    </div>`;
}

const SPAWNING_CALL_TITLE = "Open the call that spawned this";

function spawningCallHtml(row) {
  if (!Number.isFinite(row.callSequence)) return "";
  return `<button type="button" class="${ROW_JUMP_CLASS}" data-call-sequence="${esc(row.callSequence)}"
    title="${SPAWNING_CALL_TITLE}" aria-label="${SPAWNING_CALL_TITLE}">↗</button>`;
}

/** One agent, two lines — three where there is width for the counts. */
export function agentRowHtml(row, { compact = false } = {}) {
  return surfaceRowHtml("surface-agent", row, {
    trailing: spawningCallHtml(row),
    body: `${agentLineHtml(row)}
    ${compact ? "" : agentStatsHtml(row)}`,
  });
}

const pillCountCapHtml = (count) =>
  `<span class="${PILL_COUNT_CLASS}" data-motion${count ? "" : " hidden"}>${count ? esc(count) : ""}</span>`;

export function surfacePillHtml(pill, openKind) {
  return `<button type="button" class="surface-pill" data-motion data-surface-kind="${esc(pill.kind)}"
    aria-pressed="${pill.kind === openKind}">
    <span class="surface-pill-label">${esc(pill.label)}</span>
    ${pillCountCapHtml(pill.count)}
  </button>`;
}

/** One phase of a workflow: a fold the reader owns, over a keyed list of its
 *  agents. Whether it stands open is never said here — the mount opens the
 *  running one as it arrives, and the reader has it after that. */
export function phaseSectionHtml(phase, { compact = false } = {}) {
  return `<details class="${PHASE_CLASS}" data-key="${esc(phase.key)}" data-state="${esc(phase.state)}">
    <summary class="${PHASE_HEAD_CLASS}">
      ${clippedTextHtml(phase.title, { className: "surface-phase-title", pressable: false })}
      <span class="surface-phase-count">${esc(phase.done)}/${esc(phase.total)}</span>
      ${clockHtml(phase.clock, phase.runningSince)}
    </summary>
    <div class="${VIEWER_CLASS.workflowAgents}" ${KEYED_LIST_ATTRIBUTE}>${phase.rows
      .map((row) => agentRowHtml(row, { compact }))
      .join("")}</div>
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

export function workflowViewerHtml(workflow, choices, phases = [], options = {}) {
  return `<div class="${VIEWER_CLASS.viewer} surface-workflow">
    <div class="${VIEWER_CLASS.workflowChoices}">${choices.map(workflowChoiceHtml).join("")}</div>
    ${workflowHeadHtml(workflow)}
    <div class="${VIEWER_CLASS.workflowPhases}">${phases
      .map((phase) => phaseSectionHtml(phase, options))
      .join("")}</div>
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

export function checklistItemHtml(row) {
  return surfaceRowHtml("surface-checklist-item", row, {
    body: noteHtml(row.description, row.subject),
  });
}

export function surfaceOverlayHtml(label) {
  return modalDialogHtml(`<h3>${esc(label)}</h3><div class="${SURFACE_OVERLAY_BODY_CLASS}"></div>`, {
    className: "modal-surface",
  });
}
