/** @vitest-environment jsdom */
// The Issues face on the workspace's rail: its count, and whether it is drawn.
//
// The count reads the project's cached issue list and listens to that record,
// so an `issues` push moves the badge on the rail with nothing asked of the
// bridge.
//
// It no longer opens an overlay (#16 → #29), and it no longer owns its press
// (#174): it is a face of the rail, and the rail's own press goes to the
// workspace's Issues tab. What that tab shows is tested in
// trackerWorkspaceIssuesTab.test.js and workspaceViewDom.

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
    workspaceId: "ws-1",
    agents: () => agents,
    ...over,
  });
  await flush();
  return block;
};

const badge = () => button.querySelector(".dirtab-count").textContent;
const cellHtml = '<button data-tab="issues" hidden><span class="badge dirtab-count"></span></button>';
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
  document.body.innerHTML = cellHtml;
  button = document.querySelector("[data-tab=issues]");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountWorkspaceIssues } = await import("../src/core/trackerWorkspaceIssuesView.js"));
});

afterEach(() => {
  block?.dispose();
  block = null;
  document.body.innerHTML = "";
});

describe("the badge", () => {
  // #104: the tab carries the unread of the watched issues this workspace's
  // agents hold. An issue nobody watches never shows a count.
  it("counts the unread of the watched issues this workspace's agents hold", async () => {
    await putIssues([
      held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 2 }),
      held(TWO, { number: 2, id: "i2", status: "done", watched: true, unread_count: 1 }),
      held(ONE, { number: 3, id: "i3", status: "ready", unread_count: 5 }),
      held(ELSEWHERE, { number: 4, id: "i4", status: "in_progress", watched: true, unread_count: 4 }),
    ]);
    await mount();
    expect(badge()).toBe("3");
    expect(button.hidden).toBe(false);
    expect(button.title).toBe("3 unread · 2 open issues in this workspace");
  });

  it("says nothing when nothing is unread, but keeps the way in and says what is open", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "ready", watched: true, unread_count: 0 })]);
    await mount();
    expect(badge()).toBe("");
    expect(button.hidden).toBe(false);
    expect(button.title).toBe("1 open issue in this workspace");
  });

  // No icon at all, rather than one reading zero.
  it("is not drawn on a bridge that does not carry issues", async () => {
    carriedKinds = ["state", "thread", "git", "files", "terminals"];
    await putIssues([held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 1 })]);
    await mount();
    expect(button.hidden).toBe(true);
    expect(badge()).toBe("");
  });

  it("moves on the push, with nothing asked of the bridge", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "ready", watched: true, unread_count: 1 })]);
    await mount();
    expect(badge()).toBe("1");
    await putIssues([
      held(ONE, { number: 1, id: "i1", status: "ready", watched: true, unread_count: 0 }),
      held(TWO, { number: 2, id: "i2", status: "in_progress", watched: true, unread_count: 2 }),
    ]);
    await vi.waitFor(() => expect(badge()).toBe("2"));
  });

  // A workspace gains and loses agents while the bar stands there.
  it("re-reads the agents when the bar says they moved", async () => {
    await putIssues([held(TWO, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 1 })]);
    agents = [{ id: ONE, ordinal: 1 }];
    await mount();
    expect(badge()).toBe("");
    agents = [{ id: ONE, ordinal: 1 }, { id: TWO, ordinal: 2 }];
    block.refresh();
    expect(badge()).toBe("1");
  });
});

describe("the rail's cell", () => {
  // The rail's own press goes to the Issues tab; the badge block wires none.
  it("leaves the press to the rail", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    expect(button.onclick).toBeNull();
    button.click();
    await flush();
    expect(overlay()).toBeNull();
    expect(document.querySelector("dialog")).toBeNull();
  });

  // A paint of the rail rewrites its cells: the block moves onto the new one
  // and says the count it already holds, with nothing read again.
  it("follows the rail onto a repainted cell, keeping its count", async () => {
    await putIssues([held(ONE, { number: 1, id: "i1", status: "in_progress", watched: true, unread_count: 1 })]);
    await mount();
    expect(badge()).toBe("1");
    document.body.innerHTML = cellHtml;
    button = document.querySelector("[data-tab=issues]");
    block.retarget(button);
    expect(badge()).toBe("1");
    expect(button.hidden).toBe(false);
  });
});
