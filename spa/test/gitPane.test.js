import { describe, it, expect } from "vitest";
import { gitPollKey, commitSplitOptions, taskAgentCommitOptions } from "../src/core/gitPane.js";

const status = (overrides = {}) => ({
  branch: "main",
  path: "/repo",
  head: "f".repeat(40),
  files: [{ path: "a.js", staged: "none", index_status: "M", worktree_status: "M" }],
  stat: { files_changed: 1, insertions: 1, deletions: 0 },
  patch: "diff --git a/a.js b/a.js\n",
  truncated: false,
  ...overrides,
});

const log = (overrides = {}) => ({
  branch: "main",
  commits: [{ hash: "a".repeat(40), short: "aaaaaaa", subject: "s", author: "z", email: "z@x", time: 1 }],
  more: false,
  ...overrides,
});

describe("gitPollKey", () => {
  it("is stable for identical payloads", () => {
    expect(gitPollKey(status(), log())).toBe(gitPollKey(status(), log()));
  });

  it("changes when HEAD moves", () => {
    expect(gitPollKey(status(), log())).not.toBe(gitPollKey(status({ head: "e".repeat(40) }), log()));
  });

  it("changes when a file's staged state flips", () => {
    const restaged = status({ files: [{ path: "a.js", staged: "full", index_status: "M", worktree_status: "-" }] });
    expect(gitPollKey(status(), log())).not.toBe(gitPollKey(restaged, log()));
  });

  it("changes when the patch changes", () => {
    expect(gitPollKey(status(), log())).not.toBe(gitPollKey(status({ patch: "diff --git a/b b/b\n" }), log()));
  });

  it("changes when the branch changes", () => {
    expect(gitPollKey(status(), log())).not.toBe(gitPollKey(status({ branch: "dev" }), log()));
  });

  it("changes when the commit list changes", () => {
    const grown = log({ commits: [...log().commits, { hash: "b".repeat(40), short: "bbbbbbb", subject: "t", author: "z", email: "z@x", time: 2 }] });
    expect(gitPollKey(status(), log())).not.toBe(gitPollKey(status(), grown));
  });

  it("changes when paging availability changes", () => {
    expect(gitPollKey(status(), log())).not.toBe(gitPollKey(status(), log({ more: true })));
  });
});

describe("commitSplitOptions", () => {
  it("always leads with the plain Commit action", () => {
    const options = commitSplitOptions([]);
    expect(options).toHaveLength(1);
    expect(options[0].id).toBe("commit");
    expect(options[0].label).toBe("Commit");
    expect(options[0].busyLabel).toBe("Committing…");
  });

  it("appends the caller's agent options after the primary", () => {
    const agent = [{ id: "agent_commit" }, { id: "auto_commit" }];
    const options = commitSplitOptions(agent);
    expect(options.map((o) => o.id)).toEqual(["commit", "agent_commit", "auto_commit"]);
  });
});

describe("taskAgentCommitOptions", () => {
  const messageable = ["planning", "building", "blocked", "failed", "idle_unreported", "interrupted"];

  it.each(messageable)("offers Ask-agent-to-commit while %s", (state) => {
    const options = taskAgentCommitOptions(state, "fix login");
    expect(options.map((o) => o.id)).toEqual(["agent_commit", "auto_commit"]);
    expect(options[0].menuLabel).toBe("Ask agent to commit");
    expect(options[0].description).toBe("The agent writes the message");
  });

  it("drops the agent-message option when the task cannot be messaged", () => {
    for (const state of ["review", "plan_review", "done", "merged", "created"]) {
      expect(taskAgentCommitOptions(state, "g").map((o) => o.id)).toEqual(["auto_commit"]);
    }
  });

  it("describes the Build auto-commit with the task goal", () => {
    const auto = taskAgentCommitOptions("review", "fix login").find((o) => o.id === "auto_commit");
    expect(auto.menuLabel).toBe("Commit all (Build message)");
    expect(auto.description).toContain("Build: fix login");
  });
});
