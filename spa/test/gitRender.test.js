import { describe, it, expect } from "vitest";
import { AGENT_COMMIT_MESSAGE } from "../src/core/gitRender.js";

describe("AGENT_COMMIT_MESSAGE", () => {
  it("is the exact canned agent-commit instruction", () => {
    expect(AGENT_COMMIT_MESSAGE).toBe(
      "Commit all outstanding changes in this worktree as a single atomic commit with a clear, descriptive commit message. Do not make any other changes.",
    );
  });
});
