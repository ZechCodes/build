// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { confirmAction } from "../src/core/confirm.js";
import { wireRailDoneControls } from "../src/core/railDone.js";
import { requestSheetDismiss } from "../src/core/sheetDismiss.js";

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

const plan = { plan_id: "pl1", project_id: "p1", can_archive: true };
const run = {
  run_id: "r1",
  project_id: "p1",
  can_finish: true,
  goal: "feature run",
  branch: "feat/rail",
  base_branch: "main",
  stat: {
    branch: "feat/rail",
    upstream: "origin/feat/rail",
    ahead: 1,
    uncommitted: { files_changed: 2 },
  },
};
const worktree = {
  worktree_id: "w1",
  project_id: "p1",
  can_finish: true,
  name: "feature",
  branch: "feat/rail",
  base_branch: "main",
  upstream: "origin/feat/rail",
  dirty_files: 2,
  unpushed: 1,
};

function railHtml(button) {
  document.body.innerHTML = `<div id="scrim"><div id="sheet"></div></div><aside id="rail"><div class="srow" id="row">${button}<span class="warn" data-done-error hidden></span></div></aside>`;
  return document.getElementById("rail");
}

function dependencies(over = {}) {
  return {
    plans: [plan],
    runs: [run],
    worktrees: [worktree],
    callRpc: vi.fn().mockResolvedValue({ ok: true }),
    confirm: vi.fn().mockResolvedValue(true),
    refresh: vi.fn().mockResolvedValue(),
    ...over,
  };
}

