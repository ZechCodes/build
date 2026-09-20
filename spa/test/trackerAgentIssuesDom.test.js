/** @vitest-environment jsdom */
// The issues entry in a conversation's activity area.
//
// It asks the bridge for nothing. The project's issue list is already on disk,
// and an `issues` push makes the sync layer rewrite that record — so this
// entry listens to the RECORD and repaints off the push with no read of its
// own. The last describe here is the one that proves it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

/** What this device's greeting says its subscriptions carry. A case naming a
 *  list without `issues` is an older bridge answering. */
let carriedKinds = ["state", "thread", "git", "files", "terminals", "issues"];
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => ({ changes: { subscriptions: true, kinds: carriedKinds } }),
}));

const ME = "agent-01M2ME";
const THEM = "agent-01M2THEM";

let host, cache, trackerCache, mountAgentIssues, entry;

const flush = async () => {
  for (let i = 0; i < 20; i++) await new Promise((done) => setTimeout(done, 0));
};

const mine = (over = {}) => issue({ assignee: { kind: "agent", agent_id: ME }, ...over });

/** Put a project's issues on disk, the way the sync layer does. */
const putIssues = (issues) =>
  trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });

const mount = async (agentId = ME, over = {}) => {
  entry = mountAgentIssues(host, { deviceId: "dev-1", projectId: "proj-1", ...over });
  entry.set(agentId);
  await flush();
  return entry;
};

const groupIds = () => [...host.querySelectorAll("[data-agent-issue-group]")].map((one) => one.dataset.agentIssueGroup);
const cardsIn = (group) =>
  [...host.querySelectorAll(`[data-agent-issue-group="${group}"] .agent-issue-card`)].map(
    (one) => one.querySelector(".issue-number").textContent,
  );
const foldFor = (group) => host.querySelector(`details[data-agent-issue-group="${group}"]`);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  carriedKinds = ["state", "thread", "git", "files", "terminals", "issues"];
  document.body.innerHTML = '<div id="issues-host"></div>';
  host = document.querySelector("#issues-host");
  cache = await import("../src/core/localCache.js");
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountAgentIssues } = await import("../src/core/trackerAgentIssuesEntry.js"));
});

afterEach(() => {
  entry?.dispose();
  entry = null;
});

