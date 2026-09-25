/** @vitest-environment jsdom */
// #144, with nothing mocked between the greeting and the tab: the bridge's
// greeting (fixtures/api/v1/session.hello.json, which bridge/tests pins to
// the real reply) names `issues.commentUserNotifies`; the real greeting path
// writes the rule to the real cache; the mounted Issues tab reads it there.
// Needs you then holds what is assigned to the user, not what is In review
// between agents — also on a cold mount before any greeting, and a bridge
// without the name keeps the earlier rule.

import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;

const { bridgeCapabilities, greetBridge, resetChangeEvents } = await import("../src/core/changeEvents.js");
const { mountIssuesPane } = await import("../src/core/trackerIssuesPane.js");
const { readNeedsYouRule } = await import("../src/core/needsYouRule.js");
const { columns, comment, issue } = await import("./trackerWireFixture.js");

const greeting = JSON.parse(readFileSync(resolve(process.cwd(), "../fixtures/api/v1/session.hello.json"), "utf8")).result;
const PROJECT = "p1";
const betweenAgents = issue({
  id: "issue-review", number: 1, status: "in_review", watched: true,
  assignee: { kind: "agent", agent_id: "agent-astra" },
});
const withTheUser = issue({ id: "issue-mine", number: 2, status: "in_review", watched: true, assignee: { kind: "user" } });

let host;
let pane;

beforeEach(() => {
  resetChangeEvents();
  globalThis.indexedDB = new IDBFactory();
  document.body.innerHTML = '<div id="issues"></div>';
  host = document.querySelector("#issues");
});

afterEach(() => {
  pane?.dispose();
  resetChangeEvents();
});

const call = (hello) => async (method) => {
  if (method === "session.hello") return hello;
  if (method === "issues.list") return { issues: [betweenAgents, withTheUser] };
  if (method === "issues.columns") return { columns: columns() };
  return {};
};

function mount(rpc, feed = () => ({ workspaces: [], items: [], projects: [] })) {
  pane = mountIssuesPane(host, {
    projectId: PROJECT,
    projectName: "Build",
    deviceId: "dev-1",
    projectKey: `dev-1|${PROJECT}`,
    callRpc: rpc,
    catalog: () => ({ providers: [] }),
    refreshCatalog: async () => ({ providers: [] }),
    feed,
    defaultView: "dashboard",
    navigate: () => {},
  });
}

const needsYou = () => [...host.querySelectorAll('[data-dashboard-section="needsYou"] .issue-dashboard-row')]
  .map((row) => row.dataset.issue);

it("holds only what is assigned to the user, from the cache, before and after the greeting", async () => {
  expect(greeting.capabilities).toContain("issues.commentUserNotifies");
  await greetBridge(call(greeting), { deviceId: "dev-1", strict: true });
  expect(bridgeCapabilities("dev-1").issues.commentUserNotifies).toBe(true);
  await vi.waitFor(async () => expect(await readNeedsYouRule("dev-1")).toBe(true));
  mount(call(greeting));
  await vi.waitFor(() => expect(needsYou()).toEqual([withTheUser.id]));

  // A cold reload: no greeting yet, and a bridge that has not answered. The
  // cache holds the list and the rule, and the tab draws what it will keep.
  pane.dispose();
  resetChangeEvents();
  host.innerHTML = "";
  mount(() => new Promise(() => {}));
  expect(bridgeCapabilities("dev-1").issues.commentUserNotifies).toBe(false);
  await vi.waitFor(() => expect(needsYou()).toEqual([withTheUser.id]));
});

it("keeps In review in Needs you for a bridge that does not name the rule", async () => {
  const older = { ...greeting, capabilities: greeting.capabilities.filter((name) => name !== "issues.commentUserNotifies") };
  await greetBridge(call(older), { deviceId: "dev-1", strict: true });
  mount(call(older));
  await vi.waitFor(() => expect(needsYou().sort()).toEqual([withTheUser.id, betweenAgents.id]));
  expect(await readNeedsYouRule("dev-1")).toBe(false);
});

it("holds a question an agent asked, while the board's feed row still says nothing is unread", async () => {
  // The question reached the cache through an `issues` push; the board's feed
  // row is re-read only with the board, and still counts nothing unread.
  const asked = { ...betweenAgents, read_through: "ie-01K5Z1", updated_at: "2026-08-21T11:00:00Z" };
  const question = comment({
    id: "ic-01K5Z3", issue_id: asked.id, author: { kind: "agent", agent_id: "agent-astra" },
    body: "Which of the two fixes do you want?", notifies_user: true,
  });
  const rpc = async (method, params) => {
    if (method === "issues.get" && params?.issue_id === asked.id) return { issue: asked, timeline: [question] };
    if (method === "issues.list") return { issues: [asked, withTheUser] };
    return call(greeting)(method);
  };
  const staleRow = { kind: "tracker_issue", projectKey: `dev-1|${PROJECT}`, issue_id: asked.id, unread: 0 };
  await greetBridge(rpc, { deviceId: "dev-1", strict: true });
  mount(rpc, () => ({ workspaces: [], items: [staleRow], projects: [] }));
  await vi.waitFor(() => expect(needsYou().sort()).toEqual([asked.id, withTheUser.id].sort()));
  // And with no feed row for it at all.
  pane.dispose();
  host.innerHTML = "";
  mount(rpc);
  await vi.waitFor(() => expect(needsYou().sort()).toEqual([asked.id, withTheUser.id].sort()));
});