describe("rail Done controls (DOM)", () => {
  beforeEach(() => {
    document.body.innerHTML = "";
  });

  it("stops plan Done from navigating, archives with exact params, and refreshes", async () => {
    const rail = railHtml('<button class="btn mini" data-done-plan="pl1">Done</button>');
    const rowClick = vi.fn();
    document.getElementById("row").onclick = rowClick;
    const deps = dependencies();
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-plan]").click();
    await tick();

    expect(rowClick).not.toHaveBeenCalled();
    expect(deps.callRpc).toHaveBeenCalledWith("plan.archive", { plan_id: "pl1" });
    expect(deps.refresh).toHaveBeenCalledOnce();
  });

  it("removes a confirmed row before the background RPC finishes", async () => {
    const rail = railHtml('<button class="btn mini" data-done-plan="pl1">Done</button>');
    let finishRpc;
    const callRpc = vi.fn(() => new Promise((resolve) => { finishRpc = resolve; }));
    const deps = dependencies({ callRpc });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-plan]").click();
    await tick();

    expect(callRpc).toHaveBeenCalledOnce();
    expect(document.getElementById("row")).toBeNull();
    expect(deps.refresh).not.toHaveBeenCalled();

    finishRpc({ ok: true });
    await tick();
    expect(deps.refresh).toHaveBeenCalledOnce();
  });

  it("finishes a run through the same chooser and archive workflow", async () => {
    const rail = railHtml('<button class="btn mini" data-done-run="r1">Done</button>');
    const openChooser = vi.fn((_worktree, actions, choose) => {
      expect(actions.map((action) => action.id)).toEqual(["push", "merge", "delete"]);
      choose("push");
    });
    const deps = dependencies({ openChooser });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-run]").click();
    await tick();

    expect(deps.callRpc).toHaveBeenCalledWith("run.finish", {
      run_id: "r1",
      action: "push",
    });
    expect(deps.refresh).toHaveBeenCalledOnce();
  });

  it("chooses a dirty worktree action before confirming and sends exact params", async () => {
    const rail = railHtml('<button class="btn mini" data-done-worktree="w1">Done</button>');
    const openChooser = vi.fn((_worktree, actions, choose) => {
      expect(actions.map((action) => action.id)).toEqual(["push", "merge", "delete"]);
      choose("merge");
    });
    const deps = dependencies({ openChooser });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-worktree]").click();
    await tick();

    expect(deps.callRpc).toHaveBeenCalledWith("worktree.finish", {
      project_id: "p1",
      worktree_id: "w1",
      action: "merge",
    });
    expect(deps.refresh).toHaveBeenCalledOnce();
  });

  it("confirms clean worktree cleanup directly and sends the cleanup action", async () => {
    const rail = railHtml('<button class="btn mini" data-done-worktree="clean">Done</button>');
    const cleanWorktree = { ...worktree, worktree_id: "clean", dirty_files: 0 };
    const deps = dependencies({ worktrees: [cleanWorktree] });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-worktree]").click();
    await tick();

    expect(deps.confirm).toHaveBeenCalledOnce();
    expect(deps.callRpc).toHaveBeenCalledWith("worktree.finish", {
      project_id: "p1",
      worktree_id: "clean",
      action: "cleanup",
    });
  });

  it("cancels the dirty-action chooser without confirming or mutating", () => {
    const rail = railHtml('<button class="btn mini" data-done-worktree="w1">Done</button>');
    const deps = dependencies();
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-worktree]").click();
    expect(document.getElementById("scrim").classList.contains("show")).toBe(true);
    document.getElementById("wtfinish-cancel").click();

    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
    expect(deps.confirm).not.toHaveBeenCalled();
    expect(deps.callRpc).not.toHaveBeenCalled();
  });

  it("Escape closes the dirty-action chooser without confirming or mutating", () => {
    const rail = railHtml('<button class="btn mini" data-done-worktree="w1">Done</button>');
    const deps = dependencies();
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-worktree]").click();
    requestSheetDismiss();

    expect(document.getElementById("scrim").classList.contains("show")).toBe(false);
    expect(deps.confirm).not.toHaveBeenCalled();
    expect(deps.callRpc).not.toHaveBeenCalled();
  });

  it("Delete uses danger styling and Cancel or Escape never sends its RPC", async () => {
    const rail = railHtml('<button class="btn mini" data-done-worktree="w1">Done</button>');
    const deps = dependencies({ confirm: confirmAction });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-worktree]").click();
    document.querySelector('[data-finish-action="delete"]').click();
    expect(document.querySelector("[data-confirm-ok]").classList.contains("danger")).toBe(true);
    document.querySelector("[data-confirm-cancel]").click();
    await tick();
    expect(deps.callRpc).not.toHaveBeenCalled();

    rail.querySelector("[data-done-worktree]").click();
    document.querySelector('[data-finish-action="delete"]').click();
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    expect(deps.callRpc).not.toHaveBeenCalled();
  });

  it("Cancel and Escape confirmations never mutate", async () => {
    const rail = railHtml('<button class="btn mini" data-done-plan="pl1">Done</button>');
    const deps = dependencies({ confirm: confirmAction });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-plan]").click();
    document.querySelector("[data-confirm-cancel]").click();
    await tick();
    expect(deps.callRpc).not.toHaveBeenCalled();

    rail.querySelector("[data-done-plan]").click();
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await tick();
    expect(deps.callRpc).not.toHaveBeenCalled();
  });

  it("keeps a failed row and renders its RPC error for retry", async () => {
    const rail = railHtml('<button class="btn mini" data-done-plan="pl1">Done</button>');
    const deps = dependencies({ callRpc: vi.fn().mockRejectedValue(new Error("archive failed")) });
    wireRailDoneControls(rail, deps);

    rail.querySelector("[data-done-plan]").click();
    await tick();

    expect(document.getElementById("row")).toBeTruthy();
    expect(rail.querySelector("[data-done-error]").textContent).toBe("archive failed");
    expect(rail.querySelector("[data-done-error]").hidden).toBe(false);
    expect(rail.querySelector("[data-done-plan]").disabled).toBe(false);
    expect(deps.refresh).not.toHaveBeenCalled();
  });
});
