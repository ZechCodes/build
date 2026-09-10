// @vitest-environment jsdom
// The one create surface: a modal with a Branch tab and an Issue tab, opened
// by the toolbar's work menu and by the rail's project blocks alike.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { motionBeat } from "./motionRecorder.js";

const refreshFeed = vi.fn(async () => {});
vi.mock("../src/core/taskFeed.js", () => ({
  refreshFeed: (...args) => refreshFeed(...args),
  subscribeFeed: () => () => {},
  startFeed: () => {},
  stopFeed: () => {},
  primaryRunIdFor: () => null,
}));

const { App } = await import("../src/app.js");
const { openCreateWork, createWorkHtml, CREATE_KINDS } = await import("../src/core/createWork.js");

const flush = () => new Promise((done) => setTimeout(done, 0));
const modal = () => document.querySelector("#create-scrim .modal");
const input = () => modal().querySelector("#create-work-input");
const tab = (kind) => modal().querySelector(`[data-create-tab="${kind}"]`);
const type = (text) => {
  input().value = text;
  input().dispatchEvent(new Event("input"));
};
const press = (key, extra = {}) => input().dispatchEvent(new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true, ...extra }));
const rows = () => [...modal().querySelectorAll("[data-branch-pick]")];
const rowNames = () => rows().map((row) => row.querySelector(".branch-row-name").textContent);
const highlighted = () => rows().findIndex((row) => row.getAttribute("aria-selected") === "true");
/** What the modal did beyond reading the project's branches, which it reads
 *  whenever the Branch tab is up. */
const mutations = () => App.call.mock.calls.filter(([method]) => method !== "git.branches");

const listed = (name, stamps = {}) => ({
  name,
  is_current: false,
  remote: null,
  upstream: null,
  ahead: 0,
  behind: 0,
  head_subject: "",
  head_time: 0,
  holder: null,
  ...stamps,
});

/** The one holder the bridge names for a branch, as the wire carries it. */
const heldBy = (kind, id) => ({ holder: { kind, id } });

let navigate;
let branches;

beforeEach(() => {
  document.body.innerHTML = "";
  localStorage.clear();
  App.modelCatalog = null;
  App.focusComposerOnMount = false;
  branches = [];
  App.call = vi.fn(async (method) => {
    if (method === "git.branches") return { current: "main", branches };
    if (method === "run.adopt") return { run_id: "r-7", branch: "feature-x" };
    if (method === "worktree.create") return { project_id: "p1", branch: "build/mascot-model-spike", worktree_id: "wt-9" };
    if (method === "issue.create") return { project_id: "p1", issue_id: "plan-9", plan_id: "plan-9" };
    return {};
  });
  refreshFeed.mockClear();
  navigate = vi.fn();
});

