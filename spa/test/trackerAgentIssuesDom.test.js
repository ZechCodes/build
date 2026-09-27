/** @vitest-environment jsdom */
// What an agent is carrying on the issue board, as the rows behind its Issues
// pill (#34).
//
// This supplier asks the bridge for nothing. The project's issue list is
// already on disk, and an `issues` push makes the sync layer rewrite that
// record — so it listens to the RECORD and re-supplies off the push with no
// read of its own. The last describe here is the one that proves it.
//
// It draws nothing at all now: the surfaces layer draws these rows behind a
// pill like every other kind (core/agentSurfaces.js), which is what gives the
// fold for finished work without a second one being invented beside it.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { columns, issue } from "./trackerWireFixture.js";

/** What this device's greeting said. The supplier never reads it (#104
 *  review), so a case that empties it proves the rows come off the cache. */
const CARRIES_ISSUES = { changes: { subscriptions: true, kinds: ["state", "thread", "git", "files", "terminals", "issues"] } };
const NO_GREETING = { changes: { subscriptions: false, kinds: [] } };
let greeting = CARRIES_ISSUES;
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => greeting,
}));

const ME = "agent-01M2ME";
const THEM = "agent-01M2THEM";

let trackerCache, mountAgentIssues, entry, changes;

const mine = (over = {}) => issue({ assignee: { kind: "agent", agent_id: ME }, ...over });

/** Put a project's issues on disk, the way the sync layer does. */
const putIssues = (issues) =>
  trackerCache.writeIssuesRecord("dev-1", "proj-1", { issues, columns: columns() });

/** Mount, and wait for the first read of the cache to land — it says so
 *  through `onChanged` — rather than for a count of ticks. */
const mount = async (over = {}) => {
  entry = mountAgentIssues({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => changes++, ...over });
  await vi.waitFor(() => expect(changes).toBeGreaterThan(0));
  return entry;
};

/** The rows for one agent, said as `state:#number` so a case can assert the
 *  order and the standing at once. */
const rows = (agentId = ME) =>
  (entry.entriesFor(agentId) || []).map((one) => `${one.state}:#${one.number}`);

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  greeting = CARRIES_ISSUES;
  changes = 0;
  trackerCache = await import("../src/core/trackerCache.js");
  ({ mountAgentIssues } = await import("../src/core/trackerAgentIssuesEntry.js"));
});

afterEach(() => {
  entry?.dispose();
  entry = null;
});

describe("the rows it supplies", () => {
  it("read in progress, then assigned, then tracked, then what it is finished with", async () => {
    await putIssues([
      mine({ number: 1, id: "i1", status: "backlog" }),
      mine({ number: 2, id: "i2", status: "in_progress" }),
      mine({ number: 3, id: "i3", status: "done" }),
      issue({ number: 4, id: "i4", assignee: { kind: "agent", agent_id: THEM }, trackers: [ME] }),
      mine({ number: 5, id: "i5", status: "in_review" }),
    ]);
    await mount();
    expect(rows()).toEqual([
      "in_progress:#2",
      "assigned:#1",
      "tracked:#4",
      "in_review:#5",
      "done:#3",
    ]);
  });

  // The whole of the maintainer's complaint: In review was sitting above the
  // fold, and an agent that has said "this is ready to look at" has nothing
  // more to do with it. It is still OPEN — the Issues tab still lists it —
  // which is a different question asked by a different reader.
  it("counts In review among what the agent is finished with", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "in_review" })]);
    await mount();
    expect(entry.entriesFor(ME)[0].state).toBe("in_review");
  });

  // Closed is true wherever the card sits, so it is asked before the column.
  it("calls a closed issue closed, whatever column it was closed from", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "backlog", state: "closed" })]);
    await mount();
    expect(rows()).toEqual(["closed:#1"]);
  });

  it("carries what a row needs to draw itself", async () => {
    await putIssues([mine({ number: 7, id: "i7", title: "Kanban drag", status: "in_progress", updated_at: "2026-08-21T10:00:00Z" })]);
    await mount();
    expect(entry.entriesFor(ME)[0]).toEqual({
      id: "i7", state: "in_progress", number: 7, title: "Kanban drag",
      status: "in_progress", updated_at: "2026-08-21T10:00:00Z",
    });
  });

  it("supplies most recently moved first within one standing", async () => {
    await putIssues([
      mine({ number: 1, id: "i1", status: "in_progress", updated_at: "2026-08-21T10:00:00Z" }),
      mine({ number: 2, id: "i2", status: "in_progress", updated_at: "2026-08-21T12:00:00Z" }),
    ]);
    await mount();
    expect(rows()).toEqual(["in_progress:#2", "in_progress:#1"]);
  });

  it("follows the agent it is asked about", async () => {
    await putIssues([
      mine({ number: 1, id: "i1", status: "in_progress" }),
      issue({ number: 2, id: "i2", assignee: { kind: "agent", agent_id: THEM }, status: "in_progress" }),
    ]);
    await mount();
    expect(rows(ME)).toEqual(["in_progress:#1"]);
    expect(rows(THEM)).toEqual(["in_progress:#2"]);
  });
});