describe("the four categories", () => {
  it("draws in-progress work first, then the rest it holds", async () => {
    await putIssues([
      mine({ number: 1, id: "i1", status: "ready" }),
      mine({ number: 2, id: "i2", status: "in_progress" }),
    ]);
    await mount();
    expect(groupIds()).toEqual(["working", "holding"]);
    expect(cardsIn("working")).toEqual(["#2"]);
    expect(cardsIn("holding")).toEqual(["#1"]);
  });

  it("gives each card its number, title, column and when it moved", async () => {
    await putIssues([mine({ number: 12, id: "i12", status: "in_review", title: "Kanban drag" })]);
    await mount();
    const card = host.querySelector(".agent-issue-card");
    expect(card.querySelector(".issue-number").textContent).toBe("#12");
    expect(card.querySelector(".agent-issue-title").textContent).toBe("Kanban drag");
    expect(card.querySelector(".issue-status").textContent).toBe("In review");
    expect(card.querySelector("time")).not.toBeNull();
  });

  it("links each card to the issue's page on this machine", async () => {
    await putIssues([mine({ number: 3, id: "issue-3", status: "in_progress" })]);
    await mount();
    expect(host.querySelector(".agent-issue-card").getAttribute("href"))
      .toBe("#/device/dev-1/project/proj-1/issues/issue-3");
  });

  it("collects what it finished, closed or moved to Done alike", async () => {
    await putIssues([
      mine({ number: 1, id: "i1", state: "closed", status: "in_progress" }),
      mine({ number: 2, id: "i2", status: "done" }),
    ]);
    await mount();
    expect(groupIds()).toEqual(["finished"]);
    expect(cardsIn("finished").sort()).toEqual(["#1", "#2"]);
  });

  it("collects what it tracks but does not hold", async () => {
    await putIssues([
      issue({ number: 9, id: "i9", assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
    ]);
    await mount();
    expect(groupIds()).toEqual(["watching"]);
    expect(cardsIn("watching")).toEqual(["#9"]);
  });

  // A category a reader has none of is not worth a heading saying so.
  it("omits an empty category rather than drawing it", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    expect(groupIds()).toEqual(["working"]);
    expect(host.querySelectorAll(".agent-issue-group")).toHaveLength(1);
  });

  it("draws nothing at all, and stays hidden, for an agent with no issues", async () => {
    await putIssues([issue({ number: 1, id: "i1", assignee: { kind: "agent", agent_id: THEM } })]);
    await mount();
    expect(host.innerHTML).toBe("");
    expect(host.hidden).toBe(true);
  });

  it("follows the agent the rail puts in focus", async () => {
    await putIssues([
      mine({ number: 1, id: "i1", status: "in_progress" }),
      issue({ number: 2, id: "i2", assignee: { kind: "agent", agent_id: THEM }, status: "in_progress" }),
    ]);
    await mount();
    expect(cardsIn("working")).toEqual(["#1"]);
    entry.set(THEM);
    await flush();
    expect(cardsIn("working")).toEqual(["#2"]);
    entry.set(null);
    await flush();
    expect(host.hidden).toBe(true);
  });
});

describe("the two collapses", () => {
  const both = () => [
    mine({ number: 1, id: "i1", status: "in_progress" }),
    mine({ number: 2, id: "i2", status: "done" }),
    mine({ number: 3, id: "i3", state: "closed" }),
    issue({ number: 4, id: "i4", assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
  ];

  // A `details` is the browser's own toggle: it works from the keyboard and a
  // screen reader with no wiring, and a repaint that leaves it alone leaves it
  // open.
  it("folds Done and Tracking, and leaves the other two open", async () => {
    await putIssues(both());
    await mount();
    expect(foldFor("finished")).not.toBeNull();
    expect(foldFor("watching")).not.toBeNull();
    expect(foldFor("working")).toBeNull();
    expect(foldFor("holding")).toBeNull();
  });

  it("comes up shut and opens on a press", async () => {
    await putIssues(both());
    await mount();
    const fold = foldFor("finished");
    expect(fold.open).toBe(false);
    fold.querySelector("summary").click();
    expect(fold.open).toBe(true);
  });

  // The count is the only thing a shut fold says, so it is never inside the
  // part that hides.
  it("keeps the count on the fold's own head, shut or open", async () => {
    await putIssues(both());
    await mount();
    expect(foldFor("finished").querySelector("summary .agent-issue-count").textContent).toBe("2");
    expect(foldFor("watching").querySelector("summary .agent-issue-count").textContent).toBe("1");
  });

  it("counts on the open groups too", async () => {
    await putIssues(both());
    await mount();
    expect(host.querySelector('[data-agent-issue-group="working"] .agent-issue-count').textContent).toBe("1");
  });
});

describe("a bridge that does not carry issues", () => {
  // Not an empty box: an entry that is always there and usually empty is one a
  // reader learns to skip, and there is nothing to put in it here anyway.
  it("draws nothing and reads nothing", async () => {
    carriedKinds = ["state", "thread", "git", "files", "terminals"];
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    expect(host.innerHTML).toBe("");
    expect(host.hidden).toBe(true);
  });
});

describe("repainting on the push", () => {
  // The sync layer rewrites the project's record when an `issues` item arrives.
  // This entry listens to the record, so the push repaints it with no read of
  // its own and no full pass.
  it("moves a card when the pushed list moves it, with nothing else touched", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "ready" })]);
    await mount();
    expect(cardsIn("holding")).toEqual(["#1"]);
    expect(cardsIn("working")).toEqual([]);

    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    await flush();
    expect(cardsIn("working")).toEqual(["#1"]);
    expect(groupIds()).toEqual(["working"]);
  });

  it("draws an issue that did not exist when it mounted", async () => {
    await putIssues([]);
    await mount();
    expect(host.hidden).toBe(true);

    await putIssues([mine({ number: 5, id: "i5", status: "in_progress" })]);
    await flush();
    expect(cardsIn("working")).toEqual(["#5"]);
    expect(host.hidden).toBe(false);
  });

  it("stops listening once it is disposed", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "ready" })]);
    await mount();
    entry.dispose();
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    await flush();
    expect(cardsIn("holding")).toEqual(["#1"]);
    entry = null;
  });
});