describe("the create modal", () => {
  it("offers a Branch tab and an Issue tab, and opens on the one asked for, named for the project", () => {
    expect(CREATE_KINDS).toEqual(["branch", "issue"]);
    openCreateWork({ projectId: "p1", projectName: "relaydb", kind: "issue", navigate });
    expect(modal().querySelector("h3").textContent).toBe("New in relaydb");
    expect([...modal().querySelectorAll("[data-create-tab]")].map((t) => [t.dataset.createTab, t.getAttribute("aria-selected")])).toEqual([
      ["branch", "false"],
      ["issue", "true"],
    ]);
    expect(input().tagName).toBe("TEXTAREA");
    expect(modal().querySelector(".agent-choice")).toBeTruthy();
    expect(document.activeElement).toBe(input());
  });

  it("opens on Branch by default, with no harness question, and previews the branch the daemon will name", () => {
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    expect(tab("branch").getAttribute("aria-selected")).toBe("true");
    expect(input().tagName).toBe("INPUT");
    expect(modal().querySelector(".agent-choice")).toBeNull();
    type("Mascot Model Spike!");
    expect(modal().querySelector("#create-work-preview").textContent).toBe("build/mascot-model-spike");
  });

  it("keeps what was typed on each tab when switching between them", () => {
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    type("a branch name");
    tab("issue").click();
    expect(input().value).toBe("");
    type("an issue goal");
    tab("branch").click();
    expect(input().value).toBe("a branch name");
    tab("issue").click();
    expect(input().value).toBe("an issue goal");
  });

  it("cuts the branch, closes, re-reads the feed, and opens it through the caller's navigate with the composer focused", async () => {
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    type("Mascot Model Spike!");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", name: "Mascot Model Spike!" });
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "build/mascot-model-spike", tab: "changes" });
    expect(App.focusComposerOnMount).toBe(true);
    expect(refreshFeed).toHaveBeenCalled();
    expect(modal()).toBeNull();
  });

  // The daemon cuts the checkout with its state lock released and answers when
  // the git lands. A reply that names no branch — or one the browser's own timer
  // gave up on — is not a failure: the board is already carrying the row as
  // Creating, and it opens itself when the record settles.
  it("closes and leaves the board carrying the row when the create answers without a branch", async () => {
    App.call = vi.fn(async () => ({ project_id: "p1", pending_worktree_id: "wt-pending" }));
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    type("mascot spike");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal()).toBeNull();
    expect(refreshFeed).toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
    expect(App.focusComposerOnMount).toBe(false);
  });

  it("closes the same way when the reply outlives the browser's timer", async () => {
    App.call = vi.fn(async () => {
      const timedOut = new Error("worktree.create timed out");
      timedOut.timedOut = true;
      timedOut.uncertain = true;
      throw timedOut;
    });
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    type("mascot spike");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal()).toBeNull();
    expect(refreshFeed).toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("closes an issue the same way when its reply outlives the timer", async () => {
    App.call = vi.fn(async () => {
      const timedOut = new Error("issue.create timed out");
      timedOut.timedOut = true;
      timedOut.uncertain = true;
      throw timedOut;
    });
    openCreateWork({ projectId: "p2", projectName: "mascot", kind: "issue", navigate });
    type("Add a health endpoint");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal()).toBeNull();
    expect(refreshFeed).toHaveBeenCalled();
    expect(navigate).not.toHaveBeenCalled();
  });

  it("files an issue that starts nothing, carrying the harness choice", async () => {
    openCreateWork({ projectId: "p2", projectName: "mascot", kind: "issue", navigate });
    type("Add a health endpoint");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("issue.create", expect.objectContaining({ goal: "Add a health endpoint", project_id: "p2", dispatch: false }));
    expect(navigate).toHaveBeenCalledWith({ name: "issue", projectId: "p1", id: "plan-9" });
    expect(App.focusComposerOnMount).toBe(false);
    expect(modal()).toBeNull();
  });

  it("files an untouched issue with the agent displayed when the account default is not offered", async () => {
    App.modelCatalog = {
      default_provider: "pi",
      providers: [
        { id: "pi", label: "Pi", models: [], efforts: [] },
        { id: "claude_adk", label: "Claude Code", models: [], efforts: [] },
        { id: "codex", label: "Codex", models: [], efforts: [] },
      ],
    };
    openCreateWork({ projectId: "p2", projectName: "mascot", kind: "issue", navigate });
    expect(modal().querySelector("#create-choice-provider").value).toBe("claude_adk");
    type("Add a health endpoint");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("issue.create", {
      goal: "Add a health endpoint",
      project_id: "p2",
      dispatch: false,
      provider: "claude_adk",
    });
  });

  it("submits a branch on Enter, and an issue only on a modified Enter", async () => {
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    type("spike");
    press("Enter");
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", expect.anything());

    App.call.mockClear();
    openCreateWork({ projectId: "p1", projectName: "relaydb", kind: "issue", navigate });
    type("a goal");
    press("Enter");
    await flush();
    expect(App.call).not.toHaveBeenCalled();
    press("Enter", { metaKey: true });
    await flush();
    expect(App.call).toHaveBeenCalledWith("issue.create", expect.anything());
  });

  it("refuses an empty answer instead of creating something unnamed", async () => {
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal().querySelector(".create-error").textContent).toBe("Name it first.");
    tab("issue").click();
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal().querySelector(".create-error").textContent).toBe("Describe the issue first.");
    expect(mutations()).toEqual([]);
  });

  it("says what went wrong without losing what was typed, and lets you try again", async () => {
    App.call = vi.fn(async () => {
      throw new Error("a branch named build/scratch already exists");
    });
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    type("scratch");
    modal().querySelector("[data-create-go]").click();
    await flush();
    expect(modal().querySelector(".create-error").textContent).toContain("already exists");
    expect(input().value).toBe("scratch");
    expect(modal().querySelector("[data-create-go]").disabled).toBe(false);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("closes on Cancel and on Escape, creating nothing", async () => {
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    modal().querySelector("[data-create-cancel]").click();
    await motionBeat();
    expect(modal()).toBeNull();
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    await motionBeat();
    expect(modal()).toBeNull();
    expect(mutations()).toEqual([]);
  });

  it("escapes the project's name", () => {
    const html = createWorkHtml({ projectName: "<b>x</b>", kind: "branch", values: {}, busy: false, error: "", choice: {}, choiceOpen: false });
    expect(html).not.toContain("<b>x</b>");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
    expect(html).toContain('class="modal modal-create"');
  });
});

