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
