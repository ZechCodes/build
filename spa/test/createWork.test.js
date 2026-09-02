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

let navigate;

beforeEach(() => {
  document.body.innerHTML = "";
  localStorage.clear();
  App.modelCatalog = null;
  App.focusComposerOnMount = false;
  App.call = vi.fn(async (method) => {
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
    expect(App.call).not.toHaveBeenCalled();
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
    expect(App.call).not.toHaveBeenCalled();
  });

  it("escapes the project's name", () => {
    const html = createWorkHtml({ projectName: "<b>x</b>", kind: "branch", values: {}, busy: false, error: "", choice: {}, choiceOpen: false });
    expect(html).not.toContain("<b>x</b>");
    expect(html).toContain("&lt;b&gt;x&lt;/b&gt;");
    expect(html).toContain('class="modal modal-create"');
  });
});
