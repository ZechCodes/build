import { esc } from "./text.js";
import { menuButtonMarkup } from "./splitButton.js";
import { outcomeMarkHtml } from "./outcomeMark.js";
import { rowActions } from "./agentSurfacesModel.js";

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
  return `<div class="surface-row surface-agent"${callSequenceAttribute(row)}>
    <div class="surface-row-head">
      ${stateMarkHtml(row.stateMark)}
      <span class="surface-row-label">${esc(row.label)}</span>
      ${row.model ? `<span class="surface-row-model">${esc(row.model)}</span>` : ""}
      ${actionMenuHtml("subagents", row)}
    </div>
    ${row.lastTool ? `<div class="surface-row-tool">${esc(row.lastTool)}</div>` : ""}
    ${agentStatsHtml(row)}
    ${agentOutcomeHtml(row)}
  </div>`;
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

function phaseHtml(phase) {
  return `<button type="button" class="surface-phase" data-phase-index="${esc(phase.index)}"
    aria-pressed="${phase.selected}">
    <span class="surface-phase-title">${esc(phase.title)}</span>
    <span class="surface-phase-count">${esc(phase.done)}/${esc(phase.total)}</span>
  </button>`;
}

export function workflowViewerHtml(workflow, phases, agents) {
  return `<div class="surface-viewer surface-workflow">
    <div class="surface-workflow-head">
      ${stateMarkHtml(workflow.stateMark)}
      <span class="surface-row-label">${esc(workflow.name)}</span>
      ${workflow.description ? `<span class="surface-row-note">${esc(workflow.description)}</span>` : ""}
      ${actionMenuHtml("workflows", workflow)}
    </div>
    <div class="surface-workflow-body">
      <div class="surface-phases">${phases.map(phaseHtml).join("")}</div>
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

function shellRowHtml(row) {
  return `<div class="surface-row surface-shell">
    <div class="surface-row-head">
      ${stateMarkHtml(row.stateMark)}
      <span class="surface-row-label">${esc(row.description)}</span>
      ${Number.isFinite(row.exitCode) ? statHtml(`exit ${row.exitCode}`) : ""}
      ${actionMenuHtml("shells", row)}
    </div>
    ${shellTailHtml(row.tail)}
  </div>`;
}

export function shellViewerHtml(rows) {
  return `<div class="surface-viewer surface-shells">${rows.map(shellRowHtml).join("")}</div>`;
}

function checklistItemHtml(row) {
  return `<div class="surface-row surface-checklist-item">
    <div class="surface-row-head">
      ${stateMarkHtml(row.stateMark)}
      <span class="surface-row-label">${esc(row.subject)}</span>
      ${actionMenuHtml("checklist", row)}
    </div>
    ${row.description ? `<div class="surface-row-note">${esc(row.description)}</div>` : ""}
  </div>`;
}

export function checklistViewerHtml(rows) {
  return `<div class="surface-viewer surface-checklist">${rows.map(checklistItemHtml).join("")}</div>`;
}
