// The kanban, as a pure model: which tasks stand in which column, and what a
// move does.
//
// A column is not a filter somebody typed — it is what `status` says — so every
// column the project has is drawn whether or not a task stands in it. A
// task whose status names no column the bridge offers is not dropped: it gets
// a column of its own at the end, under its slug, because losing a card is
// worse than drawing one the client cannot name.
//
// Closing does not move a task to Done and moving it to Done does not close
// it, so nothing here reads `state`. The card says both; the board arranges one.
//
// No DOM, no app imports.

import { columnsOf } from "./trackerModel.js";

/** The board: one entry per column, in the columns' own order, each carrying
 *  the tasks standing in it in the order they were given (newest first, the
 *  order `tasks.list` answers in). */
export function boardColumns(columns, tasks) {
  const listed = columnsOf(columns);
  const buckets = new Map(listed.map((column) => [column.id, []]));
  const strays = new Map();
  for (const task of tasks || []) {
    const status = String(task?.status || "");
    if (buckets.has(status)) buckets.get(status).push(task);
    else strays.set(status, [...(strays.get(status) || []), task]);
  }
  return [
    ...listed.map((column) => ({ ...column, unknown: false, tasks: buckets.get(column.id) })),
    ...[...strays.entries()].map(([status, held]) => ({ id: status, name: status, unknown: true, tasks: held })),
  ];
}

/** Which column a task stands in, or null when its status names none. What a
 *  card reads to draw the column it is about to leave. */
export const columnOf = (columns, status) => columnsOf(columns).find((column) => column.id === String(status || "")) || null;

/**
 * The column `steps` away from this one, or null at the ends.
 *
 * The keyboard's half of dragging: a focused card moves left and right through
 * the columns, and stops rather than wrapping — wrapping from Done to Backlog
 * is never what a repeated key press meant.
 */
export function nextColumn(columns, status, steps) {
  const listed = columnsOf(columns);
  const at = listed.findIndex((column) => column.id === String(status || ""));
  if (at === -1) return listed[0] || null;
  const moved = at + Number(steps || 0);
  if (moved < 0 || moved >= listed.length) return null;
  return listed[moved];
}

/** A move, as `tasks.update` params. One field — moving a card is what a move
 *  is, and sending the whole record would invite rewriting a title nobody
 *  touched. */
export const moveParams = (taskId, status) => ({ task_id: taskId, status });

/**
 * The board as it will be once a move lands.
 *
 * The card moves optimistically and the column repaints from the push, so the
 * same move is applied twice — here, and again when the pushed list arrives.
 * Applying it to the task list rather than to the columns keeps the two passes
 * identical: both are "this task's status is now that".
 */
export const withMovedTask = (tasks, taskId, status) =>
  (tasks || []).map((task) => (task.id === taskId ? { ...task, status } : task));
