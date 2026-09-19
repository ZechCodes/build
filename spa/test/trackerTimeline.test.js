// An issue's timeline: comments and events interleaved, in the order the
// bridge answered them.

import { describe, expect, it } from "vitest";
import { commentRows, eventSentence, timelineRows } from "../src/core/trackerTimeline.js";

const comment = (over = {}) => ({
  type: "comment",
  id: "ic-2",
  issue_id: "issue-1",
  author: { kind: "agent", agent_id: "agent-7" },
  body: "starting on this",
  refs: [],
  created_at: "2026-08-21T10:01:00Z",
  ...over,
});

const event = (over = {}) => ({
  type: "event",
  id: "ie-1",
  issue_id: "issue-1",
  at: "2026-08-21T10:00:00Z",
  actor: { kind: "user" },
  kind: "created",
  payload: {},
  ...over,
});

describe("reading the timeline", () => {
  // A comment was written (`created_at`); an event happened (`at`). Two keys on
  // purpose, one row shape for the renderer.
  it("reads a comment's stamp and an event's, which are different keys", () => {
    expect(timelineRows([comment(), event()]).map((row) => row.at)).toEqual([
      "2026-08-21T10:01:00Z", "2026-08-21T10:00:00Z",
    ]);
  });

  it("normalizes a comment's author and an event's actor to one field", () => {
    const [written, happened] = timelineRows([comment(), event()]);
    expect(written.actor).toEqual({ kind: "agent", agent_id: "agent-7" });
    expect(happened.actor).toEqual({ kind: "user" });
  });

  // The list arrives ascending by (timestamp, id) and ids are time-ordered, so
  // re-sorting on the timestamp alone would scramble same-second pairs.
  it("keeps the order it arrived in, never re-sorting", () => {
    const entries = [
      event({ id: "ie-1", at: "2026-08-21T10:00:00Z" }),
      comment({ id: "ic-1", created_at: "2026-08-21T10:00:00Z" }),
      event({ id: "ie-2", at: "2026-08-21T10:00:00Z", kind: "moved" }),
    ];
    expect(timelineRows(entries).map((row) => row.key)).toEqual(["ie-1", "ic-1", "ie-2"]);
  });

  it("keys each row by the record's own id, so a keyed repaint holds its place", () => {
    expect(timelineRows([comment({ id: "ic-9" })])[0].key).toBe("ic-9");
    expect(timelineRows([comment({ id: undefined })])[0].key).toBe("comment-0");
  });

  // A later minor adding a type must not put an empty row in a reader's history.
  it("drops an entry of a type this client has never heard of", () => {
    expect(timelineRows([{ type: "reaction" }, comment()]).map((row) => row.type)).toEqual(["comment"]);
  });

  it("counts the comments alone", () => {
    expect(commentRows(timelineRows([comment(), event(), comment({ id: "ic-3" })]))).toHaveLength(2);
  });
});

describe("what an event says", () => {
  const sentenceOf = (kind, payload, options) =>
    eventSentence(timelineRows([event({ kind, payload })])[0], options);

  it("says each kind as the predicate of a sentence about its actor", () => {
    expect(sentenceOf("created", {})).toBe("filed this");
    expect(sentenceOf("unassigned", {})).toBe("unassigned this");
    expect(sentenceOf("reopened", {})).toBe("reopened this");
    expect(sentenceOf("dispatched", { agent_id: "agent-7" })).toBe("started an agent on this");
  });

  it("names the columns a move went between, by their display names", () => {
    expect(sentenceOf("moved", { from: "backlog", to: "in_progress" }))
      .toBe("moved this from Backlog to In progress");
  });

  // A card that moves on its own should say why.
  it("says when a move was an agent reporting Complete", () => {
    expect(sentenceOf("moved", { from: "in_progress", to: "in_review", by: "report" }))
      .toBe("moved this from In progress to In review on reporting Complete");
  });

  it("names who an assign handed it to", () => {
    expect(sentenceOf("assigned", { assignee: { kind: "project_agent" } })).toBe("assigned this to Project agent");
    expect(sentenceOf("assigned", { assignee: { kind: "agent", agent_id: "agent-7" } }, {
      agentLabels: { "agent-7": "wire-facade · Agent 1" },
    })).toBe("assigned this to wire-facade · Agent 1");
  });

  it("says what a label change added and removed", () => {
    expect(sentenceOf("labelled", { added: ["bug"], removed: [] })).toBe("added bug");
    expect(sentenceOf("labelled", { added: [], removed: ["ui"] })).toBe("removed ui");
    expect(sentenceOf("labelled", { added: ["bug"], removed: ["ui"] })).toBe("added bug and removed ui");
  });

  it("says what a link linked", () => {
    expect(sentenceOf("linked", { branch: "build/issues-spa" })).toBe("linked branch build/issues-spa");
    expect(sentenceOf("linked", {})).toBe("linked this");
  });

  // `workspace.finish` closes every open issue linking that workspace.
  it("says when a close came from a workspace being finished", () => {
    expect(sentenceOf("closed", { reason: "workspace_finished", workspace_id: "ws-1" }))
      .toBe("closed this when the workspace was finished");
    expect(sentenceOf("closed", {})).toBe("closed this");
  });

  // A later minor adding a kind leaves a reader with a row they can recognize.
  it("says an unknown kind's own name rather than nothing", () => {
    expect(sentenceOf("pinned", {})).toBe("pinned");
  });

  it("uses the project's own columns when the bridge named them", () => {
    expect(sentenceOf("moved", { from: "icebox", to: "shipping" }, {
      columns: [{ id: "icebox", name: "Icebox" }, { id: "shipping", name: "Shipping" }],
    })).toBe("moved this from Icebox to Shipping");
  });
});
