import { checklistItemHtml } from "./agentSurfacesRender.js";
import { esc } from "./text.js";

const notesHtml = (notes) => (notes || [])
  .map((note) => `<span class="agent-observation-note">${esc(note)}</span>`)
  .join("");

function goalHtml(goal) {
  if (!goal) return "";
  return `<section class="agent-observation-goal${goal.stale ? " is-stale" : ""}">
    <div class="agent-observation-heading">
      <strong>${esc(goal.status)}</strong>
      <span class="agent-observation-notes">${notesHtml(goal.notes)}</span>
    </div>
    <div class="agent-observation-objective">${esc(goal.objective)}</div>
  </section>`;
}

function checklistHtml(checklist) {
  if (!checklist) return "";
  const currentStep = checklist.currentStep
    ? `<span class="agent-observation-step">${esc(checklist.currentStep)}</span>`
    : "";
  return `<details class="agent-observation-checklist${checklist.stale ? " is-stale" : ""}">
    <summary>
      <span class="agent-observation-checklist-copy">
        <strong>Checklist</strong>
        ${currentStep}
      </span>
      <span class="agent-observation-checklist-meta">
        <span class="agent-observation-progress">${esc(checklist.progress)}</span>
        <span class="agent-observation-notes">${notesHtml(checklist.notes)}</span>
      </span>
    </summary>
    <div class="agent-observation-items">${checklist.rows.map(checklistItemHtml).join("")}</div>
  </details>`;
}

export function observationPanelHtml(model) {
  if (!model?.goal && !model?.checklist) return "";
  return `<div class="agent-observation-panel">${goalHtml(model.goal)}${checklistHtml(model.checklist)}</div>`;
}
