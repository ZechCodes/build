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

export function observationPanelHtml(model) {
  if (!model?.goal) return "";
  return `<div class="agent-observation-panel">${goalHtml(model.goal)}</div>`;
}
