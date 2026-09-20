// The words an issue line says: what was done, and who did it (#40).
//
// Zech, on the rolled build: the rows read "commented_on #39" and "created #39"
// with no actor, and the project's own agent came out as "Agent 01M2" because
// it is not a workspace agent and the feed has no label for it.
//
// One table for both lines — the notice (#38) and the agent's own action line
// (#18) — because the same verb reaching a reader two ways with two spellings
// is how a vocabulary drifts.

import { describe, expect, it } from "vitest";
import { actionPhrase, actorName } from "../src/core/trackerLineWords.js";

describe("what was done", () => {
  it("says each verb the way a person says it", () => {
    const said = (token) => actionPhrase(token);
    expect(said("create")).toBe("created");
    expect(said("comment")).toBe("commented on");
    expect(said("assign")).toBe("assigned");
    expect(said("close")).toBe("closed");
    expect(said("reopen")).toBe("reopened");
    expect(said("link")).toBe("linked");
    expect(said("track")).toBe("tracked");
  });

  it("takes the past tense the bridge may already have applied", () => {
    expect(actionPhrase("created")).toBe("created");
    expect(actionPhrase("commented")).toBe("commented on");
    expect(actionPhrase("closed")).toBe("closed");
  });

  // The reported defect: `commented_on` reached the screen with the underscore
  // still in it.
  it("never leaves an underscore on screen", () => {
    expect(actionPhrase("commented_on")).toBe("commented on");
    expect(actionPhrase("comment_on")).toBe("commented on");
    // Even a verb this build has never heard of reads as words.
    expect(actionPhrase("hurled_at_wall")).toBe("hurled at wall");
    expect(actionPhrase("some_new_verb")).not.toContain("_");
  });

  // An edit is what a reader calls it; `issues.update` is what the wire calls
  // it, and the wire's word is not the one on screen.
  it("calls an update an edit", () => {
    expect(actionPhrase("update")).toBe("edited");
    expect(actionPhrase("updated")).toBe("edited");
    expect(actionPhrase("edit")).toBe("edited");
  });

  // The bridge writes a phrase where the verb needs a target, and a client
  // that rewrote those would need to know every column and every agent.
  it("passes a phrase through as it reads", () => {
    expect(actionPhrase("moved to In review")).toBe("moved to In review");
    expect(actionPhrase("assigned to issues-spa · Agent 1")).toBe("assigned to issues-spa · Agent 1");
  });

  it("says nothing for nothing", () => {
    expect(actionPhrase("")).toBe("");
    expect(actionPhrase(null)).toBe("");
  });

  // The action line begins a sentence; the notice line has the actor in front.
  it("can lead a sentence", () => {
    expect(actionPhrase("comment", { leading: true })).toBe("Commented on");
    expect(actionPhrase("moved to In review", { leading: true })).toBe("Moved to In review");
  });
});

describe("who did it", () => {
  const LABELS = { "agent-01M2A": "issues-spa · Agent 1" };

  it("names a workspace agent the way the rest of the project names it", () => {
    expect(actorName("agent-01M2A", { agentLabels: LABELS })).toBe("issues-spa · Agent 1");
    expect(actorName({ kind: "agent", agent_id: "agent-01M2A" }, { agentLabels: LABELS })).toBe("issues-spa · Agent 1");
  });

  // The reported defect. A project agent is not an agent of any workspace, so
  // the feed has no label for it and it fell through to four characters of id.
  it("names the project's own agent after the project", () => {
    expect(actorName("project-01M2SCB", { projectName: "Build" })).toBe("Build agent");
    expect(actorName({ kind: "project_agent" }, { projectName: "Build" })).toBe("Build agent");
    expect(actorName("project-01M2SCB", { projectName: "wire-facade" })).toBe("wire-facade agent");
  });

  it("falls back to a name rather than an id when the project has none", () => {
    expect(actorName("project-01M2SCB")).toBe("Build agent");
    expect(actorName("project-01M2SCB", { projectName: "" })).toBe("Build agent");
  });

  it("says You for the reader", () => {
    expect(actorName({ kind: "user" })).toBe("You");
    expect(actorName("user")).toBe("You");
  });

  // Never a bare id: an agent this client cannot name still wears the four
  // characters it wears everywhere else.
  it("never puts a bare id on screen", () => {
    const said = actorName("agent-01M2ZZZZ", { agentLabels: LABELS });
    expect(said).toBe("Agent 01M2");
    expect(said).not.toContain("agent-");
  });

  it("takes a name the bridge already wrote", () => {
    expect(actorName("Zech")).toBe("Zech");
  });

  it("says nothing for nothing", () => {
    expect(actorName(null)).toBe("");
    expect(actorName("")).toBe("");
  });
});
