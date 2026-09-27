// The kanban's model: which tasks stand in which column, and what a move does.

import { describe, expect, it } from "vitest";
import { boardColumns, columnOf, moveParams, nextColumn, withMovedTask } from "../src/core/trackerBoardModel.js";

const task = (number, status, over = {}) => ({ id: `task-${number}`, number, status, state: "open", ...over });

describe("the board", () => {
  it("draws one column per column the project has, in their order", () => {
    expect(boardColumns(null, []).map((column) => column.id)).toEqual([
      "backlog", "ready", "in_progress", "in_review", "done",
    ]);
  });

  // A column is not a filter somebody typed — it is what `status` says.
  it("draws an empty column, because a column is not a filter", () => {
    const board = boardColumns(null, [task(1, "backlog")]);
    expect(board.find((column) => column.id === "done").tasks).toEqual([]);
  });

  it("puts each task in the column its status names, in the order given", () => {
    const board = boardColumns(null, [task(3, "ready"), task(2, "backlog"), task(1, "ready")]);
    expect(board.find((column) => column.id === "ready").tasks.map((one) => one.number)).toEqual([3, 1]);
  });

  // Losing a card is worse than drawing one the client cannot name.
  it("gives a task whose status names no column a column of its own, at the end", () => {
    const board = boardColumns(null, [task(1, "icebox")]);
    const stray = board[board.length - 1];
    expect([stray.id, stray.unknown, stray.tasks.map((one) => one.number)]).toEqual(["icebox", true, [1]]);
    expect(board).toHaveLength(6);
  });

  // Closing does not move a task to Done and Done does not close it, so the
  // board arranges on status alone.
  it("arranges on status and never on state", () => {
    const board = boardColumns(null, [task(1, "in_progress", { state: "closed" })]);
    expect(board.find((column) => column.id === "in_progress").tasks.map((one) => one.number)).toEqual([1]);
    expect(board.find((column) => column.id === "done").tasks).toEqual([]);
  });

  it("uses the columns the bridge answered when it has answered", () => {
    const columns = [{ id: "icebox", name: "Icebox" }, { id: "shipping", name: "Shipping" }];
    expect(boardColumns(columns, [task(1, "icebox")]).map((column) => column.name)).toEqual(["Icebox", "Shipping"]);
  });
});

describe("moving a card", () => {
  it("names the column a status stands in", () => {
    expect(columnOf(null, "in_review").name).toBe("In review");
    expect(columnOf(null, "icebox")).toBeNull();
  });

  // The keyboard's half of dragging.
  it("steps left and right through the columns", () => {
    expect(nextColumn(null, "ready", 1).id).toBe("in_progress");
    expect(nextColumn(null, "ready", -1).id).toBe("backlog");
  });

  // Wrapping from Done to Backlog is never what a repeated key press meant.
  it("stops at the ends rather than wrapping", () => {
    expect(nextColumn(null, "backlog", -1)).toBeNull();
    expect(nextColumn(null, "done", 1)).toBeNull();
  });

  it("starts a task standing outside the columns at the first one", () => {
    expect(nextColumn(null, "icebox", 1).id).toBe("backlog");
  });

  // Moving a card is what a move is; the whole record would invite rewriting a
  // title nobody touched.
  it("is tasks.update narrowed to one field", () => {
    expect(moveParams("task-1", "in_review")).toEqual({ task_id: "task-1", status: "in_review" });
  });

  it("applies the move to the list, so the optimistic paint and the pushed one agree", () => {
    const tasks = [task(1, "backlog"), task(2, "ready")];
    expect(withMovedTask(tasks, "task-1", "done").map((one) => one.status)).toEqual(["done", "ready"]);
    expect(tasks[0].status).toBe("backlog");
  });
});
