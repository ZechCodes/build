import { esc } from "./text.js";
import { menuButtonMarkup } from "./splitButton.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import {
  AGENT_ENTRY_KIND,
  CHECKLIST_ENTRY_KIND,
  SHELL_ENTRY_KIND,
  WORKFLOW_ENTRY_KIND,
  rowActions,
  rowSubject,
} from "./agentSurfacesModel.js";

const ACTION_MENU_LABEL = "Ask";
const ACTION_MENU_TITLE = "Ask the agent about this";

function stateMarkHtml(stateMark) {
  return stateMark ? outcomeMarkHtml(stateMark.mark, stateMark.label) : "";
}

function actionMenuHtml(kind, row) {
  const actions = rowActions(kind, row);
  if (!actions.length) return "";
  return menuButtonMarkup(ACTION_MENU_LABEL, actions, { title: ACTION_MENU_TITLE });
}

function statHtml(text) {
  return `<span class="surface-row-stat">${esc(text)}</span>`;
}

function noteHtml(description, subject) {
  if (!description || description === subject) return "";
  return `<span class="surface-row-note">${esc(description)}</span>`;
}

function surfaceRowHtml(rowClass, kind, row, { attributes = "", trailing = "", body = "" } = {}) {
  return `<div class="surface-row ${rowClass}" data-key="${esc(row.key)}"${attributes}>
    <div class="surface-row-head">
      ${stateMarkHtml(row.stateMark)}
      <span class="surface-row-label">${esc(rowSubject(kind, row))}</span>
      ${trailing}
      ${actionMenuHtml(kind, row)}
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
  return surfaceRowHtml("surface-agent", AGENT_ENTRY_KIND, row, {
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

export function workflowHeadHtml(workflow) {
  const subject = rowSubject(WORKFLOW_ENTRY_KIND, workflow);
  return `<div class="surface-workflow-head">
      ${stateMarkHtml(workflow.stateMark)}
      <span class="surface-row-label">${esc(subject)}</span>
      ${noteHtml(workflow.description, subject)}
      ${actionMenuHtml(WORKFLOW_ENTRY_KIND, workflow)}
    </div>`;
}

export function workflowViewerHtml(workflow, phases, agents) {
  return `<div class="surface-viewer surface-workflow">
    ${workflowHeadHtml(workflow)}
    <div class="surface-workflow-body">
      <div class="surface-phases">${phases.map(workflowPhaseHtml).join("")}</div>
      <div class="surface-phase-agents">${agents.map(agentRowHtml).join("")}</div>
    </div>
  </div>`;
}

export function subagentViewerHtml(rows) {
  return `<div class="surface-viewer surface-subagents">${rows.map(agentRowHtml).join("")}</div>`;
}

function shellTailHtml(tail) {
  if (!tail.length) return "";
  return `<details class="surface-shell-tail">
    <summary class="surface-shell-tail-head">Output</summary>
    <pre class="surface-shell-tail-body">${esc(tail.join("\n"))}</pre>
  </details>`;
}

export function shellRowHtml(row) {
  return surfaceRowHtml("surface-shell", SHELL_ENTRY_KIND, row, {
    trailing: Number.isFinite(row.exitCode) ? statHtml(`exit ${row.exitCode}`) : "",
    body: shellTailHtml(row.tail),
  });
}

export function shellViewerHtml(rows) {
  return `<div class="surface-viewer surface-shells">${rows.map(shellRowHtml).join("")}</div>`;
}

export function checklistItemHtml(row) {
  return surfaceRowHtml("surface-checklist-item", CHECKLIST_ENTRY_KIND, row, {
    body: noteHtml(row.description, rowSubject(CHECKLIST_ENTRY_KIND, row)),
  });
}

export function checklistViewerHtml(rows) {
  return `<div class="surface-viewer surface-checklist">${rows.map(checklistItemHtml).join("")}</div>`;
}
