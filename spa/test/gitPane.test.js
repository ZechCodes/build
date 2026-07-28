import { describe, it, expect } from "vitest";
import {
  gitPollKey,
  commitSplitOptions,
  taskAgentCommitOptions,
  gitDraftKey,
  resolveCommitDraft,
  syncCommitDraft,
  commitVariantClearsDraft,
  isPermanentGitScopeError,
  pollRenderFrozen,
} from "../src/core/gitPane.js";

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

const NOW = 1_750_000_000; // fixed clock — the key carries a coarse minute bucket

describe("gitPollKey", () => {
  it("is stable for identical payloads at the same time", () => {
    expect(gitPollKey(status(), log(), NOW)).toBe(gitPollKey(status(), log(), NOW));
  });

  it("changes when HEAD moves", () => {
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(status({ head: "e".repeat(40) }), log(), NOW));
  });

  it("changes when a file's staged state flips", () => {
    const restaged = status({ files: [{ path: "a.js", staged: "full", index_status: "M", worktree_status: "-" }] });
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(restaged, log(), NOW));
  });

  it("changes when the patch changes", () => {
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(status({ patch: "diff --git a/b b/b\n" }), log(), NOW));
  });

  it("changes when the branch changes", () => {
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(status({ branch: "dev" }), log(), NOW));
  });

  it("changes when the commit list changes", () => {
    const grown = log({ commits: [...log().commits, { hash: "b".repeat(40), short: "bbbbbbb", subject: "t", author: "z", email: "z@x", time: 2 }] });
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(status(), grown, NOW));
  });

  it("changes when paging availability changes", () => {
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(status(), log({ more: true }), NOW));
  });

  it("changes when the file list truncation flag flips", () => {
    expect(gitPollKey(status(), log(), NOW)).not.toBe(gitPollKey(status({ files_truncated: true }), log(), NOW));
  });

  it.each([
    ["repo_state", { repo_state: "clean" }, { repo_state: "merging" }],
    ["upstream", { upstream: "origin/main" }, { upstream: "origin/dev" }],
    ["ahead", { ahead: 0 }, { ahead: 3 }],
    ["behind", { behind: 0 }, { behind: 3 }],
    ["stash_count", { stash_count: 0 }, { stash_count: 2 }],
  ])("changes when the additive sync field %s changes", (_field, before, after) => {
    // An out-of-band ref move (a terminal `git fetch` shifting `behind`) must
    // repaint the toolbar chips even when HEAD/files/patch are untouched.
    expect(gitPollKey(status(before), log(), NOW)).not.toBe(gitPollKey(status(after), log(), NOW));
  });

  it("stays stable within a minute but rolls over across minute buckets", () => {
    const minuteStart = Math.floor(NOW / 60) * 60;
    expect(gitPollKey(status(), log(), minuteStart)).toBe(gitPollKey(status(), log(), minuteStart + 30));
    expect(gitPollKey(status(), log(), minuteStart)).not.toBe(gitPollKey(status(), log(), minuteStart + 61));
  });
});

describe("gitDraftKey", () => {
  it("keys run scope by run_id", () => {
    expect(gitDraftKey({ run_id: "t1" })).toBe("run:t1");
  });

  it("keys project scope by project_id", () => {
    expect(gitDraftKey({ project_id: "p1" })).toBe("project:p1");
  });

  // A worktree surface carries its project's id too, so keying on that alone
  // would hand every worktree of a project the project's own commit draft.
  it("keys worktree scope by worktree_id, not the project it belongs to", () => {
    expect(gitDraftKey({ project_id: "p1", worktree_id: "wt-1" })).toBe("worktree:wt-1");
    expect(gitDraftKey({ project_id: "p1", worktree_id: "wt-1" })).not.toBe(gitDraftKey({ project_id: "p1" }));
  });

  it("never collides across scope kinds sharing an id", () => {
    expect(gitDraftKey({ run_id: "x" })).not.toBe(gitDraftKey({ project_id: "x" }));
  });
});

describe("resolveCommitDraft", () => {
  it("restores the stashed draft when no live box exists (remount after dispose)", () => {
    const stash = new Map([["task:t1", "wip: half a message"]]);
    expect(resolveCommitDraft(null, stash, "task:t1")).toBe("wip: half a message");
  });

  it("prefers the live box over the stash — even a deliberately cleared box", () => {
    const stash = new Map([["task:t1", "stale stash"]]);
    expect(resolveCommitDraft("fresh typing", stash, "task:t1")).toBe("fresh typing");
    expect(resolveCommitDraft("", stash, "task:t1")).toBe("");
  });

  it("yields an empty draft when neither box nor stash has anything", () => {
    expect(resolveCommitDraft(null, new Map(), "project:p1")).toBe("");
  });
});

