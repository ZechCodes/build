// The small marks a task wears, wherever it is drawn: on a list row, on a
// kanban card, and at the head of its own page.
//
// One writer each, so the same task reads the same way in all three places —
// and so the two facts that are easiest to conflate stay apart. Open/closed is
// NOT the Done column: closing does not move a task to Done and moving it to
// Done does not close it, so the state wears a mark of its own and the column
// wears a chip of its own, and both are always shown.
//
// Everything here is escaped. Nothing here reads the app or the DOM.

import { esc, humanAge } from "./text.js";
import {
  COLUMN_NOTE_SHARED,
  columnName,
  priorityIsMarked,
  priorityIsPressing,
  priorityLabel,
  stateLabel,
} from "./trackerModel.js";
import { actorName } from "./trackerLineWords.js";
import { taskAvatarHtml } from "./taskAvatar.js";
import { ICON_SQUARE, ICON_SQUARE_CHECK, ICON_SQUARE_SLASH } from "./icons.js";

/** A task's mark is a checkbox (#190). Its shape says how far the work got:
 *  empty, checked once it is done, slashed when it was closed without being
 *  done — not planned. Its colour says open or closed, whatever the shape
 *  (`.task-state-open`/`-closed`), so a closed task in Done still reads closed. */
const MARK_SHAPES = Object.freeze({ open: ICON_SQUARE, done: ICON_SQUARE_CHECK, closed: ICON_SQUARE_SLASH });

const shapeOf = ({ state, status }) => {
  if (status === "done") return "done";
  return state === "closed" ? "closed" : "open";
};

const openOrClosed = ({ state }) => (state === "closed" ? "closed" : "open");

/** Open or closed, as a checkbox and its accessible name. A closed task is
 *  drawn quiet rather than absent: it is still the project's history. The
 *  mark is where a reader meets the other half of the board's surprise, so it
 *  carries the rule with it: open/closed and the column move independently. */
export const stateMarkHtml = (task) => {
  const shape = shapeOf(task);
  const done = shape === "done" ? ", done" : "";
  const said = `${stateLabel(task.state)}${done}. ${COLUMN_NOTE_SHARED}`;
  return `<span class="task-state task-state-${openOrClosed(task)} task-mark-${shape}" role="img" aria-label="${esc(said)}" title="${esc(said)}">${MARK_SHAPES[shape]}</span>`;
};

/**
 * The mark a CLOSED task wears on a list row, and nothing at all for an open
 * one (#33).
 *
 * The list opens on open tasks, so a closed row is only ever on screen
 * because the reader asked for one — but once it is there it must not be
 * mistakable for an open one, and the row lost its state dot with the dots
 * (#28). A chip, in that same vocabulary: a word, at the head of line two,
 * before the column it stands in.
 *
 * Only the closed half is drawn. "Open" on every other row is a word the
 * reader learns to skip, which is the mistake the assignee made before it.
 */
export const closedChipHtml = (state) =>
  state === "closed" ? `<span class="task-closed">${esc(stateLabel(state))}</span>` : "";

/** The column a task stands in. A chip and not a dot, because a column is a
 *  word — "In review" says something "amber" cannot. `withWhom` follows it
 *  when the column is waiting on somebody. */
export const statusChipHtml = (columns, status, withWhom = "") =>
  `<span class="task-status">${esc(columnName(columns, status))}${withWhom ? ` · ${esc(withWhom)}` : ""}</span>`;

/**
 * Who a task in review is with (#144): "you", or the reviewer's name, from
 * the cached assignee. A task in review is assigned to whoever is reviewing
 * it, so the column alone does not say it is waiting on the user. Nothing for
 * any other column, or for a task nobody holds.
 */
export const reviewerWords = (task, reading) => {
  if (task?.status !== "in_review" || !task.assignee) return "";
  return task.assignee.kind === "user" ? "you" : actorName(task.assignee, reading);
};

/** Its labels, in the order the record carries them. Free strings the user
 *  typed, so every one of them is escaped and none of them is interpreted. */
export const labelsHtml = (labels) =>
  (labels || []).map((label) => `<span class="task-label">${esc(label)}</span>`).join("");

/** How many labels a row shows before it stops counting them out (#45). Three
 *  is what fits beside the rest of line two at a phone's width; the rest are
 *  a number, and the number carries their names in its title. */
export const ROW_LABEL_LIMIT = 3;

/**
 * Its labels, on a LIST ROW.
 *
 * #45: "the task list still feels cluttered." Labels were the loudest
 * thing on the row — a pill each, bordered, however many the task wore — and
 * a dozen rows of them read as a wall. So: small muted words, spaced rather
 * than boxed, three of them, and `+n` for the rest. They are still every label
 * the task has; the ones past the third are behind a number rather than gone.
 *
 * Grouped in one element so the spacing between labels is the labels' own and
 * not line two's — they are one fact, read together.
 */
