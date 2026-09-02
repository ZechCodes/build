import { esc } from "./text.js";
import { menuButtonMarkup } from "./splitButton.js";
import { modalDialogHtml } from "./modal.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
} from "./agentSurfacesModel.js";

const ACTION_MENU_LABEL = "Ask";
const ACTION_MENU_TITLE = "Ask the agent about this";

const ROW_HEAD_CLASS = "surface-row-head";
const WORKFLOW_HEAD_CLASS = "surface-workflow-head";

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

export const SURFACE_SELECTOR = Object.fromEntries(
  Object.entries(VIEWER_CLASS).map(([name, className]) => [name, `.${className}`]),
);

function stateMarkHtml(stateMark) {
  return stateMark ? outcomeMarkHtml(stateMark.mark, stateMark.label) : "";
}

function actionMenuHtml(actions) {
  if (!actions || !actions.length) return "";
  return menuButtonMarkup(ACTION_MENU_LABEL, actions, { title: ACTION_MENU_TITLE });
}

function statHtml(text) {
  return `<span class="surface-row-stat">${esc(text)}</span>`;
}

function noteHtml(description, subject) {
  if (!description || description === subject) return "";
  return `<span class="surface-row-note">${esc(description)}</span>`;
}

function rowHeadHtml(row, { headClass = ROW_HEAD_CLASS, trailing = "" } = {}) {
  return `<div class="${headClass}">
      ${stateMarkHtml(row.stateMark)}
      <span class="surface-row-label">${esc(row.subject)}</span>
      ${trailing}
      ${actionMenuHtml(row.actions)}
    </div>`;
}

function surfaceRowHtml(rowClass, row, { attributes = "", trailing = "", body = "" } = {}) {
  return `<div class="surface-row ${rowClass}" data-key="${esc(row.key)}"${attributes}>
    ${rowHeadHtml(row, { trailing })}
    ${body}
  </div>`;
}

function agentStatsHtml(row) {
  const stats = [
    Number.isFinite(row.tokens) ? `${row.tokens} tokens` : "",
    Number.isFinite(row.toolCalls) ? `${row.toolCalls} calls` : "",
    row.duration,
  ].filter(Boolean);
  if (!stats.length) return "";
  return `<div class="surface-row-stats">${stats.map(statHtml).join("")}</div>`;
}

function agentOutcomeHtml(row) {
  if (row.error) return `<div class="surface-row-error">${esc(row.error)}</div>`;
  if (row.result) return `<div class="surface-row-result">${esc(row.result)}</div>`;
  return "";
}

function callSequenceAttribute(row) {
  return Number.isFinite(row.callSequence) ? ` data-call-sequence="${esc(row.callSequence)}"` : "";
}

export function agentRowHtml(row) {
  return surfaceRowHtml("surface-agent", row, {
    attributes: callSequenceAttribute(row),
    trailing: row.model ? `<span class="surface-row-model">${esc(row.model)}</span>` : "",
    body: `${row.lastTool ? `<div class="surface-row-tool">${esc(row.lastTool)}</div>` : ""}
    ${agentStatsHtml(row)}
    ${agentOutcomeHtml(row)}`,
  });
}

/// One pill, and the cap its count rides in at the pill's end.
///
/// The cap is always in the markup and never taken out: `data-motion` says its
/// shown-ness belongs to `core/motion.js`, which grows it into place when a
/// count arrives and shrinks it away when the last of the work finishes — the
/// pill closing up behind it rather than jumping a cap's width.
export function surfacePillHtml(pill, openKind) {
  return `<button type="button" class="surface-pill" data-motion data-surface-kind="${esc(pill.kind)}"
    aria-pressed="${pill.kind === openKind}">
    <span class="surface-pill-label">${esc(pill.label)}</span>
    <span class="${PILL_COUNT_CLASS}" data-motion${pill.count ? "" : " hidden"}>${pill.count ? esc(pill.count) : ""}</span>
  </button>`;
}

export function workflowPhaseHtml(phase) {
  return `<button type="button" class="surface-phase" data-key="${esc(phase.key)}" data-phase-index="${esc(phase.index)}"
    aria-pressed="${phase.selected}">
    <span class="surface-phase-title">${esc(phase.title)}</span>
    <span class="surface-phase-count">${esc(phase.done)}/${esc(phase.total)}</span>
  </button>`;
}

export function workflowChoiceHtml(choice) {
  return `<button type="button" class="surface-workflow-choice" data-key="${esc(choice.key)}"
    data-workflow-index="${esc(choice.index)}" aria-pressed="${choice.selected}">
    ${stateMarkHtml(choice.stateMark)}
    <span class="surface-phase-title">${esc(choice.subject)}</span>
  </button>`;
}

export function workflowHeadHtml(workflow) {
  return rowHeadHtml(workflow, {
    headClass: WORKFLOW_HEAD_CLASS,
    trailing: noteHtml(workflow.description, workflow.subject),
  });
}

export function workflowViewerHtml(workflow, choices, phases, agents) {
  return `<div class="${VIEWER_CLASS.viewer} surface-workflow">
    <div class="${VIEWER_CLASS.workflowChoices}">${choices.map(workflowChoiceHtml).join("")}</div>
    ${workflowHeadHtml(workflow)}
    <div class="surface-workflow-body">
      <div class="${VIEWER_CLASS.workflowPhases}">${phases.map(workflowPhaseHtml).join("")}</div>
      <div class="${VIEWER_CLASS.workflowAgents}">${agents.map(agentRowHtml).join("")}</div>
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
