// The tracker's record vocabulary: the columns a board draws, the priorities,
// and the one tagged actor shape that names a person or an agent.

import { describe, expect, it } from "vitest";
import {
  FALLBACK_COLUMNS,
  UNASSIGNED,
  actorInitials,
  actorLabel,
  assigneeFromKey,
  assigneeKey,
  columnName,
  columnsOf,
  issueLinks,
  priorityIsMarked,
  priorityLabel,
  sameAssignee,
  stateLabel,
} from "../src/core/trackerModel.js";

describe("the columns a board draws", () => {
  it("draws the five of phase 1 before issues.columns has answered", () => {
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

  it("drops a column with no slug at all — there is nothing to store on an issue", () => {
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
  it("says which of the two an issue is", () => {
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

  // `none` is the wire's own word for unassigned in issues.list, so the
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

  it("names an actor, using what the caller knows about this project's agents", () => {
    expect(actorLabel({ kind: "user" })).toBe("You");
    expect(actorLabel({ kind: "project_agent" })).toBe("Project agent");
    expect(actorLabel({ kind: "agent", agent_id: "agent-7" }, { "agent-7": "wire-facade · Agent 1" }))
      .toBe("wire-facade · Agent 1");
    expect(actorLabel(null)).toBe("Unassigned");
  });

  // An agent nobody can name still reads as an agent: a timeline entry with no
  // actor on it reads as an accident.
  it("falls back to four characters of an unnamed agent's id", () => {
    expect(actorLabel({ kind: "agent", agent_id: "agent-01K5ZABCDEF" })).toBe("Agent 01K5");
    expect(actorLabel({ kind: "agent", agent_id: "" })).toBe("Agent");
  });

  it("wears one initial in a circle", () => {
    expect(actorInitials({ kind: "user" })).toBe("Y");
    expect(actorInitials({ kind: "project_agent" })).toBe("P");
    expect(actorInitials({ kind: "agent", agent_id: "agent-7" })).toBe("A");
    expect(actorInitials(null)).toBe("–");
  });
});

describe("an issue's links", () => {
  it("answers every list, so a rail never asks whether it has one", () => {
    expect(issueLinks({ links: { branches: ["build/x"] } })).toEqual({
      workspace_ids: [], branches: ["build/x"], commits: [], conversation_ids: [], parent_issue_id: null,
    });
    expect(issueLinks(null).workspace_ids).toEqual([]);
  });
});
