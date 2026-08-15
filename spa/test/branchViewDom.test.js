// @vitest-environment jsdom
// The branch surface's lifecycle: a navigation that lands while the first read
// is still in flight must not leave a poll running for a view that is gone —
// the leaked-poller failure mode (every navigation orphans an interval that
// calls branch.get forever, and the app slows with each one).

import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const bodyHtml = readFileSync(resolve("index.html"), "utf8").match(/<body>([\s\S]*)<\/body>/)[1];

const flush = () => new Promise((done) => setTimeout(done, 0));

const row = {
  kind: "branch",
  project_id: "p1",
  project: "relaydb",
  branch: "build/login",
  worktree_id: "wt-1",
  agents: [],
};

/** A branch.get row whose work is committed and pushed — the state Done asks
 *  for (the bridge decides it and sends `can_finish`). */
const finishableRow = (over = {}) => ({
  ...row,
  can_finish: true,
  primary: false,
  issue_id: null,
  stat: { uncommitted: { files_changed: 0 }, ahead: 0, upstream: "origin/build/login" },
  ...over,
});

let App;
let renderBranch;

beforeEach(async () => {
  vi.resetModules();
  document.body.innerHTML = bodyHtml;
  location.hash = "#/p/p1/branch/build%2Flogin/changes";
  ({ App } = await import("../src/app.js"));
  ({ renderBranch } = await import("../src/views/branchView.js"));
  App.route = { name: "branch", projectId: "p1", branch: "build/login", tab: "changes" };
});

afterEach(() => {
  if (App.poll) clearInterval(App.poll);
  App.poll = null;
  if (App.viewDispose) App.viewDispose();
  App.viewDispose = null;
});

describe("the branch surface", () => {
  it("polls the row once mounted", async () => {
    App.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(App.poll).not.toBeNull();
    expect(App.call).toHaveBeenCalledWith("branch.get", expect.objectContaining({ project_id: "p1" }));
  });

  // Both tabs paint a .pane-split, which states the shell's gutters itself. In
  // a padded tab body those gutters are paid twice — a doubled inset all round —
  // and the body scrolls the two columns together instead of letting each scroll
  // in its own frame.
  it.each(["changes", "files"])("hands the %s pane a flush tab body", async (tab) => {
    App.route = { ...App.route, tab };
    App.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(document.querySelector("#tabbody").classList.contains("flush")).toBe(true);
  });

  it("installs no poll when the view was torn down mid-load", async () => {
    let answer;
    const firstRead = new Promise((r) => {
      answer = r;
    });
    App.call = vi.fn(() => firstRead);
    const mounting = renderBranch();
    // The user navigates away while branch.get is still in flight: the shell's
    // render() runs the outgoing view's teardown and clears its slots.
    App.viewDispose();
    App.viewDispose = null;
    App.poll = null;
    answer(row);
    await mounting;
    await flush();
    // A disposed view must not claim the poll slot the next view now owns.
    expect(App.poll).toBeNull();
  });
});

// The way a branch ends. Before this control the only Done was on the inbox
// row, so a branch you were standing in could not be closed out from inside it.
describe("closing the branch out", () => {
  /** Answer the confirmation modal every close-out opens. */
  const answerConfirm = async (ok) => {
    await flush();
    const scrim = document.getElementById("confirm-scrim");
    expect(scrim, "a confirmation was expected").toBeTruthy();
    scrim.querySelector(ok ? "[data-confirm-ok]" : "[data-confirm-cancel]").click();
    await flush();
  };

  const mountWith = async (branchRow, answers = {}) => {
    App.call = vi.fn(async (method, params) => {
      if (method === "branch.get") return branchRow;
      if (answers[method]) return answers[method](params);
      return {};
    });
    await renderBranch();
    await flush();
  };

  const finishHost = () => document.querySelector("#branch-finish");
  const doneButton = () => finishHost().querySelector(".btn.primary:not(.caret)");
  const finishCalls = () => App.call.mock.calls.filter(([method]) => method === "branch.finish");

  it("offers Done in the surface bar once the work is committed and pushed", async () => {
    await mountWith(finishableRow());
    expect(doneButton().textContent).toBe("Done");
    expect(doneButton().disabled).toBe(false);
  });

  it("says what is in the way instead of offering a Done that would fail", async () => {
    await mountWith(
      finishableRow({ can_finish: false, stat: { uncommitted: { files_changed: 2 }, ahead: 0, upstream: "origin/x" } }),
    );
    expect(doneButton().disabled).toBe(true);
    expect(doneButton().title).toContain("2 uncommitted files");
  });

  // The primary checkout is the repository: it is never archived away.
  it("offers nothing on a project's primary checkout", async () => {
    await mountWith(finishableRow({ primary: true, worktree_id: null }));
    expect(finishHost().innerHTML).toBe("");
  });

  it("finishes the branch and keeps it, on the default option", async () => {
    await mountWith(finishableRow());
    doneButton().click();
    await answerConfirm(true);
    expect(finishCalls()[0][1]).toEqual({ project_id: "p1", branch: "build/login", action: "cleanup" });
  });

  it("does nothing at all when the confirmation is cancelled", async () => {
    await mountWith(finishableRow());
    doneButton().click();
    await answerConfirm(false);
    expect(finishCalls()).toHaveLength(0);
    // The button comes back: a cancel is not an ending.
    expect(doneButton().disabled).toBe(false);
  });

  it("deletes the branch too, on the second option", async () => {
    await mountWith(finishableRow());
    finishHost().querySelector(".caret").click();
    finishHost().querySelector('.splitmenu .mi[data-action="finish_delete"]').click();
    await answerConfirm(true);
    expect(finishCalls()[0][1].action).toBe("delete");
  });

  // The bridge refuses to archive an issue its branch has not implemented, and
  // its error names the override. The refusal IS the disclosure.
  it("offers the unlink override the refusal names, and retries with it", async () => {
    let attempts = 0;
    await mountWith(finishableRow({ issue_id: "issue-1" }), {
      "branch.finish": (params) => {
        attempts += 1;
        if (!params.unlink)
          throw new Error("branch.finish: Done also archives the issue it implements — pass unlink to finish the branch alone");
        return { branch: "build/login" };
      },
    });
    doneButton().click();
    await answerConfirm(true); // the close-out itself
    await answerConfirm(true); // the unlink disclosure behind the refusal
    expect(attempts).toBe(2);
    expect(finishCalls()[1][1].unlink).toBe(true);
  });

  it("reports a failed close-out as a notice and restores the button", async () => {
    await mountWith(finishableRow(), {
      "branch.finish": () => {
        throw new Error("worktree.finish cleanup requires no uncommitted changes");
      },
    });
    doneButton().click();
    await answerConfirm(true);
    const notice = document.querySelector("#notices .notice.error");
    expect(notice).toBeTruthy();
    expect(notice.textContent).toContain("build/login");
    expect(doneButton().disabled).toBe(false);
  });
});
