import { esc } from "./text.js";
import { menuButtonMarkup } from "./splitButton.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
} from "./agentSurfacesModel.js";

const ACTION_MENU_LABEL = "Ask";
const ACTION_MENU_TITLE = "Ask the agent about this";

const WORKFLOW_HEAD_CLASS = "surface-workflow-head";

const KEYED_LIST_CLASS_THIS_MODULE_EMITS = {
  workflowChoices: "surface-workflows",
  workflowPhases: "surface-phases",
  workflowAgents: "surface-phase-agents",
  [AGENT_ENTRY_KIND]: "surface-subagents",
  [SHELL_ENTRY_KIND]: "surface-shells",
  [CHECKLIST_ENTRY_KIND]: "surface-checklist",
};

export const WORKFLOW_HEAD_SELECTOR = `.${WORKFLOW_HEAD_CLASS}`;

export const SURFACE_LIST_SELECTOR = Object.fromEntries(
  Object.entries(KEYED_LIST_CLASS_THIS_MODULE_EMITS).map(([name, className]) => [name, `.${className}`]),
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

function surfaceRowHtml(rowClass, row, { attributes = "", trailing = "", body = "" } = {}) {
  return `<div class="surface-row ${rowClass}" data-key="${esc(row.key)}"${attributes}>
    <div class="surface-row-head">
      ${stateMarkHtml(row.stateMark)}
      <span class="surface-row-label">${esc(row.subject)}</span>
      ${trailing}
      ${actionMenuHtml(row.actions)}
    </div>
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

function pillHtml(pill, openKind) {
  return `<button type="button" class="surface-pill" data-surface-kind="${esc(pill.kind)}"
    aria-pressed="${pill.kind === openKind}">
    <span class="surface-pill-label">${esc(pill.label)}</span>
    <span class="surface-pill-count">${esc(pill.count)}</span>
    ${pill.live ? `<span class="sdot sdot-working"></span>` : ""}
  </button>`;
}

export function surfacePillsHtml(pills, openKind) {
  if (!pills.length) return "";
  return `<div class="surface-pills" role="group" aria-label="Agent surfaces">${pills
    .map((pill) => pillHtml(pill, openKind))
    .join("")}</div>`;
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
  const subject = workflow.subject || "";
  return `<div class="${WORKFLOW_HEAD_CLASS}">
      ${stateMarkHtml(workflow.stateMark)}
      <span class="surface-row-label">${esc(subject)}</span>
      ${noteHtml(workflow.description, subject)}
      ${actionMenuHtml(workflow.actions)}
    </div>`;
}

export function workflowViewerHtml(workflow, choices, phases, agents) {
  return `<div class="surface-viewer surface-workflow">
    <div class="${KEYED_LIST_CLASS_THIS_MODULE_EMITS.workflowChoices}">${choices.map(workflowChoiceHtml).join("")}</div>
    ${workflowHeadHtml(workflow)}
    <div class="surface-workflow-body">
      <div class="${KEYED_LIST_CLASS_THIS_MODULE_EMITS.workflowPhases}">${phases.map(workflowPhaseHtml).join("")}</div>
      <div class="${KEYED_LIST_CLASS_THIS_MODULE_EMITS.workflowAgents}">${agents.map(agentRowHtml).join("")}</div>
    </div>
  </div>`;
}

export function kindViewerHtml(kind, rows, renderRow) {
  return `<div class="surface-viewer ${KEYED_LIST_CLASS_THIS_MODULE_EMITS[kind]}">${rows.map(renderRow).join("")}</div>`;
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
