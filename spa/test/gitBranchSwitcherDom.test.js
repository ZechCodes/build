// @vitest-environment jsdom
// The branch switcher, end to end in the DOM: opening it loads git.branches,
// picking a local branch checks it out, and picking a branch the switcher
// flagged as checked out in another worktree adopts that worktree instead —
// git refuses the same branch checked out twice, so a plain checkout there
// would fail.

import { describe, it, expect, beforeEach } from "vitest";
import { mountGitPane } from "../src/core/gitPane.js";

const cleanStatus = () => ({
  branch: "main",
  path: "/repo",
  head: "f".repeat(40),
  repo_state: "clean",
  upstream: "origin/main",
  ahead: 0,
  behind: 0,
  stash_count: 0,
  files: [],
  files_truncated: false,
  stat: { files_changed: 0, insertions: 0, deletions: 0 },
  patch: "",
  truncated: false,
});

const log = () => ({ branch: "main", commits: [], more: false });

const branchesPayload = () => ({
  current: "main",
  branches: [
    { name: "main", is_current: true, upstream: "origin/main", ahead: 0, behind: 0, head_subject: "init", head_time: 1, stat: { insertions: 0, deletions: 0 } },
    { name: "feat/login", is_current: false, upstream: null, ahead: 0, behind: 0, head_subject: "wip", head_time: 2, stat: { insertions: 12, deletions: 4 } },
    {
      name: "feat/parked",
      is_current: false,
      upstream: null,
      ahead: 0,
      behind: 0,
      head_subject: "parked elsewhere",
      head_time: 3,
      stat: { insertions: 2, deletions: 0 },
      external_worktree_id: "wt-9",
    },
  ],
});

async function mount({ rpc = {} } = {}) {
  const calls = [];
  const callRpc = async (method, params) => {
    calls.push({ method, params });
    if (rpc[method]) return rpc[method](params);
    if (method === "git.status") return cleanStatus();
    if (method === "git.log") return log();
    if (method === "git.branches") return branchesPayload();
    return {};
  };
  const navigated = [];
  const container = document.createElement("div");
  document.body.appendChild(container);
  const pane = mountGitPane(container, {
    scope: { project_id: "p1" },
    callRpc,
    onNavigate: (route) => navigated.push(route),
  });
  await settle();
  return { container, pane, calls, navigated };
}

const settle = async () => {
  for (let i = 0; i < 8; i++) await Promise.resolve();
  await new Promise((resolve) => setTimeout(resolve, 0));
};

const click = async (element) => {
  element.dispatchEvent(new window.MouseEvent("click", { bubbles: true }));
  await settle();
};

beforeEach(() => {
  document.body.innerHTML = "";
});

describe("the branch switcher", () => {
  it("loads and renders the diffstat and adopt-elsewhere flag on open", async () => {
    const { container } = await mount();
    await click(container.querySelector(".gtbranchbtn"));
    const menu = container.querySelector(".gtbranch-menu") || document.querySelector(".gtbranch-menu");
    expect(menu).toBeTruthy();
    expect(menu.textContent).toContain("+12");
    expect(menu.textContent).toContain("−4");
    expect(menu.querySelector('[data-branch="feat/parked"]').dataset.externalWorktreeId).toBe("wt-9");
    expect(menu.textContent).toContain("in another worktree");
  });

  it("checks out a plain local branch on click", async () => {
    const { container, calls } = await mount({
      rpc: { "git.checkout": () => cleanStatus() },
    });
    await click(container.querySelector(".gtbranchbtn"));
    const menu = document.querySelector(".gtbranch-menu");
    await click(menu.querySelector('[data-branch="feat/login"]'));
    expect(calls.some((c) => c.method === "git.checkout" && c.params.branch === "feat/login")).toBe(true);
    expect(calls.some((c) => c.method === "run.adopt")).toBe(false);
  });

  it("adopts the worktree and navigates instead of checking out a branch parked elsewhere", async () => {
    const { container, calls, navigated } = await mount({
      rpc: { "run.adopt": (params) => ({ ok: true, run_id: "run-9", worktree_id: params.worktree_id }) },
    });
    await click(container.querySelector(".gtbranchbtn"));
    const menu = document.querySelector(".gtbranch-menu");
    await click(menu.querySelector('[data-branch="feat/parked"]'));

    expect(calls.some((c) => c.method === "git.checkout")).toBe(false);
    const adopt = calls.find((c) => c.method === "run.adopt");
    expect(adopt.params).toEqual({ project_id: "p1", worktree_id: "wt-9" });
    expect(navigated).toEqual([{ name: "branch", projectId: "p1", branch: "feat/parked", tab: "changes" }]);
  });
});
