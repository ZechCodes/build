// The harness's half of the Done gate.
//
// `workspace.finish` removes the workspace, so the bridge refuses one whose
// work is only in it. The compose suite cannot run here — it needs the stack —
// but the reasoning it does before it calls finish can, and this is that:
// which directories would block, and whether a refusal is the one expected.

import { describe, it } from "node:test";
import assert from "node:assert/strict";
import {
  FINISH_BLOCKERS,
  FINISH_REFUSAL_PREFIX,
  everyDirectoryIsARepository,
  gitDirectoriesOf,
  isFinishRefusal,
  plainDirectoriesOf,
  refusedBecause,
} from "./finishGate.mjs";

const gitDir = (id) => ({ id, source_id: `source-${id}`, is_git: true });
const plainDir = (id) => ({ id, source_id: `source-${id}`, is_git: false });

/** A refusal as the bridge sends it: the prefix, then the blockers' sentences
 *  joined — `finish_refusal` in bridge/src/workspace.rs. */
const refusal = (...blockers) => new Error(`${FINISH_REFUSAL_PREFIX}: ${blockers.join("; ")}`);

describe("which directories stand between a workspace and Done", () => {
  it("names the ones that are not repositories", () => {
    const workspace = { directories: [gitDir("d1"), plainDir("d2"), gitDir("d3")] };

    assert.deepEqual(plainDirectoriesOf(workspace).map((d) => d.id), ["d2"]);
    assert.deepEqual(gitDirectoriesOf(workspace).map((d) => d.id), ["d1", "d3"]);
    assert.equal(everyDirectoryIsARepository(workspace), false);
  });

  it("passes a workspace whose directories are all repositories", () => {
    assert.equal(everyDirectoryIsARepository({ directories: [gitDir("d1"), gitDir("d2")] }), true);
  });

  it("passes a workspace holding no directory at all, which has nothing to lose", () => {
    assert.equal(everyDirectoryIsARepository({ directories: [] }), true);
    assert.equal(everyDirectoryIsARepository({}), true);
    assert.equal(everyDirectoryIsARepository(null), true);
  });

  it("reads a directory the bridge did not call a repository as plain", () => {
    // The safe direction: guessing wrong the other way asks Done to remove
    // something nothing else holds.
    assert.equal(everyDirectoryIsARepository({ directories: [{ id: "d1" }] }), false);
    assert.equal(everyDirectoryIsARepository({ directories: [{ id: "d1", is_git: "yes" }] }), false);
  });
});

describe("reading the gate's refusal", () => {
  it("tells the gate's refusal from any other failure", () => {
    assert.equal(isFinishRefusal(refusal(FINISH_BLOCKERS.unpushed)), true);
    assert.equal(isFinishRefusal(new Error("unknown method workspace.finish")), false);
    assert.equal(isFinishRefusal(new Error("workspace_id is required")), false);
    assert.equal(isFinishRefusal(null), false);
    assert.equal(isFinishRefusal(undefined), false);
  });

  it("says which blocker a refusal named", () => {
    const blocked = refusal(FINISH_BLOCKERS.plainDirectory);

    assert.equal(refusedBecause(blocked, FINISH_BLOCKERS.plainDirectory), true);
    assert.equal(refusedBecause(blocked, FINISH_BLOCKERS.unpushed), false);
  });

  it("finds a blocker among several, in the order the bridge lists them", () => {
    const blocked = refusal(FINISH_BLOCKERS.unpushed, FINISH_BLOCKERS.plainDirectory);

    assert.equal(refusedBecause(blocked, FINISH_BLOCKERS.unpushed), true);
    assert.equal(refusedBecause(blocked, FINISH_BLOCKERS.plainDirectory), true);
    assert.equal(refusedBecause(blocked, FINISH_BLOCKERS.dirty), false);
  });

  it("does not call a blocker-shaped message from somewhere else a refusal", () => {
    // The sentence alone is not the gate; the prefix is what says it was.
    const elsewhere = new Error("git.status failed: it has commits no remote has");

    assert.equal(refusedBecause(elsewhere, FINISH_BLOCKERS.unpushed), false);
  });

  it("takes a plain string as well as an Error, since a wire refusal may be either", () => {
    assert.equal(isFinishRefusal(`${FINISH_REFUSAL_PREFIX}: ${FINISH_BLOCKERS.dirty}`), true);
  });
});

describe("the sentences the harness asserts on", () => {
  it("are the bridge's own, so a drift fails loudly rather than passing", () => {
    // Mirrored from `blocker_sentence` in bridge/src/workspace.rs. If that
    // wording changes, these strings must change with it — and the compose
    // checks that use them will say so.
    assert.deepEqual(FINISH_BLOCKERS, {
      agentWorking: "an agent is working in it",
      dirty: "it has uncommitted changes",
      unpushed: "it has commits no remote has",
      plainDirectory: "it holds a directory that is not a repository",
      unknown: "its Git state could not be read",
    });
    assert.equal(FINISH_REFUSAL_PREFIX, "workspace.finish is not available yet");
  });
});
