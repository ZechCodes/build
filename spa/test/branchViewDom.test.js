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

/** A branch.get row with something to finish. `can_finish` is structural — is
 *  there anything here at all — and `finish.warnings` is what deleting it would
 *  cost, which the confirmation carries and never refuses over. */
const finishableRow = (over = {}) => ({
  ...row,
  state: "review",
  can_finish: true,
  finish: { warnings: [] },
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

  // The toolbar's create form arms this right before navigating here — the
  // one-shot signal that this landing is a branch just cut, nobody in it yet.
  it("focuses the rail's composer when the toolbar just cut this branch, and clears the flag", async () => {
    App.call = vi.fn(async () => row);
    App.focusComposerOnMount = true;
    await renderBranch();
    await flush();
    expect(document.getElementById("railinput")).toBe(document.activeElement);
    expect(App.focusComposerOnMount).toBe(false);
  });

  it("leaves focus alone on an ordinary visit", async () => {
    App.call = vi.fn(async () => row);
    await renderBranch();
    await flush();
    expect(document.getElementById("railinput")).not.toBe(document.activeElement);
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

  // The branch never resolved: every tick fails the same way, and the empty
  // state is all there is. Repainting it would rebuild the one control on it.
  it("states a branch it cannot find once, and leaves the way out standing", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    App.call = vi.fn(async () => {
      throw new Error("unknown branch");
    });
    await renderBranch();
    await vi.advanceTimersByTimeAsync(0);
    const back = document.querySelector("#branchback");
    expect(back).toBeTruthy();

    await vi.advanceTimersByTimeAsync(4000); // two more failing reads

    expect(document.querySelector("#branchback"), "the empty state was rebuilt").toBe(back);
    vi.useRealTimers();
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

  it("offers Done in the surface bar, always pressable — it is never refused", async () => {
    await mountWith(finishableRow({ finish: { warnings: [{ code: "uncommitted", message: "build/login has 2 uncommitted files" }] } }));
    expect(doneButton().textContent).toBe("Done");
    expect(doneButton().disabled).toBe(false);
  });

  // The primary checkout is the repository: there is nothing there to delete.
  it("offers nothing on a project's primary checkout", async () => {
    await mountWith(finishableRow({ primary: true, worktree_id: null }));
    expect(finishHost().innerHTML).toBe("");
  });

  it("offers nothing when the bridge says there is nothing to finish", async () => {
    await mountWith(finishableRow({ can_finish: false }));
    expect(finishHost().innerHTML).toBe("");
  });

  it("deletes the branch, and says so before it does", async () => {
    await mountWith(finishableRow());
    doneButton().click();
    await flush();
    expect(document.getElementById("confirm-scrim").textContent).toContain("Delete branch build/login");
    document.getElementById("confirm-scrim").querySelector("[data-confirm-ok]").click();
    await flush();
    expect(finishCalls()[0][1]).toEqual({ project_id: "p1", branch: "build/login", action: "delete" });
  });

  it("puts the bridge's warnings in the confirmation, above what it will do", async () => {
    await mountWith(
      finishableRow({
        finish: {
          warnings: [
            { code: "unmerged", message: "build/login has never been pushed, and has 3 commits that main does not", count: 3, ref: "main" },
          ],
        },
      }),
    );
    doneButton().click();
    await flush();
    const scrim = document.getElementById("confirm-scrim");
    expect(scrim.querySelector(".confirm-warnings").textContent).toContain("3 commits that main does not");
    expect(scrim.textContent.indexOf("3 commits")).toBeLessThan(scrim.textContent.indexOf("Delete branch"));
    scrim.querySelector("[data-confirm-cancel]").click();
    await flush();
  });

  it("does nothing at all when the confirmation is cancelled", async () => {
    await mountWith(finishableRow());
    doneButton().click();
    await answerConfirm(false);
    expect(finishCalls()).toHaveLength(0);
    // The button comes back: a cancel is not an ending.
    expect(doneButton().disabled).toBe(false);
  });

  // The row poll runs every 1.6s. Re-rendering the Done control on a tick that
  // resolved the same close-out threw away a click already in progress.
  describe("under the row poll", () => {
    beforeEach(() => {
      vi.useFakeTimers({ shouldAdvanceTime: true });
    });
    afterEach(() => {
      vi.useRealTimers();
    });

    const pollTick = async () => {
      await vi.advanceTimersByTimeAsync(2000);
    };

    it("leaves the button standing through a poll that reads the same row", async () => {
      await mountWith(finishableRow());
      const before = doneButton();
      await pollTick();
      expect(doneButton(), "the button was rebuilt by the poll").toBe(before);
    });

    it("holds the busy button through a poll while the deletion is in flight", async () => {
      let finish;
      await mountWith(finishableRow(), {
        "branch.finish": () =>
          new Promise((resolve) => {
            finish = resolve;
          }),
      });
      doneButton().click();
      await answerConfirm(true);
      expect(doneButton().disabled).toBe(true);

      await pollTick();

      expect(doneButton().disabled, "a poll re-armed the button mid-flight").toBe(true);
      finish({ branch: "build/login" });
    });
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
