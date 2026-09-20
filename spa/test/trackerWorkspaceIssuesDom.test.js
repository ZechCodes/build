/** @vitest-environment jsdom */
// The issues icon beside the workspace's settings cog, and the view it opens.
//
// Both halves read the project's cached issue list and listen to that record,
// so an `issues` push moves the badge on the bar and the rows under an open
// overlay without a read of their own.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

let carriedKinds = ["state", "thread", "git", "files", "terminals", "issues"];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: carriedKinds } }),
}));

const ONE = "agent-01M2ONE";
const TWO = "agent-01M2TWO";
const ELSEWHERE = "agent-01M2ELSE";

let button, trackerCache, mountWorkspaceIssues, block, agents;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const held = (agentId, over = {}) => issue({ assignee: { kind: "agent", agent_id: agentId }, ...over });
const putIssues = (issues) => trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });

const mount = async (over = {}) => {
  block = mountWorkspaceIssues(button, {
    deviceId: "dev-1",
    projectId: "proj-1",
    workspaceName: () => "wire-facade",
    agents: () => agents,
    ...over,
  });
  await flush();
  return block;
};

const badge = () => button.querySelector(".tb-issues-count").textContent;
const overlay = () => document.querySelector(".modal-workspace-issues");
const agentSections = () =>
  [...document.querySelectorAll("[data-issues-agent]")].map((one) => one.querySelector("h3").textContent);
const cardsUnder = (label) => {
  const section = [...document.querySelectorAll("[data-issues-agent]")]
    .find((one) => one.querySelector("h3").textContent === label);
  return [...(section?.querySelectorAll(".agent-issue-card") || [])].map(
    (one) => one.querySelector(".issue-number").textContent,
  );
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  carriedKinds = ["state", "thread", "git", "files", "terminals", "issues"];
  agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
  document.body.innerHTML =
    '<button data-workspace-issues hidden><span class="tb-issues-count"></span></button>';
  button = document.querySelector("[data-workspace-issues]");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountWorkspaceIssues } = await import("../src/core/trackerWorkspaceIssuesView.js"));
});

afterEach(() => {
  block?.dispose();
  block = null;
  document.body.innerHTML = "";
});

describe("the badge", () => {
  // A badge is a call to look, and finished work is not one.
  it("counts the open issues this workspace's agents hold", async () => {
    await putIssues([
      held(ONE, { number: 1, id: "i1", status: "in_progress" }),
      held(TWO, { number: 2, id: "i2", status: "ready" }),
      held(ONE, { number: 3, id: "i3", status: "done" }),
      held(ELSEWHERE, { number: 4, id: "i4", status: "in_progress" }),
    ]);
    await mount();
    expect(badge()).toBe("2");
    expect(button.hidden).toBe(false);
  });

  it("says nothing when nothing is open, but keeps the way in", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "done" })]);
    await mount();
    expect(badge()).toBe("");
    expect(button.hidden).toBe(false);
  });

  // No icon at all, rather than one reading zero.
  it("is not drawn on a bridge that does not carry issues", async () => {
    carriedKinds = ["state", "thread", "git", "files", "terminals"];
    await putIssues([held(ONE, { number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    expect(button.hidden).toBe(true);
    expect(badge()).toBe("");
  });

  it("moves on the push, with nothing asked of the bridge", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "ready" })]);
    await mount();
    expect(badge()).toBe("1");
    await putIssues([
      held(ONE, { number: 1, id: "i1", status: "ready" }),
      held(TWO, { number: 2, id: "i2", status: "in_progress" }),
    ]);
    await flush();
    expect(badge()).toBe("2");
  });

  // A workspace gains and loses agents while the bar stands there.
  it("re-reads the agents when the bar says they moved", async () => {
    await putIssues([held(TWO, { number: 1, id: "i1", status: "in_progress" })]);
    agents = [{ id: ONE, ordinal: 1 }];
    await mount();
    expect(badge()).toBe("");
    agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
    block.refresh();
    expect(badge()).toBe("1");
  });
});

describe("the view it opens", () => {
  const someIssues = () => [
    held(ONE, { number: 1, id: "i1", status: "in_progress", title: "Kanban drag" }),
    held(ONE, { number: 2, id: "i2", status: "ready" }),
    held(ONE, { number: 3, id: "i3", status: "done" }),
    held(TWO, { number: 4, id: "i4", status: "in_review" }),
    held(ELSEWHERE, { number: 5, id: "i5", status: "in_progress" }),
  ];

  it("opens on a press and names the workspace", async () => {
    await putIssues(someIssues());
    await mount();
    button.click();
    await flush();
    expect(overlay()).not.toBeNull();
    expect(overlay().querySelector("h3").textContent).toBe("Issues in wire-facade");
  });

  it("groups the rows by agent, in the order the row lists them", async () => {
    await putIssues(someIssues());
    await mount();
    button.click();
    await flush();
    expect(agentSections()).toEqual(["Agent 1", "Agent 2"]);
  });

  it("puts in-progress first within an agent and folds what is finished", async () => {
    await putIssues(someIssues());
    await mount();
    button.click();
    await flush();
    const section = [...document.querySelectorAll("[data-issues-agent]")][0];
    expect([...section.querySelectorAll("[data-agent-issue-group]")].map((one) => one.dataset.agentIssueGroup))
      .toEqual(["working", "holding", "finished"]);
    const fold = section.querySelector('details[data-agent-issue-group="finished"]');
    expect(fold.open).toBe(false);
    fold.querySelector("summary").click();
    expect(fold.open).toBe(true);
  });

  it("gives each row its number, title, column and moved time, linking to the issue", async () => {
    await putIssues(someIssues());
    await mount();
    button.click();
    await flush();
    const card = document.querySelector(".agent-issue-card");
    expect(card.querySelector(".issue-number").textContent).toBe("#1");
    expect(card.querySelector(".agent-issue-title").textContent).toBe("Kanban drag");
    expect(card.querySelector(".issue-status").textContent).toBe("In progress");
    expect(card.querySelector("time")).not.toBeNull();
    expect(card.getAttribute("href")).toBe("#/device/dev-1/project/proj-1/issues/i1");
  });

  it("leaves out an issue held by an agent of another workspace", async () => {
    await putIssues(someIssues());
    await mount();
    button.click();
    await flush();
    expect(cardsUnder("Agent 1")).toEqual(["#1", "#2", "#3"]);
    expect(document.body.innerHTML).not.toContain("#5");
  });

  // The one place an empty state IS drawn: the reader pressed a button to get
  // here and is owed an answer.
  it("says so when the workspace's agents hold nothing", async () => {
    await putIssues([held(ELSEWHERE, { number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    button.click();
    await flush();
    expect(overlay().textContent).toContain("No issues assigned in this workspace.");
  });

  it("moves the rows under an open view when the push moves them", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "ready" })]);
    await mount();
    button.click();
    await flush();
    expect(cardsUnder("Agent 1")).toEqual(["#1"]);

    await putIssues([
      held(ONE, { number: 1, id: "i1", status: "ready" }),
      held(ONE, { number: 7, id: "i7", status: "in_progress" }),
    ]);
    await flush();
    expect(cardsUnder("Agent 1")).toEqual(["#7", "#1"]);
  });

  it("takes the view down with it", async () => {
    await putIssues(someIssues());
    await mount();
    button.click();
    await flush();
    expect(overlay()).not.toBeNull();
    block.dispose();
    block = null;
    await flush();
    expect(overlay()).toBeNull();
  });
});