describe("the create modal's branch picker", () => {
  const openOnBranches = async (rowsListed) => {
    branches = rowsListed;
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    await flush();
  };

  it("lists the project's branches when the tab opens, saying what pressing each one does", async () => {
    await openOnBranches([
      listed("main", { is_current: true, ...heldBy("primary_checkout", "wt-root") }),
      listed("feature-x"),
      listed("feature-remote", { remote: "origin" }),
      listed("feature-run", heldBy("run", "r-1")),
      listed("feature-elsewhere", heldBy("external_worktree", "wt-3")),
    ]);
    expect(App.call).toHaveBeenCalledWith("git.branches", { project_id: "p1" });
    expect(rowNames()).toEqual(["main", "feature-x", "feature-remote", "feature-run", "feature-elsewhere"]);
    expect(rows().map((row) => row.querySelector(".branch-row-verb").textContent)).toEqual([
      "Adopt",
      "Check out",
      "Fetch & check out",
      "Open",
      "Adopt",
    ]);
    expect(rows()[2].textContent).toContain("origin");
    expect(rows()[0].textContent).toContain("primary checkout");
    expect(rows()[4].textContent).toContain("another worktree");
  });

  it("asks for the branches only when the Branch tab is the one being looked at", async () => {
    branches = [listed("feature-x")];
    openCreateWork({ projectId: "p1", projectName: "relaydb", kind: "issue", navigate });
    await flush();
    expect(App.call).not.toHaveBeenCalled();
    tab("branch").click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("git.branches", { project_id: "p1" });
    expect(rowNames()).toEqual(["feature-x"]);
  });

  it("filters the list as you type, offering the branch the text would cut first", async () => {
    await openOnBranches([listed("main"), listed("feature-core-moderation")]);
    type("fcm");
    expect(rowNames()).toEqual(["build/fcm", "feature-core-moderation"]);
    expect(modal().querySelector("#create-work-preview").textContent).toBe("build/fcm");
    expect(input().value).toBe("fcm");
    expect(document.activeElement).toBe(input());
  });

  it("previews the row a press would take, so a branch that exists is never previewed as one about to be cut", async () => {
    await openOnBranches([listed("feature-x")]);
    type("feature-x");
    expect(rowNames()).toEqual(["feature-x"]);
    expect(modal().querySelector("#create-work-preview").textContent).toBe("feature-x");
  });

  it("checks out a branch nothing holds, and opens it with the composer focused", async () => {
    await openOnBranches([listed("feature-x")]);
    rows()[0].click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", branch: "feature-x" });
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "feature-x", tab: "changes" });
    expect(App.focusComposerOnMount).toBe(true);
    expect(refreshFeed).toHaveBeenCalled();
    expect(modal()).toBeNull();
  });

  it("adopts the worktree a branch is already checked out in, then opens it", async () => {
    await openOnBranches([listed("feature-elsewhere", heldBy("external_worktree", "wt-3"))]);
    rows()[0].click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("run.adopt", { project_id: "p1", worktree_id: "wt-3" });
    expect(App.call).not.toHaveBeenCalledWith("worktree.create", expect.anything());
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "feature-elsewhere", tab: "changes" });
    expect(App.focusComposerOnMount).toBe(false);
  });

  it("adopts the primary checkout by what it is", async () => {
    await openOnBranches([listed("main", { is_current: true, ...heldBy("primary_checkout", "wt-root") })]);
    rows()[0].click();
    await flush();
    expect(App.call).toHaveBeenCalledWith("run.adopt", { project_id: "p1", primary: true });
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "main", tab: "changes" });
  });

  it("just opens a branch a run already owns", async () => {
    await openOnBranches([listed("feature-run", heldBy("run", "r-1"))]);
    App.call.mockClear();
    rows()[0].click();
    await flush();
    expect(mutations()).toEqual([]);
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "feature-run", tab: "changes" });
    expect(App.focusComposerOnMount).toBe(false);
  });

  it("moves the highlight with the arrows and presses it with Enter", async () => {
    await openOnBranches([listed("main"), listed("feature-x")]);
    expect(highlighted()).toBe(-1);
    press("ArrowDown");
    press("ArrowDown");
    expect(highlighted()).toBe(1);
    press("ArrowUp");
    expect(highlighted()).toBe(0);
    press("Enter");
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", branch: "main" });
  });

  it("highlights the branch it would cut as soon as there is text, so Enter still cuts it", async () => {
    await openOnBranches([listed("main")]);
    type("spike");
    expect(highlighted()).toBe(0);
    expect(rowNames()[0]).toBe("build/spike");
    press("Enter");
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", name: "spike" });
  });

  it("means the branch itself when the text spells one exactly", async () => {
    await openOnBranches([listed("feature-x")]);
    type("feature-x");
    expect(rowNames()).toEqual(["feature-x"]);
    press("Enter");
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", branch: "feature-x" });
  });

  it("keeps the typed text, the caret and the rows when a pick is refused", async () => {
    await openOnBranches([listed("feature-x")]);
    App.call = vi.fn(async () => {
      throw new Error("branch \"feature-x\" is already checked out by run r-1");
    });
    type("feature-x");
    input().setSelectionRange(3, 3);
    press("Enter");
    await flush();
    expect(modal().querySelector(".create-error").textContent).toContain("already checked out");
    expect(input().value).toBe("feature-x");
    expect(input().selectionStart).toBe(3);
    expect(rowNames()).toEqual(["feature-x"]);
    expect(navigate).not.toHaveBeenCalled();
  });

  it("means the branch itself when the arrows have left nothing highlighted", async () => {
    await openOnBranches([listed("feature-x")]);
    type("feature-x");
    expect(highlighted()).toBe(0);
    press("ArrowUp");
    expect(highlighted()).toBe(-1);
    press("Enter");
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", branch: "feature-x" });
    expect(App.call).not.toHaveBeenCalledWith("worktree.create", { project_id: "p1", name: "feature-x" });
  });

  it("says nothing about matches while the listing is still on the wire, and says it once the answer is in", async () => {
    let answer;
    App.call = vi.fn(
      (method) => new Promise((resolve) => {
        answer = () => resolve({ current: "main", branches: [] });
        if (method !== "git.branches") resolve({});
      }),
    );
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    await flush();
    type("!!!");
    expect(modal().querySelector(".branch-picker-note")).toBeNull();
    answer();
    await flush();
    expect(modal().querySelector(".branch-picker-note").textContent).toBe("No branch matches.");
  });

  it("lets a listing that lands after the modal was dismissed fall on the floor, painting nothing and taking no focus", async () => {
    let answer;
    App.call = vi.fn(
      (method) => new Promise((resolve) => {
        answer = () => resolve({ current: "main", branches: [listed("feature-x")] });
        if (method !== "git.branches") resolve({});
      }),
    );
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    await flush();
    const focus = vi.spyOn(HTMLElement.prototype, "focus");
    try {
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      await motionBeat();
      expect(document.querySelector("#create-scrim")).toBeNull();
      answer();
      await flush();
      expect(document.querySelector("#create-scrim")).toBeNull();
      expect(focus).not.toHaveBeenCalled();
    } finally {
      focus.mockRestore();
    }
  });

  it("scrolls the row the arrows land on into view, so Enter presses what can be seen", async () => {
    const original = Element.prototype.scrollIntoView;
    const scrolled = [];
    Element.prototype.scrollIntoView = function scrollIntoView(options) {
      scrolled.push([this.dataset.branchPick, options]);
    };
    try {
      await openOnBranches([listed("main"), listed("feature-x")]);
      press("ArrowDown");
      press("ArrowDown");
      expect(scrolled).toEqual([
        ["0", { block: "nearest" }],
        ["1", { block: "nearest" }],
      ]);
      press("ArrowUp");
      press("ArrowUp");
      expect(scrolled).toHaveLength(3);
      expect(highlighted()).toBe(-1);
    } finally {
      Element.prototype.scrollIntoView = original;
    }
  });

  it("lets a failure after the create surface instead of painting it as a refused create", async () => {
    navigate = vi.fn(() => {
      throw new Error("no route for that branch");
    });
    await openOnBranches([listed("feature-x")]);
    const pressed = rows()[0].onclick();
    await expect(pressed).rejects.toThrow("no route for that branch");
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", branch: "feature-x" });
    expect(refreshFeed).toHaveBeenCalled();
    expect(modal()).toBeNull();
  });

  it("still cuts a branch by name when the listing itself cannot be read", async () => {
    App.call = vi.fn(async (method) => {
      if (method === "git.branches") throw new Error("not a git repository");
      return { project_id: "p1", branch: "build/spike" };
    });
    openCreateWork({ projectId: "p1", projectName: "relaydb", navigate });
    await flush();
    expect(modal().querySelector(".branch-picker-note").textContent).toContain("not a git repository");
    type("spike");
    press("Enter");
    await flush();
    expect(App.call).toHaveBeenCalledWith("worktree.create", { project_id: "p1", name: "spike" });
    expect(navigate).toHaveBeenCalledWith({ name: "branch", projectId: "p1", branch: "build/spike", tab: "changes" });
  });
});
