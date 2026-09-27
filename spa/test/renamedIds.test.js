// #190: an id minted before the rename names the record under its new prefix.
import { describe, expect, it } from "vitest";
import { currentId } from "../src/core/renamedIds.js";

describe("an id from before tasks were renamed", () => {
  it("reads a task, a comment and an event under their new prefixes", () => {
    expect(currentId("issue-01M3HPVBKS8JPRKXP3JXM4W6VQ")).toBe("task-01M3HPVBKS8JPRKXP3JXM4W6VQ");
    expect(currentId("ic-01M3HPXXE28HXBB5VB2QBHQRXV")).toBe("tc-01M3HPXXE28HXBB5VB2QBHQRXV");
    expect(currentId("ie-01M3HPVBKS8JPRKXP3JXM4W6VR")).toBe("te-01M3HPVBKS8JPRKXP3JXM4W6VR");
  });

  it("leaves a current id, any other id and a non-string alone", () => {
    for (const id of ["task-1", "tc-7", "te-2", "run-1", "agent-01", "issued", "", null, undefined, 42]) {
      expect(currentId(id)).toBe(id);
    }
  });
});