// No rows means no pill. Not an empty one: a pill that is always there and
// opens on nothing is one a reader learns to skip.
describe("when there is nothing to show", () => {
  it("supplies nothing for an agent holding and tracking nothing", async () => {
    await putIssues([issue({ number: 1, id: "i1", assignee: { kind: "agent", agent_id: THEM } })]);
    await mount();
    expect(entry.entriesFor(ME)).toBeNull();
  });

  it("supplies nothing when no agent is in focus", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    await mount();
    expect(entry.entriesFor(null)).toBeNull();
  });

  // A bridge that has never carried issues writes no list, so there is none.
  it("supplies nothing where no list is cached", async () => {
    await mount();
    expect(entry.entriesFor(ME)).toBeNull();
  });
});

// Paint from cache: a cold or offline start has no greeting yet, or a bridge
// that is gone. The pill's rows come off the cached list all the same.
describe("before any bridge answers", () => {
  it("supplies the cached rows with no greeting", async () => {
    greeting = NO_GREETING;
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    entry = mountAgentIssues({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => changes++ });
    await vi.waitFor(() => expect(rows()).toEqual(["in_progress:#1"]));
  });
});

describe("staying live off the record, with nothing asked of the bridge", () => {
  it("re-supplies when the pushed list moves an issue", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "backlog" })]);
    await mount();
    expect(rows()).toEqual(["assigned:#1"]);

    changes = 0;
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);

    await vi.waitFor(() => expect(rows()).toEqual(["in_progress:#1"]));
    expect(changes).toBeGreaterThan(0);
  });

  it("picks up an issue that did not exist when it mounted", async () => {
    await putIssues([]);
    await mount();
    expect(entry.entriesFor(ME)).toBeNull();

    await putIssues([mine({ number: 9, id: "i9", status: "in_progress" })]);

    await vi.waitFor(() => expect(rows()).toEqual(["in_progress:#9"]));
  });

  it("stops listening once it is disposed", async () => {
    await putIssues([mine({ number: 1, id: "i1", status: "backlog" })]);
    await mount();
    entry.dispose();
    entry = null;

    // A live supplier on the same record is the settle point: it hears the
    // write after the disposed one would have, and reads in the order asked,
    // so once it has re-supplied the disposed one has had every chance to.
    let heard = 0;
    const live = mountAgentIssues({ deviceId: "dev-1", projectId: "proj-1", onChanged: () => heard++ });
    await vi.waitFor(() => expect(heard).toBeGreaterThan(0));

    changes = 0;
    await putIssues([mine({ number: 1, id: "i1", status: "in_progress" })]);
    await vi.waitFor(() => expect(live.entriesFor(ME).map((one) => one.state)).toEqual(["in_progress"]));

    expect(changes).toBe(0);
    live.dispose();
  });
});
