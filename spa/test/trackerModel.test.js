// The tracker's record vocabulary: the columns a board draws, the priorities,
// and the one tagged actor shape that names a person or an agent.

import { describe, expect, it } from "vitest";
import {
  COLUMN_NOTE_SHARED,
  FALLBACK_COLUMNS,
  UNASSIGNED,
  actorInitials,
  assigneeFromKey,
  assigneeKey,
  columnName,
  columnNote,
  columnsOf,
  taskLinks,
  priorityIsMarked,
  priorityLabel,
  sameAssignee,
  stateLabel,
} from "../src/core/trackerModel.js";

describe("the columns a board draws", () => {
  it("draws the five of phase 1 before tasks.columns has answered", () => {
    expect(columnsOf(null).map((column) => column.id)).toEqual([
      "backlog", "ready", "in_progress", "in_review", "done",
    ]);
    expect(columnsOf([]).map((column) => column.name)).toEqual(FALLBACK_COLUMNS.map((column) => column.name));
  });

  it("takes the bridge's own columns, in the bridge's own order", () => {
    const answered = [{ id: "icebox", name: "Icebox" }, { id: "shipping", name: "Shipping" }];
    expect(columnsOf(answered)).toEqual(answered);
  });

  // A slug is stored, not an enum, so a later per-project column set is a
  // record change. A column with no display name is still a column.
  it("draws a column the bridge named without a name under its slug", () => {
    expect(columnsOf([{ id: "icebox" }])).toEqual([{ id: "icebox", name: "icebox" }]);
  });

  it("drops a column with no slug at all — there is nothing to store on a task", () => {
    expect(columnsOf([{ name: "Nowhere" }, { id: "ready", name: "Ready" }])).toEqual([{ id: "ready", name: "Ready" }]);
  });

  it("names the column a status stands in, and says the slug for one it cannot name", () => {
    expect(columnName(null, "in_review")).toBe("In review");
    expect(columnName([{ id: "icebox", name: "Icebox" }], "icebox")).toBe("Icebox");
    expect(columnName(null, "retired_column")).toBe("retired_column");
  });

  it("answers a fresh array, so a caller that sorts the columns cannot sort the fallback", () => {
    const first = columnsOf(null);
    first.reverse();
    expect(columnsOf(null)[0].id).toBe("backlog");
  });
});

describe("priorities", () => {
  it("names each one, and calls an unknown priority none", () => {
    expect(priorityLabel("urgent")).toBe("Urgent");
    expect(priorityLabel("none")).toBe("None");
    expect(priorityLabel("screaming")).toBe("None");
  });

  // A board where every card wears a chip says nothing with one.
  it("marks only the priorities worth a chip", () => {
    expect(["none", "low", "medium", "high", "urgent"].filter(priorityIsMarked)).toEqual([
      "medium", "high", "urgent",
    ]);
  });
});

describe("open and closed", () => {
  // Independent of the Done column: one says whether the work is live, the
  // other where it stands on the board.
  it("says which of the two a task is", () => {
    expect(stateLabel("open")).toBe("Open");
    expect(stateLabel("closed")).toBe("Closed");
  });
});

describe("the tagged actor shape", () => {
  it("keys each kind so two reads of the same agent compare equal", () => {
    expect(assigneeKey({ kind: "user" })).toBe("user");
    expect(assigneeKey({ kind: "project_agent" })).toBe("project_agent");
    expect(assigneeKey({ kind: "agent", agent_id: "agent-7" })).toBe("agent:agent-7");
    expect(sameAssignee({ kind: "agent", agent_id: "agent-7" }, { kind: "agent", agent_id: "agent-7" })).toBe(true);
    expect(sameAssignee({ kind: "agent", agent_id: "agent-7" }, { kind: "agent", agent_id: "agent-8" })).toBe(false);
  });

  // `none` is the wire's own word for unassigned in tasks.list, so the
  // filter's value and the param it becomes are one string.
  it("keys unassigned as the word the wire uses", () => {
    expect(UNASSIGNED).toBe("none");
    expect(assigneeKey(null)).toBe("none");
    expect(sameAssignee(null, undefined)).toBe(true);
  });

  it("reads a key back into the shape the wire takes", () => {
    expect(assigneeFromKey("user")).toEqual({ kind: "user" });
    expect(assigneeFromKey("project_agent")).toEqual({ kind: "project_agent" });
    expect(assigneeFromKey("agent:agent-7")).toEqual({ kind: "agent", agent_id: "agent-7" });
    expect(assigneeFromKey("none")).toBeNull();
  });

  // What to call an actor moved to core/trackerLineWords.js with the rest of
  // the tracker's vocabulary (#63) — test/trackerActorName.test.js holds it,
  // and holds every surface that prints one to agreeing with it.

  it("wears one initial in a circle", () => {
    expect(actorInitials({ kind: "user" })).toBe("Y");
    expect(actorInitials({ kind: "project_agent" })).toBe("P");
    expect(actorInitials({ kind: "agent", agent_id: "agent-7" })).toBe("A");
    expect(actorInitials(null)).toBe("–");
  });
});

describe("a task's links", () => {
  it("answers every list, so a rail never asks whether it has one", () => {
    expect(taskLinks({ links: { branches: ["build/x"] } })).toEqual({
      workspace_ids: [], branches: ["build/x"], commits: [], conversation_ids: [], parent_task_id: null,
    });
    expect(taskLinks(null).workspace_ids).toEqual([]);
  });
});

// The board is the one place the user meets rules the agents are told outright
// and the user never is. Every sentence is the Tasks Spec's own; nothing is
// invented for a column the spec only names.
describe("what a column means", () => {
  it("says of In review the thing a first-time reader gets wrong", () => {
    expect(columnNote(null, "in_review")).toContain("ready to be looked at, not that it is accepted");
  });

  it("says of Done that closing is a separate question", () => {
    expect(columnNote(null, "done")).toContain("Closing is separate");
  });

  // The spec states what a dispatch does to these two, and no more, so neither
  // does this.
  it("says of Backlog and Ready what assigning does to them", () => {
    expect(columnNote(null, "backlog")).toContain("moves it to In progress");
    expect(columnNote(null, "ready")).toContain("moves it to In progress");
  });

  // The one thing about this board that surprises people, under every column.
  it("carries the independence rule on every column", () => {
    for (const slug of ["backlog", "ready", "in_progress", "in_review", "done"]) {
      expect(columnNote(null, slug)).toContain(COLUMN_NOTE_SHARED);
    }
  });

  // A later per-project column set is a record change, and a board of
  // unexplained columns would be worse than one with a general note on each.
  it("still says something about a column this build has no words for", () => {
    expect(columnNote([{ id: "icebox", name: "Icebox" }], "icebox")).toBe(COLUMN_NOTE_SHARED);
    expect(columnNote(null, "")).toBe(COLUMN_NOTE_SHARED);
  });
});
