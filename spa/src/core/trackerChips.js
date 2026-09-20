// The small marks an issue wears, wherever it is drawn: on a list row, on a
// kanban card, and at the head of its own page.
//
// One writer each, so the same issue reads the same way in all three places —
// and so the two facts that are easiest to conflate stay apart. Open/closed is
// NOT the Done column: closing does not move an issue to Done and moving it to
// Done does not close it, so the state wears a dot of its own and the column
// wears a chip of its own, and both are always shown.
//
// Everything here is escaped. Nothing here reads the app or the DOM.

import { esc, humanAge } from "./text.js";
import { COLUMN_NOTE_SHARED, actorLabel, columnName, priorityIsMarked, priorityLabel, stateLabel } from "./trackerModel.js";

/** Open or closed, as a dot and its accessible name. A closed issue is drawn
 *  quiet rather than absent: it is still the project's history. */
/** Open or closed, as a dot and its accessible name. The dot is where a reader
 *  meets the other half of the board's surprise, so it carries the rule with
 *  it: this mark and the column move independently. */
export const stateDotHtml = (state) => {
  const label = stateLabel(state);
  const said = `${label}. ${COLUMN_NOTE_SHARED}`;
  return `<span class="issue-state issue-state-${state === "closed" ? "closed" : "open"}" role="img" aria-label="${esc(said)}" title="${esc(said)}"></span>`;
};

/** The column an issue stands in. A chip and not a dot, because a column is a
 *  word — "In review" says something "amber" cannot. */
export const statusChipHtml = (columns, status) =>
  `<span class="issue-status">${esc(columnName(columns, status))}</span>`;

/** Its labels, in the order the record carries them. Free strings the user
 *  typed, so every one of them is escaped and none of them is interpreted. */
export const labelsHtml = (labels) =>
  (labels || []).map((label) => `<span class="issue-label">${esc(label)}</span>`).join("");

/** Its priority — only when it is one worth a mark. A board where every card
 *  wears a chip says nothing with one, so `none` and `low` wear none. */
export const priorityChipHtml = (priority) =>
  priorityIsMarked(priority)
    ? `<span class="issue-priority issue-priority-${esc(priority)}">${esc(priorityLabel(priority))}</span>`
    : "";

/** Who holds it. Unassigned is a state worth showing rather than a blank: an
 *  issue nobody holds is the one most likely to need somebody. */
export const assigneeHtml = (assignee, agentLabels) => {
  const label = assignee ? actorLabel(assignee, agentLabels) : "Unassigned";
  return `<span class="issue-assignee${assignee ? "" : " issue-unassigned"}">${esc(label)}</span>`;
};

/** When it last moved, in the app's own human scale. An unparseable or absent
 *  stamp says nothing rather than "just now" — a wrong time reads as a fact. */
export function ageHtml(stamp, nowMs = Date.now()) {
  const at = Date.parse(stamp || "");
  if (!Number.isFinite(at)) return "";
  return `<time class="issue-age" datetime="${esc(stamp)}">${esc(humanAge((nowMs - at) / 1000))}</time>`;
}

/** The plain-text version of the same, for a title attribute or a sentence. */
export function ageText(stamp, nowMs = Date.now()) {
  const at = Date.parse(stamp || "");
  return Number.isFinite(at) ? humanAge((nowMs - at) / 1000) : "";
}

/** `#12`, which is how an issue is named everywhere a person says it aloud. */
export const numberHtml = (issue) => `<span class="issue-number">#${esc(String(issue.number ?? ""))}</span>`;
