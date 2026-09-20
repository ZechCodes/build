/** @vitest-environment jsdom */
// The issues icon beside the workspace's settings cog: its count, and where it
// goes.
//
// The count reads the project's cached issue list and listens to that record,
// so an `issues` push moves the badge on the bar with nothing asked of the
// bridge.
//
// It no longer opens an overlay (#16 → #29). A modal is a thing you must close
// before you can act on what is in it, and closing it is leaving the issue, so
// the press now goes to the workspace's Issues TAB — which is the full tracker
// with the workspace's agents still in the rail beside it. What that tab shows
// is tested in trackerWorkspaceIssuesTab.test.js and workspaceViewDom.

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
/** Where the press said to go. The icon does not know the router — the toolbar
 *  passes `open`, because the toolbar is what knows where it is standing. */
let opened = [];

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const held = (agentId, over = {}) => issue({ assignee: { kind: "agent", agent_id: agentId }, ...over });
const putIssues = (issues) => trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });

const mount = async (over = {}) => {
  block = mountWorkspaceIssues(button, {
    deviceId: "dev-1",
    projectId: "proj-1",
    workspaceId: "ws-1",
    agents: () => agents,
    open: (where) => opened.push(where),
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
  opened = [];
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

describe("the press", () => {
  it("goes to this workspace's issues tab rather than opening anything", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    button.click();
    await flush();
    expect(opened).toEqual([{ deviceId: "dev-1", projectId: "proj-1", workspaceId: "ws-1" }]);
  });

  // The overlay is gone: nothing is mounted over the page at all.
  it("puts no overlay over the page", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    button.click();
    await flush();
    expect(document.querySelector(".modal-workspace-issues")).toBeNull();
    expect(document.querySelector("dialog")).toBeNull();
  });

  it("goes there even when the workspace's agents are holding nothing", async () => {
    await putIssues([held(ELSEWHERE, { number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    button.click();
    await flush();
    expect(opened).toHaveLength(1);
  });
});