describe("syncCommitDraft", () => {
  it("stores a non-empty draft under the scope key", () => {
    const stash = new Map();
    syncCommitDraft(stash, "task:t1", "wip");
    expect(stash.get("task:t1")).toBe("wip");
  });

  it("deletes the slot when the draft is emptied", () => {
    const stash = new Map([["task:t1", "wip"]]);
    syncCommitDraft(stash, "task:t1", "");
    expect(stash.has("task:t1")).toBe(false);
  });

  it("round-trips through resolveCommitDraft across a remount", () => {
    const stash = new Map();
    syncCommitDraft(stash, "task:t1", "typed before the shell rebuild");
    expect(resolveCommitDraft(null, stash, "task:t1")).toBe("typed before the shell rebuild");
  });
});

describe("commitVariantClearsDraft", () => {
  it.each(["commit", "agent_commit", "auto_commit"])("clears the draft after %s succeeds", (variant) => {
    expect(commitVariantClearsDraft(variant)).toBe(true);
  });

  it("leaves the draft alone for unknown option ids", () => {
    expect(commitVariantClearsDraft("something_else")).toBe(false);
  });
});

describe("isPermanentGitScopeError", () => {
  it.each([
    "unknown project_id",
    "unknown run_id",
    "unknown worktree_id: wt-abc",
    "provide exactly one of project_id, run_id, or project_id + worktree_id",
  ])("treats %s as terminal", (message) => {
    expect(isPermanentGitScopeError(message)).toBe(true);
  });

  it("treats other failures as transient", () => {
    expect(isPermanentGitScopeError("cannot open repository: /gone")).toBe(false);
    expect(isPermanentGitScopeError("timeout")).toBe(false);
    expect(isPermanentGitScopeError("")).toBe(false);
    expect(isPermanentGitScopeError(undefined)).toBe(false);
  });
});

describe("pollRenderFrozen", () => {
  it("always freezes while an action RPC is in flight", () => {
    expect(pollRenderFrozen({ paneRendered: true, keyUnchanged: false, draftActive: false, actionInFlight: true })).toBe(true);
    expect(pollRenderFrozen({ paneRendered: false, keyUnchanged: false, draftActive: false, actionInFlight: true })).toBe(true);
  });

  it("freezes a rendered pane while the key is unchanged or a draft is active", () => {
    expect(pollRenderFrozen({ paneRendered: true, keyUnchanged: true, draftActive: false, actionInFlight: false })).toBe(true);
    expect(pollRenderFrozen({ paneRendered: true, keyUnchanged: false, draftActive: true, actionInFlight: false })).toBe(true);
  });

  it("repaints when new content arrives with no draft and no action", () => {
    expect(pollRenderFrozen({ paneRendered: true, keyUnchanged: false, draftActive: false, actionInFlight: false })).toBe(false);
  });

  it("never freezes the very first paint unless an action is in flight", () => {
    expect(pollRenderFrozen({ paneRendered: false, keyUnchanged: true, draftActive: true, actionInFlight: false })).toBe(false);
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
  const messageable = ["building", "blocked", "failed", "idle_unreported", "interrupted"];

  it.each(messageable)("offers Ask-agent-to-commit while %s", (state) => {
    const options = taskAgentCommitOptions(state, "fix login");
    expect(options.map((o) => o.id)).toEqual(["agent_commit", "auto_commit"]);
    expect(options[0].menuLabel).toBe("Ask agent to commit");
    expect(options[0].description).toBe("The agent writes the message");
  });

  it("drops the agent-message option when the task cannot be messaged", () => {
    for (const state of ["review", "stage_gate", "merged", "created", "archived", "abandoned"]) {
      expect(taskAgentCommitOptions(state, "g").map((o) => o.id)).toEqual(["auto_commit"]);
    }
  });

  it("describes the Build auto-commit with the task goal", () => {
    const auto = taskAgentCommitOptions("review", "fix login").find((o) => o.id === "auto_commit");
    expect(auto.menuLabel).toBe("Commit all (Build message)");
    expect(auto.description).toContain("Build: fix login");
  });
});
