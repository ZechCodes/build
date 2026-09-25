// Closing out a branch: which surfaces offer it, what it sends, and what the
// confirmation promises. Done on a branch DELETES it, and the bridge's
// `branch.finish` is the only way a branch leaves the board, so the model that
// decides when to offer it and what to say has to be right before any of it
// reaches the DOM.

import { describe, it, expect } from "vitest";
import {
  branchCloseout,
  branchFinishConfirm,
  branchFinishFacts,
  branchFinishParams,
  branchInboxKey,
  branchKeptNotice,
} from "../src/core/branchFinish.js";
import { entryKeyOf } from "../src/core/inbox.js";

const clean = { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" };

const branchRow = (over = {}) => ({
  kind: "branch",
  deviceId: "dev-1",
  project_id: "p1",
  projectKey: "dev-1/p1",
  branch: "build/login",
  state: "review",
  worktree_id: "wt-1",
  run_id: null,
  issue_id: null,
  primary: false,
  can_finish: true,
  finish: { warnings: [] },
  stat: clean,
  ...over,
});

describe("whether a branch can be closed out here", () => {
  it("offers the control on a worktree-backed branch", () => {
    const closeout = branchCloseout(branchRow());
    expect(closeout.shown).toBe(true);
    expect(closeout.options.map((option) => option.id)).toEqual(["finish_delete"]);
  });

  it("offers the control on a run-backed branch", () => {
    expect(branchCloseout(branchRow({ run_id: "run-1", worktree_id: "wt-1" })).shown).toBe(true);
  });

  it("hides the control when no checkout on this device carries the branch", () => {
    expect(branchCloseout(null).shown).toBe(false);
    expect(branchCloseout(branchRow({ worktree_id: null, run_id: null })).shown).toBe(false);
  });

  // `can_finish` is structural — whether there is anything here to finish — not
  // a judgement about the state of the work. Uncommitted, unpushed and unmerged
  // are warnings, and a shown control is always pressable.
  it("hides the control when the bridge says there is nothing to finish", () => {
    expect(branchCloseout(branchRow({ can_finish: false })).shown).toBe(false);
  });

  it("offers exactly one behavior: delete, named after the branch it deletes", () => {
    const [only] = branchCloseout(branchRow(), { deletesBranch: true }).options;
    expect(only.label).toBe("Done");
    expect(only.menuLabel).toBe("Done — delete the branch");
    expect(only.description).toBe("delete branch build/login and its checkout");
    expect(only.danger).toBe(true);
  });

  // #87: a bridge without `branches.finishDelete` keeps the branch whatever it
  // is sent, so the option promises only the checkout.
  it("promises no deletion where the bridge keeps the branch", () => {
    const [only] = branchCloseout(branchRow()).options;
    expect(only.label).toBe("Done");
    expect(only.menuLabel).toBe("Done — remove the checkout");
    expect(only.description).toBe("remove the checkout of build/login; the branch stays");
    expect(`${only.menuLabel} ${only.description} ${only.busyLabel}`).not.toMatch(/delet/);
  });
});

describe("what Done sends", () => {
  it("deletes the branch where the bridge deletes it", () => {
    expect(branchFinishParams("finish_delete", { projectId: "p1", branch: "build/login", deletesBranch: true })).toEqual({
      project_id: "p1",
      branch: "build/login",
      action: "delete",
    });
  });

  it("sends no action to a bridge that would drop it", () => {
    expect(branchFinishParams("finish_delete", { projectId: "p1", branch: "build/login" })).toEqual({
      project_id: "p1",
      branch: "build/login",
    });
  });

  it("refuses an option it has no action for", () => {
    expect(() => branchFinishParams("finish_cleanup", { projectId: "p1", branch: "b" })).toThrow(/unknown/);
  });
});

// #87: the bridge measures the branch again once the checkout is gone, and a
// branch that moved in between stays. The workspace is gone either way.
describe("what Done says when the branch stayed", () => {
  it("says the checkout went and the branch stayed, in the bridge's sentence", () => {
    const reason = "Build cannot delete the branch build/login: it gained commits while Build was deleting it.";
    expect(branchKeptNotice("build/login", { deleted: true, branch_deleted: false, branch_reason: reason })).toEqual({
      summary: "Removed the checkout of build/login; the branch stays",
      detail: reason,
    });
  });

  it("says nothing when the branch went, or when nothing was asked of it", () => {
    expect(branchKeptNotice("build/login", { deleted: true, branch_deleted: true })).toBe(null);
    expect(branchKeptNotice("build/login", { deleted: true })).toBe(null);
    expect(branchKeptNotice("build/login", undefined)).toBe(null);
  });
});

describe("the facts Done speaks about", () => {
  it("reads them off the row, and names the URL's branch when the read has not answered", () => {
    const facts = branchFinishFacts(
      branchRow({
        issue_id: "issue-1",
        finish: { warnings: [{ code: "unpushed", message: "build/login has 2 commits that origin/build/login does not" }] },
      }),
      "build/fallback",
    );
    expect(facts).toEqual({
      branch: "build/login",
      issueId: "issue-1",
      merged: false,
      warnings: ["build/login has 2 commits that origin/build/login does not"],
    });
    expect(branchFinishFacts(null, "build/fallback").branch).toBe("build/fallback");
    expect(branchFinishFacts(null, "build/fallback").warnings).toEqual([]);
  });

  it("knows when the work landed", () => {
    expect(branchFinishFacts(branchRow({ state: "merged" }), "b").merged).toBe(true);
  });
});

describe("what the confirmation promises", () => {
  const deleting = (row) => ({ ...branchFinishFacts(row, "build/login"), deletesBranch: true });

  it("outlines the deletion, as the destructive verb it is", () => {
    const plan = branchFinishConfirm(deleting(branchRow()));
    expect(plan.intro).toBe("Done deletes the branch. This cannot be undone.");
    expect(plan.actions[0]).toBe("Delete branch build/login");
    expect(plan.danger).toBe(true);
    expect(plan.confirmLabel).toBe("Delete");
  });

  it("says the bridge is too old, and promises only the checkout, where it keeps the branch", () => {
    const plan = branchFinishConfirm({
      ...branchFinishFacts(branchRow({ issue_id: "issue-1" }), "build/login"),
      deviceName: "studio",
    });
    expect(plan.intro).toBe(
      "Build cannot delete the branch on studio: the bridge is too old. Done removes its checkout and keeps the branch.",
    );
    expect(plan.actions).toEqual([
      "Remove its checkout",
      "Take its conversation off the inbox",
      "Return the issue it implements to the inbox",
    ]);
    expect(plan.confirmLabel).toBe("Remove");
  });

  it("carries what the bridge says the deletion would cost", () => {
    const plan = branchFinishConfirm(
      branchFinishFacts(
        branchRow({
          finish: {
            warnings: [{ code: "uncommitted", message: "build/login has 2 uncommitted files — removing the checkout discards them" }],
          },
        }),
        "build/login",
      ),
    );
    expect(plan.warnings).toEqual(["build/login has 2 uncommitted files — removing the checkout discards them"]);
  });

  // Deleting an unmerged branch hands its issue back to the inbox; merging
  // first files the issue away with it. The outline says which, before the click.
  it("says where the issue it implements ends up", () => {
    const back = branchFinishConfirm(deleting(branchRow({ issue_id: "issue-1" })));
    expect(back.actions.join(" ")).toContain("Return the issue it implements to the inbox, noting that build/login was deleted");

    const archived = branchFinishConfirm(branchFinishFacts(branchRow({ issue_id: "issue-1", state: "merged" }), "build/login"));
    expect(archived.actions.join(" ")).toContain("Archive the issue");
  });

  it("leaves the issue out when the branch implements none", () => {
    const plan = branchFinishConfirm(branchFinishFacts(branchRow(), "build/login"));
    expect(plan.actions.some((action) => action.includes("issue"))).toBe(false);
  });
});

describe("the inbox row this branch is", () => {
  const urlNames = { projectId: "p1", branch: "build/login", projectKey: "dev-1/p1" };

  it("names a run-backed branch the way the inbox names it", () => {
    const backing = branchRow({ run_id: "run-1", worktree_id: "wt-1" });
    expect(branchInboxKey(backing, urlNames)).toBe(entryKeyOf(backing));
    expect(branchInboxKey(backing, urlNames)).toBe("run-1");
  });

  it("names a bare checkout the way the inbox names it", () => {
    const backing = branchRow({ run_id: null, worktree_id: "wt-1" });
    expect(branchInboxKey(backing, urlNames)).toBe(entryKeyOf(backing));
    expect(branchInboxKey(backing, urlNames)).toBe("wt-1");
  });

  it("names a branch with no checkout by its project and its name", () => {
    const backing = branchRow({ run_id: null, worktree_id: null });
    expect(branchInboxKey(backing, urlNames)).toBe(entryKeyOf(backing));
    expect(branchInboxKey(backing, urlNames)).toBe("branch:dev-1/p1:build/login");
  });

  it("keys a no-entity row by the projectKey it is given", () => {
    // The surface's own read (`branch.get`) is one device's answer and carries
    // no account-wide name, so the caller says which project this is — and the
    // key the inbox is holding the same row under matches.
    const fromTheWire = { kind: "branch", project_id: "p1", branch: "build/login", run_id: null, worktree_id: null };
    expect(branchInboxKey(fromTheWire, urlNames)).toBe("branch:dev-1/p1:build/login");
    expect(branchInboxKey(fromTheWire, { ...urlNames, projectKey: "dev-2/p1" })).toBe("branch:dev-2/p1:build/login");
    // A row that already knows its own project outranks what it is told.
    expect(branchInboxKey(branchRow({ run_id: null, worktree_id: null }), { ...urlNames, projectKey: "dev-2/p1" })).toBe(
      "branch:dev-1/p1:build/login",
    );
  });

  it("names the URL's branch when the read has not answered", () => {
    expect(branchInboxKey(null, urlNames)).toBe("branch:dev-1/p1:build/login");
  });
});