export function rowLabelsHtml(labels, limit = ROW_LABEL_LIMIT) {
  const all = (labels || []).filter(Boolean);
  if (!all.length) return "";
  const shown = all.slice(0, limit);
  const rest = all.slice(limit);
  const more = rest.length
    ? `<span class="task-label task-label-more" title="${esc(rest.join(", "))}">+${rest.length}</span>`
    : "";
  return `<span class="task-labels">${labelsHtml(shown)}${more}</span>`;
}

/** The mark a pressing priority wears on a list row: one stroke for high, two
 *  for urgent. Shape as well as colour, because a mark that is only a colour
 *  is not a mark to every reader. */
const PRIORITY_MARKS = Object.freeze({ high: "!", urgent: "!!" });

/**
 * Its priority, on a LIST ROW: a small mark before the title, and only when
 * the priority is pressing (#45).
 *
 * Before the TITLE rather than among the facts on line two, because priority
 * is how the row should be read and not another thing it says — the eye going
 * down a column of titles meets it on the way in. Everything at medium and
 * below wears nothing: a list where most rows carry a mark has no marks in it.
 */
export const priorityMarkHtml = (priority) =>
  priorityIsPressing(priority)
    ? `<span class="task-priority-mark task-priority-mark-${esc(priority)}" role="img" aria-label="${esc(priorityLabel(priority))} priority" title="${esc(priorityLabel(priority))} priority">${PRIORITY_MARKS[priority]}</span>`
    : "";

/** Its priority, as a chip — the board card's form. A board where every card
 *  wears a chip says nothing with one, so `none` and `low` wear none. */
export const priorityChipHtml = (priority) =>
  priorityIsMarked(priority)
    ? `<span class="task-priority task-priority-${esc(priority)}">${esc(priorityLabel(priority))}</span>`
    : "";

/** Who holds it. Unassigned is a state worth showing rather than a blank: on a
 *  card and on the task's own page, where one task is the subject and "who
 *  has this" is a question being answered. */
const assigneeProvider = (assignee, reading) => {
  const id = assignee?.agent_id;
  return reading.identities?.[id]?.provider || reading.agentProviders?.[id] || "";
};

const isProjectAssignee = (assignee) =>
  assignee?.kind === "project_agent" || Boolean(assignee?.agent_id?.startsWith("project-"));

export const assigneeHtml = (assignee, reading) => {
  const label = assignee ? actorName(assignee, reading) : "Unassigned";
  const icon = isProjectAssignee(assignee) || assigneeProvider(assignee, reading)
    ? taskAvatarHtml(assignee, reading) : "";
  return `<span class="task-assignee${assignee ? "" : " task-unassigned"}">${icon}${esc(label)}</span>`;
};

/**
 * Who holds it, on a LIST row, where the same word on every unheld row is a
 * word the reader learns to skip.
 *
 * So an unheld row says nothing about its assignee and offers the press
 * instead — quiet until the row is reached, which the stylesheet does rather
 * than this: an affordance that is not in the markup is one the keyboard and
 * a screen reader cannot find either.
 */
export const rowAssigneeHtml = (assignee, reading) =>
  assignee ? assigneeHtml(assignee, reading) : `<span class="task-assign-cue">Assign</span>`;

/** What the press is called where it cannot be seen. It names the holder when
 *  there is one, because a control whose visible words are missing from its
 *  accessible name is one a speech-control user cannot say out loud. */
export const assignPressLabel = (task, reading) => {
  const named = `#${task?.number ?? ""}`;
  if (!task?.assignee) return `Assign ${named}`;
  return `${named} is assigned to ${actorName(task.assignee, reading)}. Assign it to somebody else`;
};

/** When it last moved, in the app's own human scale. An unparseable or absent
 *  stamp says nothing rather than "just now" — a wrong time reads as a fact. */
export function ageHtml(stamp, nowMs = Date.now()) {
  const at = Date.parse(stamp || "");
  if (!Number.isFinite(at)) return "";
  return `<time class="task-age" datetime="${esc(stamp)}">${esc(humanAge((nowMs - at) / 1000))}</time>`;
}

/** The plain-text version of the same, for a title attribute or a sentence. */
export function ageText(stamp, nowMs = Date.now()) {
  const at = Date.parse(stamp || "");
  return Number.isFinite(at) ? humanAge((nowMs - at) / 1000) : "";
}

/** `#12`, which is how a task is named everywhere a person says it aloud. */
export const numberHtml = (task) => `<span class="task-number">#${esc(String(task.number ?? ""))}</span>`;
