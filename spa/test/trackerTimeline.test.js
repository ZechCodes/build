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

  // Through the tracker's one naming function since #63, so the sentence here
  // and the notice line in the conversation call the same actor the same
  // thing — the project's own agent is named after its project.
  it("names who an assign handed it to", () => {
    expect(sentenceOf("assigned", { assignee: { kind: "project_agent" } }, { projectName: "Build" }))
      .toBe("assigned this to Build");
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

  // #87: Done that deleted the branch says which, on every issue linking it.
  it("says which branch Done deleted", () => {
    expect(sentenceOf("branch_deleted", { branch: "build/login", workspace_id: "ws-1" }))
      .toBe("deleted branch build/login when the workspace was finished");
    expect(sentenceOf("branch_deleted", {})).toBe("deleted the branch when the workspace was finished");
    expect(sentenceOf("branch_deleted", { branch: "build/login", reason: "restore failed at abc" }))
      .toContain("restore failed at abc");
  });

  // #167: a reclaim takes the workspace's branch where that is safe, and says
  // why it stayed where it was not.
  it("says what a reclaim did with the workspace's branch", () => {
    expect(sentenceOf("branch_deleted", { branch: "build/login", reclaimed: true }))
      .toBe("deleted branch build/login when the workspace was reclaimed");
    expect(sentenceOf("branch_kept", {
      branch: "build/login",
      reclaimed: true,
      reason: "Build cannot delete the branch build/login: it has commits no remote has.",
    })).toBe("kept branch build/login when the workspace was reclaimed. Build cannot delete the branch build/login: it has commits no remote has.");
    expect(sentenceOf("branch_kept", {})).toBe("kept the branch when the workspace was reclaimed");
    // A workspace over several repositories: which one it happened in.
    expect(sentenceOf("branch_deleted", { branch: "build/x", reclaimed: true, repository: "/home/ada/code/assets" }))
      .toBe("deleted branch build/x in assets when the workspace was reclaimed");
    expect(sentenceOf("branch_kept", { branch: "build/x", reclaimed: true, repository: "/home/ada/code/build/", reason: "Why." }))
      .toBe("kept branch build/x in build when the workspace was reclaimed. Why.");
  });

  // The reclaim service (#135) records a linked workspace going quiet, losing
  // its build output, and being reclaimed.
  it("says what became of a linked workspace", () => {
    expect(sentenceOf("workspace_idle", { workspace_name: "quiet" }))
      .toBe("noted workspace quiet has had no activity for a day");
    // #167: the threshold is a setting, and the event says which it was.
    expect(sentenceOf("workspace_idle", { workspace_name: "quiet", idle_after_secs: 86_400 }))
      .toBe("noted workspace quiet has had no activity for a day");
    expect(sentenceOf("workspace_idle", { workspace_name: "quiet", idle_after_secs: 6 * 3600 }))
      .toBe("noted workspace quiet has had no activity for 6 hours");
    expect(sentenceOf("workspace_idle", { workspace_name: "quiet", idle_after_secs: 3 * 86_400 }))
      .toBe("noted workspace quiet has had no activity for 3 days");
    expect(sentenceOf("workspace_idle", { workspace_name: "quiet", idle_after_secs: 3600 }))
      .toBe("noted workspace quiet has had no activity for an hour");
    expect(sentenceOf("workspace_idle", { workspace_name: "quiet", idle_after_secs: 90 * 60 }))
      .toBe("noted workspace quiet has had no activity for 90 minutes");
    expect(sentenceOf("workspace_pruned", { workspace_name: "quiet", pruned_bytes: 12_000_000_000 }))
      .toBe("dropped 12.0 GB of build output from workspace quiet");
    expect(sentenceOf("workspace_reclaimed", { workspace_name: "quiet", size_bytes: 640_000_000 }))
      .toBe("reclaimed workspace quiet (640 MB)");
    expect(sentenceOf("workspace_reclaimed", { workspace_name: "quiet", size_bytes: null }))
      .toBe("reclaimed workspace quiet");
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
